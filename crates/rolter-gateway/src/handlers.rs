use std::fmt::Write;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use tokio::time::sleep;

use axum::body::Body;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tracing::Instrument;

use rolter_balancer::complexity::POLICY_PARAM;
use rolter_balancer::RouteContext;

use crate::budgets::{
    decide_unpriced, resolve_unpriced_policy, ScopeIds, SpendRecorder, UnpricedDecision,
};
use crate::cache::{CachedResponse, ResponseCache};
use crate::fake_llm;
use crate::logging::RequestLog;
use crate::rate_limits::TokenRecorder;
use crate::state::{AppState, KeyMeta, Snapshot};

/// Liveness probe. Stays `200` while draining: the process is healthy, it is
/// just no longer taking new traffic.
pub async fn healthz() -> &'static str {
    "ok"
}

/// Readiness probe. Reports `503` once the control plane has marked this node
/// as draining, so a load balancer stops routing to it while in-flight
/// requests finish (#543).
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if state.draining.load(std::sync::atomic::Ordering::Relaxed) {
        return (StatusCode::SERVICE_UNAVAILABLE, "draining");
    }
    (StatusCode::OK, "ok")
}

/// Prometheus metrics endpoint.
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    // sampled here rather than maintained on the response path: queue depth is
    // a scrape-time question, and keeping it live would put a counter update on
    // every request to answer it (#1051)
    state.metrics.usage_records_queued.store(
        state.log.usage_recorders().queued() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
    // same reasoning for in-flight load: it is a live number the balancer reads
    // continuously, and the scrape is the only place that needs it summed
    state.metrics.inflight_requests.store(
        state.loads.total_in_flight(),
        std::sync::atomic::Ordering::Relaxed,
    );
    let mut body = state.metrics.render();
    // scraped upstream engine series (#850), one label set per provider
    state.upstream_metrics.render(&mut body);
    body.push_str("# HELP rolter_egress_proxy_requests_total upstream requests by proxy pool member and outcome\n");
    body.push_str("# TYPE rolter_egress_proxy_requests_total counter\n");
    body.push_str("# HELP rolter_egress_proxy_quarantined whether a proxy pool member is temporarily quarantined\n");
    body.push_str("# TYPE rolter_egress_proxy_quarantined gauge\n");
    for proxy in state.forwarder.proxy_metrics() {
        let label = proxy.proxy.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(
            body,
            "rolter_egress_proxy_requests_total{{proxy=\"{label}\",outcome=\"success\"}} {}",
            proxy.successes
        );
        let _ = writeln!(
            body,
            "rolter_egress_proxy_requests_total{{proxy=\"{label}\",outcome=\"failure\"}} {}",
            proxy.failures
        );
        let _ = writeln!(
            body,
            "rolter_egress_proxy_quarantined{{proxy=\"{label}\"}} {}",
            u8::from(proxy.quarantined)
        );
    }
    body.push_str(
        "# HELP rolter_cache_telemetry_age_seconds age of the latest cache telemetry update\n",
    );
    body.push_str("# TYPE rolter_cache_telemetry_age_seconds gauge\n");
    for (provider, source, age) in state.cache_telemetry.freshness() {
        let provider = provider.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(
            body,
            "rolter_cache_telemetry_age_seconds{{provider=\"{provider}\",source=\"{source}\"}} {age}"
        );
    }
    // adaptive-routing telemetry (#544): one series per route on the adaptive
    // strategy. counters reset when a config reload rebuilds the balancer,
    // which lines up with a rolter_config_version bump.
    body.push_str(
        "# HELP rolter_adaptive_routing_decisions_total routing picks by adaptive-routing mode\n",
    );
    body.push_str("# TYPE rolter_adaptive_routing_decisions_total counter\n");
    body.push_str(
        "# HELP rolter_adaptive_routing_engaged whether the adaptive blend is currently routing\n",
    );
    body.push_str("# TYPE rolter_adaptive_routing_engaged gauge\n");
    body.push_str(
        "# HELP rolter_adaptive_routing_target_score blended adaptive score per route target\n",
    );
    body.push_str("# TYPE rolter_adaptive_routing_target_score gauge\n");
    let snap = state.snapshot.load();
    for (model, entry) in snap.routes.iter() {
        let Some(counts) = entry.balancer.decisions() else {
            continue;
        };
        let label = model.replace('\\', "\\\\").replace('"', "\\\"");
        // the same per-target scores the control plane serves (#751), so a
        // deployment that scrapes prometheus does not need the dashboard to
        // see why a route ranks its targets the way it does
        let loads = state.loads.snapshot(model, entry.route.targets.len());
        if let Some(telemetry) = entry.balancer.telemetry(&loads) {
            for target in &telemetry.targets {
                let _ = writeln!(
                    body,
                    "rolter_adaptive_routing_target_score{{model=\"{label}\",target=\"{}\"}} {}",
                    target.target, target.score
                );
            }
        }
        for (mode, value) in [
            ("blend", counts.blend),
            ("exploration", counts.exploration),
            ("fallback", counts.fallback),
        ] {
            let _ = writeln!(
                body,
                "rolter_adaptive_routing_decisions_total{{model=\"{label}\",mode=\"{mode}\"}} {value}"
            );
        }
        let _ = writeln!(
            body,
            "rolter_adaptive_routing_engaged{{model=\"{label}\"}} {}",
            u8::from(counts.engaged)
        );
    }
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

/// OpenAI-compatible model listing built from configured routes, filtered to
/// the models the caller's virtual key is allowed to see.
pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let snap = state.snapshot.load();
    let vk = match authenticate(&state, &snap, &headers, "/v1/models") {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    let builtin = std::iter::once(fake_llm::MODEL_NAME)
        .filter(|m| !snap.routes.contains_key(*m))
        .map(str::to_string);
    // bare route ids (and the builtin) are owned_by "rolter"
    let mut data: Vec<Value> = snap
        .routes
        .iter()
        .filter(|(_, entry)| {
            vk.as_ref().is_none_or(|key| {
                key_allows_route(key, entry) && model_visible_to(Some(key), entry)
            })
        })
        .map(|(model, _)| model.clone())
        .chain(builtin)
        .filter(|m| vk.as_ref().is_none_or(|vk| vk.model_permitted(m)))
        .map(|m| json!({"id": m, "object": "model", "owned_by": "rolter"}))
        .collect();

    // provider-slug/model ids (ADR-0017): make every provider-addressable model
    // discoverable alongside the route ids. a provider's models are the upstream
    // models it serves across the configured routes' targets (and variants);
    // owned_by names the provider so a client can group by it. sorted + deduped
    // for a stable listing, and filtered by the same key allow-list
    let name_to_slug: std::collections::HashMap<&str, &str> = snap
        .providers_by_slug
        .iter()
        .map(|(slug, name)| (name.as_str(), slug.as_str()))
        .collect();
    let mut pinned: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    // provider name -> upstream models it serves, reused to expand group ids
    let mut provider_models: std::collections::HashMap<&str, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for entry in snap.routes.values() {
        let route = &entry.route;
        let targets = route
            .targets
            .iter()
            .chain(route.variants.iter().flat_map(|v| v.targets.iter()));
        for target in targets {
            let upstream = target.model.as_deref().unwrap_or(&route.model);
            provider_models
                .entry(target.provider.as_str())
                .or_default()
                .insert(upstream.to_string());
            let Some(slug) = name_to_slug.get(target.provider.as_str()) else {
                continue;
            };
            let id = format!("{slug}/{upstream}");
            if vk
                .as_ref()
                .is_none_or(|vk| vk.model_permitted(&id) && vk.provider_allowed(&target.provider))
            {
                pinned.insert((id, target.provider.clone()));
            }
        }
    }
    data.extend(
        pinned
            .into_iter()
            .map(|(id, provider)| json!({"id": id, "object": "model", "owned_by": provider})),
    );

    // group-slug/model ids (ADR-0017 addendum): a group address is the deduped
    // union of the models its member providers serve. owned_by names the group.
    // a member with an explicit upstream rewrite exposes that rewritten model.
    let mut grouped: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for (slug, group) in &snap.groups_by_slug {
        for member in &group.members {
            let models: Vec<String> = match &member.model {
                Some(m) => vec![m.clone()],
                None => provider_models
                    .get(member.provider.as_str())
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default(),
            };
            for model in models {
                let id = format!("{slug}/{model}");
                if vk.as_ref().is_none_or(|vk| {
                    vk.model_permitted(&id) && vk.provider_allowed(&member.provider)
                }) {
                    grouped.insert((id, group.name.clone()));
                }
            }
        }
    }
    data.extend(
        grouped
            .into_iter()
            .map(|(id, group)| json!({"id": id, "object": "model", "owned_by": group})),
    );

    Json(json!({"object": "list", "data": data})).into_response()
}

/// Whether a virtual key can reach at least one target of this route. An empty
/// provider list is deliberately permissive for existing keys and configs.
pub(crate) fn key_allows_route(key: &KeyMeta, entry: &crate::state::RouteEntry) -> bool {
    // the owner's access profile can deny a whole named route regardless of
    // which providers sit behind it (#791)
    key.route_permitted(&entry.route.model)
        && entry
            .route
            .targets
            .iter()
            .chain(
                entry
                    .route
                    .variants
                    .iter()
                    .flat_map(|variant| variant.targets.iter()),
            )
            .any(|target| key.provider_allowed(&target.provider))
}

/// Enforce the part of model visibility that is available on the gateway
/// request path. User restrictions remain a control-plane authorization
/// concern because a virtual key intentionally carries no user identity.
pub(crate) fn model_visible_to(key: Option<&KeyMeta>, entry: &crate::state::RouteEntry) -> bool {
    let visibility = &entry.route.advanced.visibility;
    if visibility.allowed_team_ids.is_empty()
        && visibility.allowed_key_ids.is_empty()
        && visibility.allowed_user_ids.is_empty()
    {
        return true;
    }
    let Some(key) = key else {
        return false;
    };
    visibility
        .allowed_team_ids
        .iter()
        .any(|id| id == &key.team_id)
        || visibility.allowed_key_ids.iter().any(|id| id == &key.id)
}

pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(state, headers, body, "/v1/chat/completions").await
}

pub async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(state, headers, body, "/v1/completions").await
}

/// OpenAI Responses API. This intentionally shares the JSON proxy pipeline so
/// provider-native fields and SSE events remain byte-for-byte passthrough.
pub async fn responses(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    proxy(state, headers, body, "/v1/responses").await
}

pub async fn retrieve_response(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(state, headers, path, uri, LifecycleOperation::Retrieve).await
}

pub async fn delete_response(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(state, headers, path, uri, LifecycleOperation::Delete).await
}

pub async fn cancel_response(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(state, headers, path, uri, LifecycleOperation::Cancel).await
}

pub async fn response_input_items(
    state: State<AppState>,
    headers: HeaderMap,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(state, headers, path, uri, LifecycleOperation::InputItems).await
}

#[derive(Clone, Copy)]
enum LifecycleOperation {
    Retrieve,
    Delete,
    Cancel,
    InputItems,
}

impl LifecycleOperation {
    fn supported(self, caps: crate::response_registry::LifecycleCapabilities) -> bool {
        match self {
            Self::Retrieve => caps.retrieve,
            Self::Delete => caps.delete,
            Self::Cancel => caps.cancel,
            Self::InputItems => caps.input_items,
        }
    }

    fn method(self) -> Method {
        match self {
            Self::Retrieve | Self::InputItems => Method::GET,
            Self::Delete => Method::DELETE,
            Self::Cancel => Method::POST,
        }
    }

    fn path(self, response_id: &str) -> String {
        match self {
            Self::Retrieve | Self::Delete => format!("/v1/responses/{response_id}"),
            Self::Cancel => format!("/v1/responses/{response_id}/cancel"),
            Self::InputItems => format!("/v1/responses/{response_id}/input_items"),
        }
    }
}

async fn response_lifecycle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(response_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    operation: LifecycleOperation,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let snap = state.snapshot.load();
    let vk = match authenticate(&state, &snap, &headers, uri.path()) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    let tenant = tenant_scope(vk.as_ref());
    let Some(route) = state.response_registry.get(&tenant, &response_id) else {
        return response_not_found();
    };
    if !operation.supported(route.capabilities) {
        return crate::error::ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "response lifecycle operation is not supported by the originating provider contract",
        )
        .with_code("response_lifecycle_unsupported")
        .into_response();
    }
    let Some(provider) = snap.providers.get(&route.provider) else {
        return response_not_found();
    };
    if provider.kind != rolter_core::ProviderKind::Openai {
        return response_not_found();
    }
    let resolved_keys = provider.resolve_api_keys();
    let api_key = match &route.provider_key_fingerprint {
        Some(expected) => resolved_keys
            .into_iter()
            .map(|(key, _)| key)
            .find(|key| provider_key_fingerprint(key) == *expected),
        None if resolved_keys.is_empty() => None,
        None => return response_not_found(),
    };
    if route.provider_key_fingerprint.is_some() && api_key.is_none() {
        return response_not_found();
    }
    let mut upstream_path = operation.path(&route.provider_native_id);
    if let Some(query) = uri.query() {
        upstream_path.push('?');
        upstream_path.push_str(query);
    }
    // trace context plus any client headers the operator allowlisted (#564)
    let trace_ctx =
        crate::trace::outbound_headers(&headers, &state.forwarder.forwarded_header_names());
    let trace_headers: Vec<(&str, &str)> = trace_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    match state
        .forwarder
        .forward_resource(
            provider,
            operation.method(),
            &upstream_path,
            api_key.as_deref(),
            &trace_headers,
        )
        .await
    {
        Ok(response) => {
            if matches!(operation, LifecycleOperation::Delete) && response.status().is_success() {
                state.response_registry.remove(&tenant, &response_id);
            }
            lifecycle_response(response, &route)
        }
        Err(err) => upstream_error_response(&err.to_string()),
    }
}

fn lifecycle_response(
    response: reqwest::Response,
    route: &crate::response_registry::ResponseRoute,
) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut upstream_headers = response.headers().clone();
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "content-length",
    ] {
        upstream_headers.remove(name);
    }
    if !upstream_headers.contains_key(header::CONTENT_TYPE) {
        upstream_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    let body = Body::from_stream(response.bytes_stream());
    let mut builder = Response::builder().status(status);
    if let Some(headers) = builder.headers_mut() {
        *headers = upstream_headers;
        for (name, value) in [
            ("x-rolter-provider", route.provider.as_str()),
            ("x-rolter-target", route.target.as_str()),
            ("x-rolter-model", route.model.as_str()),
        ] {
            if let Ok(value) = HeaderValue::from_str(value) {
                headers.insert(name, value);
            }
        }
    }
    builder.body(body).unwrap_or_else(|_| {
        error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build response",
        )
    })
}

fn response_not_found() -> Response {
    crate::error::ApiError::new(StatusCode::NOT_FOUND, "response not found")
        .with_code("response_not_found")
        .with_param("response_id")
        .into_response()
}

/// Lifecycle extensions not covered by ROL-264 remain uniformly unsupported.
pub async fn unsupported_response_lifecycle(
    State(state): State<AppState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(_response_id): Path<String>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let snap = state.snapshot.load();
    if let Err(resp) = authenticate(&state, &snap, &headers, uri.path()) {
        return resp;
    }
    crate::error::ApiError::new(
        StatusCode::NOT_IMPLEMENTED,
        "response lifecycle operations are not supported; Rolter does not route model-less response identifiers",
    )
    .with_code("response_lifecycle_unsupported")
    .into_response()
}

fn tenant_scope(vk: Option<&KeyMeta>) -> String {
    vk.map(|key| key.tenant_key.clone())
        .unwrap_or_else(|| "anonymous".to_string())
}

fn provider_key_fingerprint(key: &str) -> String {
    rolter_auth::hex::encode(&Sha256::digest(key.as_bytes()))
}

pub async fn messages(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    proxy(state, headers, body, "/v1/messages").await
}

pub async fn embeddings(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(state, headers, body, "/v1/embeddings").await
}

pub async fn rerank(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    proxy(state, headers, body, "/v1/rerank").await
}

pub async fn images_generations(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(state, headers, body, "/v1/images/generations").await
}

pub async fn audio_speech(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(state, headers, body, "/v1/audio/speech").await
}

pub async fn audio_transcriptions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_multipart(state, headers, body, "/v1/audio/transcriptions").await
}

pub async fn audio_translations(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy_multipart(state, headers, body, "/v1/audio/translations").await
}

/// Rewrite the plain-text `413` that axum's `DefaultBodyLimit` returns into the
/// OpenAI-style json error the rest of the gateway emits. Runs on every request
/// but only rewrites responses that are actually `413 Payload Too Large`, so the
/// steady-state cost is a single status comparison.
pub async fn map_payload_too_large(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let resp = next.run(req).await;
    if resp.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return crate::error::ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body exceeds the configured max_body_bytes limit",
        )
        .with_code("request_too_large")
        .into_response();
    }
    resp
}

/// Enforce the deployment's required ingress headers (#1162).
///
/// This runs ahead of routing and ahead of authentication, because that is
/// what the setting means: a request that does not carry the operator's mesh
/// identity header, WAF marker or tenant tag never reaches the data plane at
/// all. Health and readiness are exempt — a probe is the deployment's own
/// traffic, and locking the orchestrator out of `/healthz` would take the
/// gateway down rather than protect it.
///
/// The refusal names the header and never its expected value: an operator may
/// well have put a shared secret there, and echoing it back would hand it to
/// exactly the caller who did not know it.
pub async fn enforce_required_headers(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let snap = state.snapshot.load();
    if !snap.security.required_headers.is_empty() && !is_probe_path(req.uri().path()) {
        let headers = req.headers();
        let missing = snap.security.missing_required_header(|name| {
            headers.get(name).and_then(|value| value.to_str().ok())
        });
        if let Some(name) = missing {
            state.metrics.auth_failures_total.fetch_add(1, Relaxed);
            return crate::error::ApiError::new(
                StatusCode::FORBIDDEN,
                format!("request is missing the required header `{name}`"),
            )
            .with_code("required_header_missing")
            .into_response();
        }
    }
    next.run(req).await
}

/// Paths the deployment itself calls: never subject to the ingress filter.
fn is_probe_path(path: &str) -> bool {
    matches!(path, "/healthz" | "/readyz")
}

fn extract_key(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = value.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }
    if let Some(value) = headers.get("x-api-key") {
        if let Ok(s) = value.to_str() {
            return Some(s.to_string());
        }
    }
    None
}

/// OpenAI-style error response with the `type` inferred from `status`. Kept as
/// the single choke point so every error path stays wire-compatible; callers
/// that want a `code`/`param` build [`crate::error::ApiError`] directly.
fn error_json(status: StatusCode, message: &str) -> Response {
    crate::error::ApiError::new(status, message).into_response()
}

/// Tenant identity forwarded to a guardrail webhook or plugin as metadata.
/// Shared by both call sites so the envelope always carries the same shape.
fn plugin_tenant(scope: &ScopeIds) -> rolter_core::WebhookTenant {
    rolter_core::WebhookTenant {
        org: (!scope.org.is_empty()).then(|| scope.org.clone()),
        team: (!scope.team.is_empty()).then(|| scope.team.clone()),
        project: (!scope.project.is_empty()).then(|| scope.project.clone()),
        key: (!scope.key.is_empty()).then(|| scope.key.clone()),
    }
}

/// Preserve a queue-admission failure as a client-actionable overload response
/// instead of flattening it into a generic upstream 502 after failover is
/// exhausted. Other forwarding failures keep their existing gateway-error form.
fn upstream_error_response(message: &str) -> Response {
    if let Some((code, message)) = translation_request_error(message) {
        return crate::error::ApiError::new(StatusCode::BAD_REQUEST, message)
            .with_code(code)
            .with_param("messages")
            .into_response();
    }
    if let Some(message) = queue_admission_message(message) {
        let (status, code) = if message.contains("dropped") {
            (StatusCode::SERVICE_UNAVAILABLE, "queue_dropped")
        } else if message.contains("timed out") {
            (StatusCode::TOO_MANY_REQUESTS, "queue_timeout")
        } else {
            (StatusCode::TOO_MANY_REQUESTS, "queue_full")
        };
        return crate::error::ApiError::new(status, message)
            .with_code(code)
            .into_response();
    }
    error_json(StatusCode::BAD_GATEWAY, message)
}

/// Request-translation failures the caller can fix, paired with the OpenAI-style
/// error `code` they render as. Every one of these is a property of the body the
/// client sent, so it is a 400 — flattening them into the generic upstream 502
/// tells an operator to look at the provider for a problem in their own request.
const TRANSLATION_REQUEST_ERRORS: &[(&str, &str)] = &[
    ("role_capability: ", "role_capability_unsupported"),
    ("unsupported_content_part: ", "unsupported_content_part"),
    (
        "gemini_interactions_unsupported: ",
        "gemini_interactions_unsupported",
    ),
];

/// Match a forwarding failure against [`TRANSLATION_REQUEST_ERRORS`], returning
/// its error code and the message with the `Error::Config` wrapping stripped.
fn translation_request_error(message: &str) -> Option<(&'static str, &str)> {
    let bare = message.strip_prefix("config error: ")?;
    TRANSLATION_REQUEST_ERRORS
        .iter()
        .find_map(|(prefix, code)| bare.strip_prefix(prefix).map(|rest| (*code, rest)))
}

/// Recognize a queue-admission failure and hand back its bare message.
/// `Error::Upstream` renders as `upstream error: <message>`, so the forwarding
/// loop sees the wrapped form; matching on the raw prefix alone silently
/// downgraded every shed request to a generic 502 (#639).
fn queue_admission_message(message: &str) -> Option<&str> {
    let bare = message.strip_prefix("upstream error: ").unwrap_or(message);
    bare.starts_with("provider queue").then_some(bare)
}

fn is_queue_admission_error(message: &str) -> bool {
    queue_admission_message(message).is_some()
}

/// The upstream vendor a presented credential looks like, when it is plainly
/// not one of ours.
///
/// rolter mints `sk-rolter-…`. Every prefix below belongs to a *provider*, and
/// a caller who sends one has confused the two credentials this product has:
/// the virtual key rolter gives **you** for the gateway, and the provider key a
/// vendor gives **rolter** for the upstream. That mistake used to render as a
/// bare `invalid api key`, indistinguishable from a revoked key or a typo, and
/// it was the single most common confusion during dogfooding (#943).
///
/// Prefix matching only, on a credential the caller already holds, so this
/// leaks nothing: it says something about the string that was sent, never about
/// what is stored. Order matters — the longer, more specific prefixes are
/// checked before the bare `sk-` fallback.
fn provider_key_vendor(key: &str) -> Option<&'static str> {
    // ours, so never a mistake
    if key.starts_with(ROLTER_KEY_PREFIX) {
        return None;
    }
    const VENDORS: &[(&str, &str)] = &[
        ("sk-ant-", "Anthropic"),
        ("sk-or-", "OpenRouter"),
        ("sk-proj-", "OpenAI"),
        ("AIza", "Google AI Studio"),
        ("gsk_", "Groq"),
        ("xai-", "xAI"),
        ("fw_", "Fireworks"),
        ("hf_", "Hugging Face"),
        ("r8_", "Replicate"),
        ("csk-", "Cerebras"),
        ("nvapi-", "NVIDIA"),
        ("pplx-", "Perplexity"),
    ];
    for (prefix, vendor) in VENDORS {
        if key.starts_with(prefix) {
            return Some(vendor);
        }
    }
    // every remaining `sk-` is some openai-compatible vendor's key; naming the
    // shape is still more useful than saying nothing
    key.starts_with("sk-").then_some("provider")
}

/// The prefix every rolter virtual key carries, from
/// `rolter_control::crud::generate_virtual_key`.
const ROLTER_KEY_PREFIX: &str = "sk-rolter-";

/// Sent when no credential arrived at all. Names the credential rather than
/// saying "api key", which is the word the product uses for three different
/// things (#943).
const MISSING_KEY_MESSAGE: &str =
    "missing api key: send a rolter virtual key as `Authorization: Bearer sk-rolter-…`";

/// The rejection for a key that did not match, naming the mistake when the
/// string is recognisably an upstream credential.
fn invalid_key_message(key: &str) -> String {
    match provider_key_vendor(key) {
        Some("provider") => format!(
            "invalid api key: this looks like a provider credential. the gateway expects a \
             rolter virtual key ({ROLTER_KEY_PREFIX}…), minted on the dashboard's Virtual Keys \
             screen; provider keys are configured on the provider and are never sent by clients"
        ),
        Some(vendor) => {
            let article = if vendor.starts_with(['A', 'E', 'I', 'O', 'U']) {
                "an"
            } else {
                "a"
            };
            format!(
            "invalid api key: this looks like {article} {vendor} provider key. the gateway expects a \
             rolter virtual key ({ROLTER_KEY_PREFIX}…), minted on the dashboard's Virtual Keys \
             screen; provider keys are configured on the provider and are never sent by clients"
            )
        }
        None => "invalid api key".to_string(),
    }
}

/// Shared virtual-key auth check for every `/v1/*` handler. Returns the
/// matched key (or `None` when no keys are configured, i.e. auth disabled).
#[allow(clippy::result_large_err)]
pub(crate) fn authenticate(
    state: &AppState,
    snap: &Snapshot,
    headers: &HeaderMap,
    path: &str,
) -> Result<Option<KeyMeta>, Response> {
    // an operator-named unauthenticated path (#1162). exact match only, and
    // checked before anything else, because the point of the list is that
    // these paths answer without a key at all — a health integration or a
    // model catalogue a sidecar reads. no key is looked up, so nothing is
    // attributed to one either
    if snap.security.bypasses_auth(path) {
        return Ok(None);
    }
    if snap.keys.is_empty() {
        // an empty key set is only "auth disabled" for unmanaged local dev.
        // on a managed gateway it means every key was revoked (or none exists
        // yet), which must lock the data plane down rather than open it
        // the config file wins in either direction when it says anything:
        // it is a deliberate local override of a database the operator may
        // not even have. otherwise the Security screen's toggle decides,
        // falling back to what the deployment mode implies
        let required = snap
            .require_auth
            .unwrap_or(snap.security.virtual_key_required || state.managed_auth);
        if required {
            state.metrics.auth_failures_total.fetch_add(1, Relaxed);
            return Err(error_json(StatusCode::UNAUTHORIZED, MISSING_KEY_MESSAGE));
        }
        return Ok(None);
    }
    match extract_key(headers) {
        Some(key) => {
            // look up by peppered digest so the plaintext key is never used as a
            // map key and never lingers in memory beyond this call
            let digest = rolter_auth::hash_key(&snap.pepper, &key);
            match snap.keys.get(&digest) {
                Some(vk) if vk.is_active(Utc::now()) => Ok(Some(vk.clone())),
                // matched but revoked or expired, and a key that matched
                // nothing: both answer the same way, so a caller cannot probe
                // which of the two happened
                Some(_) | None => {
                    state.metrics.auth_failures_total.fetch_add(1, Relaxed);
                    Err(error_json(
                        StatusCode::UNAUTHORIZED,
                        &invalid_key_message(&key),
                    ))
                }
            }
        }
        None => {
            state.metrics.auth_failures_total.fetch_add(1, Relaxed);
            Err(error_json(StatusCode::UNAUTHORIZED, MISSING_KEY_MESSAGE))
        }
    }
}

/// Shared proxy pipeline: parse, authenticate, balance, forward, stream back.
async fn proxy(state: AppState, headers: HeaderMap, body: Bytes, path: &str) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let started = Instant::now();

    let mut parsed: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "invalid json body")
                .with_code("invalid_json")
                .into_response()
        }
    };
    let model = match parsed.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "missing model field")
                .with_code("missing_required_parameter")
                .with_param("model")
                .into_response()
        }
    };

    let snap = state.snapshot.load();

    let vk = {
        // the span is held, not entered: `authenticate` opens no child spans, and
        // an entered guard is `!Send`, which would make the whole handler future
        // `!Send`. a held span still parents to the request span and still times
        // the stage from creation to drop
        let _stage = crate::trace::stage_span!("auth");
        match authenticate(&state, &snap, &headers, path) {
            Ok(vk) => vk,
            Err(resp) => return resp,
        }
    };
    if let Some(vk) = &vk {
        if !vk.model_permitted(&model) {
            return crate::error::ApiError::new(
                StatusCode::FORBIDDEN,
                "model not allowed for this key",
            )
            .with_code("model_not_allowed")
            .with_param("model")
            .into_response();
        }
    }

    // scope identity drives both budget enforcement and log attribution
    let scope = vk
        .as_ref()
        .map(|v| ScopeIds {
            org: v.org_id.clone(),
            team: v.team_id.clone(),
            project: v.project_id.clone(),
            key: v.id.clone(),
            business_unit: v.business_unit_id.clone(),
            customer: v.customer_id.clone(),
        })
        .unwrap_or_default();

    // block before spending upstream tokens when any applicable budget is spent
    if let Some(exceeded) = state.budgets.exceeded(&snap.budgets, &scope).await {
        state.metrics.budget_blocks_total.fetch_add(1, Relaxed);
        return crate::error::ApiError::new(
            StatusCode::PAYMENT_REQUIRED,
            format!(
                "budget exceeded for {:?} '{}' (limit ${:.2})",
                exceeded.scope, exceeded.id, exceeded.limit_usd
            ),
        )
        .with_code("insufficient_quota")
        .into_response();
    }

    // pre_route plugins: consult before the route is resolved, while only the
    // key's scope and raw request are known. Block rejects; transform rewrites
    // `parsed` in place. Org/project-scoped, ordered by position (#509)
    let trace_id = headers
        .get(crate::trace::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let pre_route_plugins = snap.plugins.for_stage(
        rolter_core::PluginStage::PreRoute,
        &scope.org,
        (!scope.project.is_empty()).then_some(scope.project.as_str()),
    );
    let mut pre_route_transformed = false;
    if !pre_route_plugins.is_empty() {
        let tenant = plugin_tenant(&scope);
        match crate::plugin_dispatch::dispatch(
            &pre_route_plugins,
            rolter_core::PluginStage::PreRoute,
            &state.metrics,
            &model,
            "", // no route resolved yet at this stage
            trace_id,
            &tenant,
            &parsed,
        )
        .await
        {
            crate::plugin_dispatch::DispatchOutcome::Allow(content) => {
                pre_route_transformed = content != parsed;
                parsed = content;
            }
            crate::plugin_dispatch::DispatchOutcome::Block(reason) => {
                return crate::error::ApiError::new(
                    StatusCode::BAD_REQUEST,
                    reason.unwrap_or_else(|| "request blocked by plugin".to_string()),
                )
                .with_code("plugin_blocked")
                .into_response();
            }
        }
    }

    // throughput cap: reject before forwarding when a matching request/token
    // window is already at capacity (admission also counts the request)
    if let Some(hit) = state.rate_limiter.check(&snap.rate_limits, &scope).await {
        state.metrics.rate_limit_blocks_total.fetch_add(1, Relaxed);
        let mut resp = crate::error::ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "{} rate limit exceeded for {:?} '{}' (limit {}/min)",
                hit.kind, hit.scope, hit.id, hit.limit
            ),
        )
        .with_code("rate_limit_exceeded")
        .into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(hit.retry_after));
        return resp;
    }

    // built-in fake-llm answers locally unless a configured route shadows it
    if model == fake_llm::MODEL_NAME && !snap.routes.contains_key(&model) {
        return match path {
            "/v1/chat/completions" => fake_llm::chat_completions(&parsed),
            "/v1/responses" => fake_llm::responses(&parsed),
            "/v1/messages" => fake_llm::messages(&parsed),
            "/v1/embeddings" => fake_llm::embeddings(&parsed),
            "/v1/rerank" => fake_llm::rerank(&parsed),
            "/v1/images/generations" => fake_llm::images(&parsed),
            "/v1/audio/speech" => fake_llm::speech(&parsed),
            _ => error_json(
                StatusCode::NOT_FOUND,
                &format!("'{model}' is not served on {path}"),
            ),
        };
    }

    // route-name-first: a named route always wins, even one whose name
    // contains '/'. only on a miss do we try `provider-slug/model` addressing
    // (ADR-0017), which pins a provider and forwards `model` as the upstream
    // model through the same classic-pool machinery (owned entry held here).
    //
    // route resolution through the key's access checks is one attributable
    // stage; `strategy` and `candidates` are recorded once the route is known
    let route_stage = crate::trace::stage_span!(
        "route.select",
        route = tracing::field::Empty,
        strategy = tracing::field::Empty,
        candidates = tracing::field::Empty
    );
    let pinned = if snap.routes.contains_key(&model) {
        None
    } else {
        snap.resolve_pinned(&model)
    };
    let mut entry = match snap.routes.get(&model).or(pinned.as_ref()) {
        Some(entry) => entry,
        None => {
            return crate::error::ApiError::new(
                StatusCode::NOT_FOUND,
                format!("no route for model '{model}'"),
            )
            .with_code("model_not_found")
            .with_param("model")
            .into_response()
        }
    };
    // The policy consumes only the request's already-bounded byte count. Its
    // ordered tier selection never retains prompt text. If an operator removes
    // a tier route or a key cannot access it, preserve the requested route and
    // expose that safe fallback in bounded telemetry.
    if let Some((tier, tier_route)) =
        rolter_balancer::complexity::ComplexityPolicy::from_params(&entry.route.params)
            .ok()
            .flatten()
            .and_then(|policy| policy.select(body.len()).cloned())
            .map(|tier| (tier.name, tier.route))
    {
        let selected = snap.routes.get(&tier_route).filter(|candidate| {
            (!candidate.route.targets.is_empty() || candidate.route.has_variants())
                && vk
                    .as_ref()
                    .is_none_or(|key| key_allows_route(key, candidate))
                && model_visible_to(vk.as_ref(), candidate)
        });
        let fallback = selected.is_none();
        if let Some(candidate) = selected {
            entry = candidate;
        }
        state
            .metrics
            .observe_complexity(&model, &tier, &entry.route.model, fallback);
    }
    if entry.route.targets.is_empty() && !entry.route.has_variants() {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "route has no targets");
    }
    if !model_visible_to(vk.as_ref(), entry) {
        return crate::error::ApiError::new(
            StatusCode::FORBIDDEN,
            "model is not visible to this key",
        )
        .with_code("model_not_allowed")
        .with_param("model")
        .into_response();
    }
    if let Some(key) = &vk {
        // checked before the composite gate so the caller is told which of the
        // two denied them: the route policy their owner holds, or the key's
        // own provider allow-list (#791)
        if !key.route_permitted(&entry.route.model) {
            return crate::error::ApiError::new(
                StatusCode::FORBIDDEN,
                "route is not allowed for this key's access profile",
            )
            .with_code("route_not_allowed")
            .with_param("model")
            .into_response();
        }
        if !key_allows_route(key, entry) {
            return crate::error::ApiError::new(
                StatusCode::FORBIDDEN,
                "no provider on this route is allowed for this key",
            )
            .with_code("provider_not_allowed")
            .with_param("model")
            .into_response();
        }
    }
    // the route survived every access check: record what was chosen and close
    // the stage, so what follows is not nested under route selection
    route_stage.record("route", entry.route.model.as_str());
    route_stage.record("strategy", tracing::field::debug(&entry.route.strategy));
    route_stage.record("candidates", entry.route.targets.len());
    drop(route_stage);

    if let Some(max_output) = entry.route.advanced.limits.output_tokens {
        for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
            if parsed
                .get(field)
                .and_then(Value::as_u64)
                .is_some_and(|value| value > u64::from(max_output))
            {
                return crate::error::ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("{field} exceeds this model's output token limit of {max_output}"),
                )
                .with_code("max_tokens_exceeded")
                .with_param(field)
                .into_response();
            }
        }
    }

    // versioned prompt templates: render the immutable versions active for this
    // route and inject their decorator messages around the caller's own messages.
    // variable substitution is structural (values land as inert JSON strings);
    // an unknown/missing/oversized variable rejects with a client error. runs
    // before guardrails so any appended user/assistant content is still scanned
    // (ROL-256).
    let mut template_applied = false;
    let template_scope = rolter_core::PromptTemplateRequestScope {
        route_model: &entry.route.model,
        org_id: &scope.org,
        project_id: &scope.project,
        virtual_key_id: &scope.key,
    };
    if snap.prompt_templates.active_for_scope(&template_scope) {
        match crate::prompt_templates::apply_for_scope(
            &snap.prompt_templates,
            &template_scope,
            path,
            &mut parsed,
        ) {
            Ok(report) => {
                if report.decorations > 0 {
                    template_applied = true;
                    state
                        .metrics
                        .prompt_template_decorations_total
                        .fetch_add(report.decorations as u64, Relaxed);
                }
            }
            Err(err) => {
                state
                    .metrics
                    .prompt_template_rejections_total
                    .fetch_add(1, Relaxed);
                return crate::error::ApiError::new(StatusCode::BAD_REQUEST, err.message())
                    .with_code("invalid_prompt_template")
                    .into_response();
            }
        }
    }

    // both the built-in scan and the webhook consult are pre-call guardrail work,
    // so they share one stage: a request slowed by a sluggish guardrail service
    // shows up here rather than inside the upstream leg. never carries the
    // matched content — only whether a rule fired
    let guardrails_pre_stage = crate::trace::stage_span!(
        "guardrails.pre",
        redacted = tracing::field::Empty,
        webhook = snap.guardrail_webhook.enabled
    );

    // built-in guardrails: scan request content before proxying. a block rule
    // rejects with an OpenAI-compatible error; redactions rewrite `parsed` in
    // place and force the re-serialized forward path below. never logs raw
    // matches — only rule-name/count counters (ROL-261).
    let mut guardrail_redacted = false;
    // the route's override decides which rules apply here; a route that
    // disables every rule skips the JSON walk entirely (#590)
    if snap.guardrails.pre_call_active_for(&entry.guardrails) {
        match crate::guardrails::apply_input(&snap.guardrails, &entry.guardrails, path, &mut parsed)
        {
            Ok(report) => {
                if report.redactions > 0 {
                    guardrail_redacted = true;
                    state
                        .metrics
                        .guardrail_redactions_total
                        .fetch_add(report.redactions as u64, Relaxed);
                }
            }
            Err(rule) => {
                state.metrics.guardrail_blocks_total.fetch_add(1, Relaxed);
                return crate::error::ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("request blocked by guardrail '{rule}'"),
                )
                .with_code("guardrail_blocked")
                .into_response();
            }
        }
    }

    // custom guardrail webhook: consult a self-hosted service before proxying.
    // block -> OpenAI-compatible error; transform -> replace the request body;
    // transport failures resolve per the operator's fail-open/closed choice. only
    // the assembled envelope is sent; prompt content is never logged here (ROL-257).
    let mut webhook_transformed = false;
    if snap.guardrail_webhook.enabled {
        let tenant = plugin_tenant(&scope);
        match crate::guardrail_webhook::consult_pre_call(
            &snap.guardrail_webhook,
            &state.metrics,
            &model,
            &entry.route.model,
            trace_id,
            tenant,
            &parsed,
        )
        .await
        {
            crate::guardrail_webhook::WebhookOutcome::Allow => {}
            crate::guardrail_webhook::WebhookOutcome::Block(reason) => {
                return crate::error::ApiError::new(
                    StatusCode::BAD_REQUEST,
                    reason.unwrap_or_else(|| "request blocked by guardrail service".to_string()),
                )
                .with_code("guardrail_blocked")
                .into_response();
            }
            crate::guardrail_webhook::WebhookOutcome::Transform(content) => {
                parsed = content;
                webhook_transformed = true;
            }
        }
    }

    // external PII sanitizer: unlike the webhook above, this returns transformed
    // *content* — entities replaced by deterministic placeholders — plus an
    // opaque token the same service accepts back to reverse the transform. the
    // mapping never enters this process, so there is nothing here to leak into
    // a log, metric or span (#848)
    let mut pii_sanitized = false;
    let mut pii_ticket: Option<rolter_core::RestorationTicket> = None;
    let pii_scope = rolter_core::TokenScope {
        org: scope.org.clone(),
        team: scope.team.clone(),
        project: scope.project.clone(),
        route: entry.route.model.clone(),
    };
    // asking the caller before deciding, because the policy — not the caller —
    // is what decides. under `never` a caller-set header changes nothing
    let restore_requested = headers
        .get(rolter_core::pii_sanitizer::RESTORE_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let will_restore = snap
        .pii_sanitizer
        .restoration
        .should_restore(restore_requested);
    if snap.pii_sanitizer.sanitizes_request() {
        match crate::pii_sanitizer::sanitize(
            &snap.pii_sanitizer,
            &state.metrics,
            "request",
            &model,
            &entry.route.model,
            trace_id,
            plugin_tenant(&scope),
            pii_scope.clone(),
            will_restore,
            &parsed,
        )
        .await
        {
            crate::pii_sanitizer::SanitizeOutcome::Unchanged => {}
            crate::pii_sanitizer::SanitizeOutcome::Sanitized {
                content,
                ticket,
                replaced,
            } => {
                // `replaced > 0` short-circuits the deep compare for the case
                // that matters; a service that rewrote the body while reporting
                // nothing is still honoured, it just costs a comparison
                pii_sanitized = replaced > 0 || content != parsed;
                parsed = content;
                pii_ticket = ticket;
            }
            crate::pii_sanitizer::SanitizeOutcome::Block(reason) => {
                return crate::error::ApiError::new(StatusCode::BAD_GATEWAY, reason)
                    .with_code("pii_sanitizer_unavailable")
                    .into_response();
            }
        }
    }

    // captured here because `model`, `trace_id` and `scope` are moved into the
    // spend recorder and log row before the response leg runs
    let pii_model = model.clone();
    let pii_trace_id = trace_id.to_string();
    let pii_tenant = plugin_tenant(&scope);
    let pii_route = entry.route.model.clone();

    // pre_upstream plugins: consult after the route is resolved, before
    // forwarding. Same block/transform/fail-open-or-closed contract as
    // pre_route, run in the same stage as the guardrail webhook above so a
    // slow plugin shows up here rather than inside the upstream leg (#509)
    let mut plugin_transformed = false;
    let pre_upstream_plugins = snap.plugins.for_stage(
        rolter_core::PluginStage::PreUpstream,
        &scope.org,
        (!scope.project.is_empty()).then_some(scope.project.as_str()),
    );
    if !pre_upstream_plugins.is_empty() {
        let tenant = plugin_tenant(&scope);
        match crate::plugin_dispatch::dispatch(
            &pre_upstream_plugins,
            rolter_core::PluginStage::PreUpstream,
            &state.metrics,
            &model,
            &entry.route.model,
            trace_id,
            &tenant,
            &parsed,
        )
        .await
        {
            crate::plugin_dispatch::DispatchOutcome::Allow(content) => {
                plugin_transformed = content != parsed;
                parsed = content;
            }
            crate::plugin_dispatch::DispatchOutcome::Block(reason) => {
                return crate::error::ApiError::new(
                    StatusCode::BAD_REQUEST,
                    reason.unwrap_or_else(|| "request blocked by plugin".to_string()),
                )
                .with_code("plugin_blocked")
                .into_response();
            }
        }
    }
    guardrails_pre_stage.record(
        "redacted",
        guardrail_redacted
            || webhook_transformed
            || plugin_transformed
            || pre_route_transformed
            || pii_sanitized,
    );
    drop(guardrails_pre_stage);

    // inject the admin's per-model param defaults (temperature, max_tokens, ...)
    // before forwarding; caller values survive only where the override policy
    // permits. falls back to the untouched body if re-serialization fails
    let effective_model = entry.route.model.clone();
    let rewrite_model = effective_model != model;
    let has_forward_params = entry.route.params.keys().any(|key| key != POLICY_PARAM);
    let forward_body = if !has_forward_params
        && !rewrite_model
        && !guardrail_redacted
        && !webhook_transformed
        && !plugin_transformed
        && !pre_route_transformed
        && !template_applied
        && !pii_sanitized
    {
        body.clone()
    } else {
        let mut injected = parsed.clone();
        if rewrite_model {
            if let Some(object) = injected.as_object_mut() {
                object.insert("model".to_string(), Value::String(effective_model.clone()));
            }
        }
        entry.route.apply_params(&mut injected);
        if let Some(object) = injected.as_object_mut() {
            object.remove(POLICY_PARAM);
        }
        serde_json::to_vec(&injected)
            .map(Bytes::from)
            .unwrap_or_else(|_| body.clone())
    };

    // capture log fields independent of the chosen target
    let stream = parsed
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    // post-call rules mask the response body, which means buffering it in full. a
    // streamed response cannot have both, so the operator's `streaming_post_call`
    // choice decides: refuse the request (the default) or serve the stream with
    // output rules not applied. read after the webhook stage so it judges the body
    // that will actually be sent upstream (#591)
    let output_guard = crate::guardrails::OutputGuard::resolve(
        &snap.guardrails,
        &entry.guardrails,
        &state.metrics,
        path,
    );
    if output_guard.is_some()
        && stream
        && snap.guardrails.streaming_post_call() == rolter_core::StreamingPostCall::Reject
    {
        state
            .metrics
            .guardrail_stream_rejections_total
            .fetch_add(1, Relaxed);
        return crate::error::ApiError::new(
            StatusCode::BAD_REQUEST,
            "this model applies output guardrails, which cannot be enforced on a streamed \
             response; retry without \"stream\": true",
        )
        .with_code("guardrail_streaming_unsupported")
        .with_param("stream")
        .into_response();
    }
    // a stream that got here is one the operator opted out of masking for
    let output_guard = if stream { None } else { output_guard };

    // the sanitizer's response leg needs the whole body for the same reason the
    // output guardrails do: an entity — or a placeholder — can straddle any SSE
    // chunk boundary, and neither detection nor reversal is correct on a
    // fragment. same trade-off, same resolution as `streaming_post_call` (#848)
    let pii_touches_response = snap.pii_sanitizer.touches_response(restore_requested);
    if pii_touches_response
        && stream
        && snap.pii_sanitizer.streaming == rolter_core::StreamingResponse::Reject
    {
        state
            .metrics
            .pii_stream_rejections_total
            .fetch_add(1, Relaxed);
        return crate::error::ApiError::new(
            StatusCode::BAD_REQUEST,
            "this deployment sanitizes or restores response content, which cannot be applied \
             to a streamed response; retry without \"stream\": true",
        )
        .with_code("pii_streaming_unsupported")
        .with_param("stream")
        .into_response();
    }
    // a stream that got here is one the operator opted out of the response leg
    // for; the request leg still ran, so PII still did not reach the provider
    let pii_leg = (pii_touches_response && !stream).then(|| PiiResponseLeg {
        state: &state,
        config: &snap.pii_sanitizer,
        model: &pii_model,
        route: &pii_route,
        trace_id: &pii_trace_id,
        tenant: pii_tenant.clone(),
        scope: &pii_scope,
        ticket: pii_ticket.as_ref(),
        will_restore,
    });
    // the ensure_request_id middleware guarantees this header is present
    let request_id = headers
        .get(crate::trace::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // adopt the caller's distributed trace when one was propagated inbound
    let trace_id = crate::trace::request_trace_id(&headers);

    // post_response plugins: same buffering constraint as output guardrails
    // above, so they follow the identical streamed-response opt-out (#509)
    let post_response_plugin_list = snap.plugins.for_stage(
        rolter_core::PluginStage::PostResponse,
        &scope.org,
        (!scope.project.is_empty()).then_some(scope.project.as_str()),
    );
    let post_response_plugins = (!stream && !post_response_plugin_list.is_empty()).then(|| {
        crate::plugin_dispatch::PostResponsePlugins {
            plugins: post_response_plugin_list,
            metrics: &state.metrics,
            model: model.clone(),
            route: entry.route.model.clone(),
            trace_id: trace_id.clone(),
            tenant: plugin_tenant(&scope),
        }
    });
    // scope identity for log attribution (empty for config-defined keys)
    let (vk_id, org_id, team_id, project_id) = (
        scope.key.clone(),
        scope.org.clone(),
        scope.team.clone(),
        scope.project.clone(),
    );
    // the trace backend needs the same tenant identity the request log carries,
    // so a collector can route this org's spans to this org's destination (#836)
    crate::trace::record_tenant(&org_id, &team_id, &project_id);
    // governance dimensions the key's spend rolls up to (#539)
    let (business_unit_id, customer_id) = key_attribution(vk.as_ref());
    // records this request's tokens against its rate limits once usage is known
    let token_recorder = TokenRecorder::new(
        state.rate_limiter.clone(),
        snap.rate_limits.clone(),
        scope.clone(),
    );
    let price = snap.prices.get(&effective_model).cloned();
    // refuse before the upstream call when the operator will not serve traffic
    // they cannot account for (#974)
    if let Some(refused) =
        unpriced_admission(&state, &snap, &scope, &effective_model, price.is_some())
    {
        return refused;
    }
    // records this request's cost against its budgets once cost_usd is known
    let recorder = SpendRecorder::new(state.budgets.clone(), snap.budgets.clone(), scope);

    let session_key = headers.get("x-session-id").and_then(|v| v.to_str().ok());
    let prompt = std::str::from_utf8(&body).ok();
    let token_ids = parse_vllm_token_ids(&headers);
    let ctx = RouteContext {
        session_key,
        prompt,
        token_ids: token_ids.as_deref(),
        // adapter identity only exists when the request addresses something
        // other than the route's own model — i.e. a passthrough provider-group
        // route, which is how vLLM addresses LoRA adapters over shared base
        // weights. On a single-model route the two are equal and this stays
        // None, which keeps adapter scoring inert: treating one model as an
        // adapter would pin the whole route to whichever target served first
        adapter: (model != effective_model).then_some(model.as_str()),
    };
    // trace context plus any client headers the operator allowlisted (#564).
    //
    // this is the *fallback* set, used when no OTLP pipeline is installed: it
    // copies the caller's trace headers verbatim, as before #805. With a
    // pipeline installed the trace context is re-injected per attempt from
    // inside that attempt's `upstream.request` span, so the provider call
    // parents to the gateway rather than to the caller (see `forwarded_names`
    // below and its use in the retry loop)
    let forwarded_names = state.forwarder.forwarded_header_names();
    let trace_ctx = crate::trace::outbound_headers(&headers, &forwarded_names);
    let trace_headers: Vec<(&str, &str)> = trace_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // exact-match response cache (ROL-56, streaming added in ROL-235): eligible
    // for routes that opted in, when the global switch is on and Redis is wired.
    // both non-streaming JSON and streaming SSE responses are cacheable — a
    // streaming miss is buffered in full, stored, and replayed instantly on a
    // later hit. the key hashes the post-injection forward body so admin param
    // defaults are part of the identity; per-key routes also mix in the vk id.
    // a virtual key may override its route's cache decision (ROL-235 part 2):
    // Some(false) opts the key out, Some(true) opts it in even on a route that
    // didn't; None inherits the route flag. the global switch still gates all
    // exact and semantic lookups are one stage: a cache that is slow to answer
    // is as interesting as one that misses, and both delay the same request
    let cache_stage = crate::trace::stage_span!(
        "cache.lookup",
        hit = tracing::field::Empty,
        kind = tracing::field::Empty
    );
    let cache_ttl = entry.route.cache_ttl_secs(snap.cache.default_ttl_secs);
    let cache_eligible = path != "/v1/responses"
        && vk
            .as_ref()
            .and_then(|k| k.cache_override)
            .unwrap_or_else(|| entry.route.cache_enabled());
    let cache_key = if snap.cache.enabled && state.response_cache.is_enabled() && cache_eligible {
        let scope_seg = if entry.route.cache_per_key() {
            vk_id.as_str()
        } else {
            ""
        };
        Some(ResponseCache::make_key(
            &snap.cache.namespace,
            path,
            scope_seg,
            &forward_body,
        ))
    } else {
        None
    };
    if let Some(key) = &cache_key {
        if let Some(hit) = state.response_cache.get(key).await {
            state.metrics.cache_hits_total.fetch_add(1, Relaxed);
            cache_stage.record("hit", true);
            cache_stage.record("kind", "exact");
            return cached_response(
                hit,
                &state.log,
                CacheHitLog {
                    request_id: request_id.clone(),
                    trace_id: trace_id.clone(),
                    vk_id: vk_id.clone(),
                    org_id: org_id.clone(),
                    team_id: team_id.clone(),
                    project_id: project_id.clone(),
                    business_unit_id: business_unit_id.clone(),
                    customer_id: customer_id.clone(),
                    model: model.clone(),
                    sample_rate: snap.logging.sample_rate,
                    started,
                },
                output_guard.as_ref(),
            );
        }
        state.metrics.cache_misses_total.fetch_add(1, Relaxed);
    }
    let mut semantic_store = None;
    if let (Some(exact_key), Some(semantic)) = (
        cache_key.as_ref(),
        entry.route.cache.as_ref().and_then(|c| c.semantic.as_ref()),
    ) {
        if let Some(text) = semantic_cache_text(&forward_body) {
            if let Some(embedding) = semantic_embedding(&state, &snap, semantic, &text).await {
                let scope_seg = if entry.route.cache_per_key() {
                    vk_id.as_str()
                } else {
                    ""
                };
                let index_key = ResponseCache::semantic_index_key(
                    &snap.cache.namespace,
                    path,
                    &entry.route.model,
                    scope_seg,
                );
                if let Some(hit) = state
                    .response_cache
                    .semantic_get(
                        &index_key,
                        &embedding,
                        semantic.threshold,
                        semantic.max_candidates,
                    )
                    .await
                {
                    state
                        .metrics
                        .semantic_cache_hits_total
                        .fetch_add(1, Relaxed);
                    cache_stage.record("hit", true);
                    cache_stage.record("kind", "semantic");
                    return cached_response(
                        hit,
                        &state.log,
                        CacheHitLog {
                            request_id: request_id.clone(),
                            trace_id: trace_id.clone(),
                            vk_id: vk_id.clone(),
                            org_id: org_id.clone(),
                            team_id: team_id.clone(),
                            project_id: project_id.clone(),
                            business_unit_id: business_unit_id.clone(),
                            customer_id: customer_id.clone(),
                            model: model.clone(),
                            sample_rate: snap.logging.sample_rate,
                            started,
                        },
                        output_guard.as_ref(),
                    );
                }
                state
                    .metrics
                    .semantic_cache_misses_total
                    .fetch_add(1, Relaxed);
                semantic_store = Some((
                    index_key,
                    exact_key.clone(),
                    embedding,
                    semantic.max_candidates,
                ));
            }
        }
    }

    // every lookup that could answer this request has missed
    cache_stage.record("hit", false);
    drop(cache_stage);

    // from here the handler is waiting on an upstream that may take minutes. if
    // the caller hangs up in that window axum drops this future and the request
    // would vanish with no row at all, so an armed guard owns the row until a
    // path that logs it takes over (#1083)
    let mut cancel = crate::cancel::CancelGuard::new(
        state.log.clone(),
        started,
        RequestLog {
            ts: crate::logging::started_at(started),
            request_id: request_id.clone(),
            trace_id: trace_id.clone(),
            virtual_key_id: vk_id.clone(),
            org_id: org_id.clone(),
            team_id: team_id.clone(),
            project_id: project_id.clone(),
            business_unit_id: business_unit_id.clone(),
            customer_id: customer_id.clone(),
            model: model.clone(),
            stream: stream as u8,
            sample_rate: snap.logging.sample_rate,
            ..Default::default()
        },
    );
    // pick a target and forward, retrying transient failures on a fresh target
    // (exponential backoff + jitter). retries happen before any body bytes reach
    // the client, so a partial response is never duplicated. a route with
    // variants routes through the weighted-variant fallback chain instead of the
    // classic single-pool balancer.
    // the successful attempt's span, held past the retry block so token usage
    // can be recorded on it once the response body has been consumed (#808)
    let mut genai_span: Option<tracing::Span> = None;
    let (
        outcome,
        last_provider,
        last_target,
        last_error,
        inflight_guard,
        chosen_variant,
        last_key_fingerprint,
    ) = if entry.route.has_variants() {
        let fwd = forward_variants(
            &state,
            entry,
            &snap,
            &model,
            &ctx,
            &parsed,
            &body,
            path,
            started,
            &trace_headers,
            vk.as_ref(),
        )
        .await;
        (
            fwd.outcome,
            fwd.last_provider,
            fwd.last_target,
            fwd.last_error,
            fwd.inflight_guard,
            fwd.variant,
            fwd.provider_key_fingerprint,
        )
    } else {
        let retry = &snap.retry;
        let cooldown = &snap.cooldown;
        let cd_enabled = cooldown.enabled();
        // live per-target in-flight counts steer load-aware strategies away from busy
        // targets; the count for the chosen target is held for the whole request.
        // scraped upstream queue depth (when enabled) is folded in so the balancer
        // also sees pressure that hasn't reached this gateway's own counters
        let mut loads = state.loads.snapshot(&model, entry.route.targets.len());
        for (i, target) in entry.route.targets.iter().enumerate() {
            if let Some(l) = loads.get_mut(i) {
                *l = l.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
            }
        }
        let mut tried: Vec<usize> = Vec::with_capacity(entry.route.targets.len());
        let mut last_provider = String::new();
        let mut last_target = model.clone();
        let mut last_error: Option<String> = None;
        let mut outcome: Option<(reqwest::Response, u16, bool)> = None;
        let mut inflight_guard: Option<crate::load::LoadGuard> = None;
        let mut last_key_fingerprint: Option<String> = None;

        for attempt in 0..=retry.max_retries {
            let idx = match pick_untried(
                entry,
                &ctx,
                &tried,
                &loads,
                &state.cooldowns,
                &state.health,
                &state.breaker,
                &model,
                cd_enabled,
                vk.as_ref(),
            ) {
                Some(i) => i,
                None => break, // no untried target left to fail over to
            };
            entry.balancer.observe(idx, &ctx);
            tried.push(idx);
            // count this attempt as in-flight; the guard falls out of scope (and
            // decrements) on retry, or is moved into the stream wrapper on success
            let mut guard = state.loads.begin(&model, idx);
            // the latency model learns cost as a function of prompt size, so it
            // needs this request's size before the guard's drop folds in the
            // sample (#853). inert on every other strategy
            guard.set_prompt_tokens(rolter_balancer::predictor::prompt_tokens(&ctx));

            let target = &entry.route.targets[idx];
            let provider = match snap.providers.get(&target.provider) {
                Some(provider) => provider,
                None => {
                    last_error = Some("configured target provider not found".to_string());
                    break;
                }
            };
            last_provider = target.provider.clone();
            last_target = target.model.clone().unwrap_or_else(|| model.clone());
            // weighted pick across the provider's key pool, skipping keys
            // parked on a cooldown (single-key providers yield their one key)
            let multi_key = provider.api_keys.len() > 1;
            let key_ns = key_pool_key(&target.provider);
            let picked_key = provider.pick_api_key_indexed(jitter(started), |i| {
                multi_key && state.cooldowns.is_parked(&key_ns, i)
            });
            let api_key = picked_key.as_ref().map(|(_, k)| k.as_str());
            last_key_fingerprint = api_key.map(provider_key_fingerprint);
            let upstream_model = target.model.as_deref();

            // one span per attempt, following the OTel GenAI conventions so a
            // conformant backend's built-in GenAI views work. the trace context
            // handed to the provider is injected from *inside* this span, which
            // is what makes the provider call a child of the gateway rather
            // than a sibling parented to the caller (#805).
            //
            // `otel.name` carries the `{operation} {model}` name the spec asks
            // for: `tracing` span names are compile-time constants, so the
            // exported name has to be set as a field (#808). `otel.kind=client`
            // marks this as the outbound leg
            let genai_model = upstream_model.unwrap_or(model.as_str());
            let upstream_stage = crate::trace::stage_span!(
                "upstream.request",
                attempt = attempt,
                otel.name = tracing::field::Empty,
                otel.kind = "client",
                gen_ai.operation.name = tracing::field::Empty,
                gen_ai.provider.name = %target.provider,
                gen_ai.system = %target.provider,
                gen_ai.request.model = %genai_model,
                gen_ai.output.type = tracing::field::Empty,
                gen_ai.response.model = tracing::field::Empty,
                gen_ai.response.id = tracing::field::Empty,
                gen_ai.response.finish_reasons = tracing::field::Empty,
                gen_ai.request.encoding_formats = tracing::field::Empty,
                gen_ai.embeddings.dimension.count = tracing::field::Empty,
                gen_ai.usage.input_tokens = tracing::field::Empty,
                gen_ai.usage.output_tokens = tracing::field::Empty,
                error.type = tracing::field::Empty,
                http.response.status_code = tracing::field::Empty
            );
            genai_span = Some(upstream_stage.clone());
            if let Some(operation) = crate::genai::operation_for_path(path) {
                upstream_stage.record(crate::genai::OPERATION_NAME, operation.name);
                upstream_stage.record(
                    "otel.name",
                    crate::genai::span_name(&operation, genai_model).as_str(),
                );
                if let Some(output_type) = operation.output_type {
                    upstream_stage.record(crate::genai::OUTPUT_TYPE, output_type);
                }
                // how the caller asked for the vectors — a request-side
                // attribute, so it is known now rather than at finalize (#846)
                if operation.name == crate::genai::OP_EMBEDDINGS {
                    if let Some(formats) = crate::genai::encoding_formats(&parsed) {
                        upstream_stage
                            .record(crate::genai::REQUEST_ENCODING_FORMATS, formats.as_str());
                    }
                }
            }
            let injected_ctx;
            let injected_refs;
            let attempt_headers: &[(&str, &str)] = if rolter_core::telemetry::is_active() {
                injected_ctx = upstream_stage
                    .in_scope(|| crate::trace::outbound_headers(&headers, &forwarded_names));
                injected_refs = injected_ctx
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect::<Vec<_>>();
                &injected_refs
            } else {
                &trace_headers
            };

            match state
                .provider_queues
                .forward_json(
                    &snap.queue,
                    provider,
                    path,
                    forward_body.clone(),
                    api_key,
                    upstream_model,
                    attempt_headers,
                )
                .instrument(upstream_stage.clone())
                .await
            {
                Ok(response) => {
                    let status = response.status().as_u16();
                    upstream_stage.record("http.response.status_code", status);
                    // error.type is conditionally required on failure, and must
                    // stay low-cardinality — the status class, never the
                    // provider's free-text message (#808)
                    if status >= 400 {
                        upstream_stage.record(
                            crate::genai::ERROR_TYPE,
                            crate::genai::error_type_for_status(status),
                        );
                    }
                    // a 429/401 on a multi-key provider is a key-level
                    // failure: park the key, keep the target in rotation
                    // and retry — a sibling key usually succeeds
                    let key_failure = multi_key && (status == 429 || status == 401);
                    if key_failure {
                        if let Some((ki, _)) = &picked_key {
                            let secs = cooldown.duration_secs(retry_after_secs(&response));
                            state.cooldowns.park(&key_ns, *ki, secs.max(1));
                            state
                                .metrics
                                .key_cooldowns_tripped_total
                                .fetch_add(1, Relaxed);
                        }
                        // let the same target be re-picked with a fresh key
                        tried.pop();
                        if attempt < retry.max_retries {
                            state.metrics.retries_total.fetch_add(1, Relaxed);
                            sleep(Duration::from_millis(retry_delay_ms(
                                retry,
                                &response,
                                attempt + 1,
                                started,
                            )))
                            .await;
                            continue;
                        }
                    } else if is_retryable_status(status) {
                        // park the failing target so siblings absorb the load
                        if cd_enabled {
                            let secs = cooldown.duration_secs(retry_after_secs(&response));
                            state.cooldowns.park(&model, idx, secs);
                            state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
                        }
                        // feed the circuit breaker; a sustained run trips the target open
                        if state.breaker.on_failure(&model, idx) {
                            state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
                        }
                        if attempt < retry.max_retries {
                            state.metrics.retries_total.fetch_add(1, Relaxed);
                            sleep(Duration::from_millis(retry_delay_ms(
                                retry,
                                &response,
                                attempt + 1,
                                started,
                            )))
                            .await;
                            continue;
                        }
                    } else if state.breaker.on_success(&model, idx) {
                        // a good response closes a breaker that was probing (half-open)
                        state.metrics.breaker_closed_total.fetch_add(1, Relaxed);
                    }
                    let is_sse = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(|ct| ct.contains("event-stream"))
                        .unwrap_or(false);
                    if status < 400 {
                        // successful attempt: fold its duration into the
                        // target's latency EWMA once streaming finishes
                        guard.mark_ok();
                    }
                    inflight_guard = Some(guard);
                    outcome = Some((response, status, is_sse));
                    break;
                }
                Err(err) => {
                    let message = err.to_string();
                    if is_queue_admission_error(&message) {
                        last_error = Some(message);
                        break;
                    }
                    last_error = Some(message);
                    // a connection-level failure parks the target too
                    if cd_enabled {
                        state
                            .cooldowns
                            .park(&model, idx, cooldown.duration_secs(None));
                        state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
                    }
                    // and counts against the circuit breaker
                    if state.breaker.on_failure(&model, idx) {
                        state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
                    }
                    if attempt < retry.max_retries {
                        state.metrics.retries_total.fetch_add(1, Relaxed);
                        sleep(Duration::from_millis(
                            retry.backoff_ms(attempt + 1, jitter(started)),
                        ))
                        .await;
                        continue;
                    }
                    break;
                }
            }
        }
        (
            outcome,
            last_provider,
            last_target,
            last_error,
            inflight_guard,
            String::new(),
            last_key_fingerprint,
        )
    };

    let capture_payloads = payload_capture_enabled(&snap.logging.payload_capture, &model, &vk_id);
    // from here every arm answers the caller: the streamed path hands the row
    // to `UsageLoggingStream` (which marks it if the client then leaves), the
    // error arm logs its own, and "no target selected" has never logged one
    cancel.disarm();
    match outcome {
        // token counts, latency and ttft are filled by the stream wrapper once
        // the body has been fully forwarded to the client
        Some((response, status, is_sse)) => {
            let translation = if status < 400 {
                snap.providers
                    .get(&last_provider)
                    .map(|provider| {
                        rolter_proxy::TranslationPlan::resolve(
                            path,
                            provider.kind,
                            provider.role_profile_for(Some(&last_target)),
                        )
                    })
                    .unwrap_or_else(rolter_proxy::TranslationPlan::passthrough)
            } else {
                rolter_proxy::TranslationPlan::passthrough()
            };
            let response_observer: Option<crate::logging::CompletionObserver> =
                if path == "/v1/responses" && status < 400 {
                    let capabilities = snap
                        .providers
                        .get(&last_provider)
                        .map(|provider| provider.kind == rolter_core::ProviderKind::Openai)
                        .unwrap_or(false);
                    state
                        .response_registry
                        .template(
                            tenant_scope(vk.as_ref()),
                            last_provider.clone(),
                            last_target.clone(),
                            model.clone(),
                            last_key_fingerprint,
                            if capabilities {
                                crate::response_registry::LifecycleCapabilities::NATIVE_OPENAI
                            } else {
                                crate::response_registry::LifecycleCapabilities::UNSUPPORTED
                            },
                        )
                        .map(|template| {
                            let registry = state.response_registry.clone();
                            Box::new(move |body: &[u8]| {
                                registry.record_body(template, is_sse, body);
                            }) as crate::logging::CompletionObserver
                        })
                } else {
                    None
                };
            let log = RequestLog {
                ts: crate::logging::started_at(started),
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                business_unit_id,
                customer_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: chosen_variant,
                status,
                stream: stream as u8,
                capture_payloads,
                payload_max_bytes: snap.logging.payload_capture.max_bytes,
                payload_redact_fields: snap.logging.payload_capture.redact_fields.clone(),
                request_payload: if capture_payloads {
                    crate::logging::capture_payload(
                        &forward_body,
                        snap.logging.payload_capture.max_bytes,
                        &snap.logging.payload_capture.redact_fields,
                    )
                } else {
                    String::new()
                },
                sample_rate: snap.logging.sample_rate,
                ..Default::default()
            };
            // on a cache-eligible route, buffer a successful response (JSON or a
            // full SSE stream) so it can be stored in Redis, then replay the
            // buffered bytes through the same accounting stream (byte-for-byte
            // identical token/cost/log handling to the uncached path). streaming
            // hits later replay these frames instantly; the final-chunk usage is
            // still parsed because the frames are handed on with is_sse preserved
            if let Some(key) = cache_key {
                if status < 400 {
                    let content_type = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("application/json")
                        .to_string();
                    match response.bytes().await {
                        Ok(bytes) => {
                            let bytes = translation.translate_response(bytes, is_sse);
                            // skip storing bodies over the configured ceiling
                            // (0 = no limit); they're still served normally
                            let limit = snap.cache.max_entry_bytes;
                            if limit == 0 || bytes.len() as u64 <= limit {
                                let cached = CachedResponse {
                                    status,
                                    content_type: content_type.clone(),
                                    body: bytes.to_vec(),
                                };
                                state.response_cache.put(&key, &cached, cache_ttl).await;
                                if let Some((index, entry_id, embedding, max_candidates)) =
                                    semantic_store
                                {
                                    state
                                        .response_cache
                                        .semantic_put(
                                            &index,
                                            &entry_id,
                                            embedding,
                                            &cached,
                                            cache_ttl,
                                            max_candidates,
                                        )
                                        .await;
                                    state
                                        .metrics
                                        .semantic_cache_stores_total
                                        .fetch_add(1, Relaxed);
                                }
                                state.metrics.cache_stores_total.fetch_add(1, Relaxed);
                            } else {
                                state.metrics.cache_too_large_total.fetch_add(1, Relaxed);
                            }
                            // masked on the way out, not on the way in: the
                            // entry stored above is what the upstream said, so
                            // later rule changes still reach it (see
                            // `cached_response`)
                            let bytes = match output_guard.as_ref() {
                                Some(guard) => match guard.apply(&bytes) {
                                    crate::guardrails::OutputDecision::Unchanged => bytes,
                                    crate::guardrails::OutputDecision::Masked(masked) => masked,
                                    crate::guardrails::OutputDecision::Blocked(rule) => {
                                        return guardrail_output_blocked(&rule)
                                    }
                                },
                                None => bytes,
                            };
                            // the sanitizer's response leg, after the output
                            // guardrails and after the cache store: what is
                            // cached is what the upstream said, so a restore is
                            // never persisted and a later delivery of the same
                            // entry is re-evaluated against its own request's
                            // policy and ticket (#848)
                            let bytes = match pii_leg.as_ref().filter(|_| status < 400) {
                                Some(leg) => pii_response_leg_apply(leg, bytes).await,
                                None => bytes,
                            };
                            return buffered_response(
                                bytes,
                                status,
                                content_type,
                                is_sse,
                                started,
                                state.log.clone(),
                                price,
                                log,
                                recorder,
                                token_recorder,
                                inflight_guard,
                                response_observer,
                                genai_span.clone(),
                            );
                        }
                        Err(err) => {
                            state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
                            return error_json(StatusCode::BAD_GATEWAY, &err.to_string());
                        }
                    }
                }
            }
            stream_response(
                response,
                is_sse,
                translation,
                started,
                state.log.clone(),
                price,
                log,
                recorder,
                token_recorder,
                inflight_guard,
                response_observer,
                output_guard.as_ref(),
                genai_span.clone(),
                post_response_plugins.clone(),
                pii_leg.as_ref(),
            )
            .await
        }
        None => {
            // no attempt ever reached an upstream: the balancer had no target
            if last_error.is_none() {
                return error_json(StatusCode::SERVICE_UNAVAILABLE, "no target selected");
            }
            state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
            let message = last_error.unwrap_or_default();
            state.log.log(RequestLog {
                ts: crate::logging::started_at(started),
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                business_unit_id,
                customer_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: chosen_variant,
                status: StatusCode::BAD_GATEWAY.as_u16(),
                stream: stream as u8,
                latency_ms: started.elapsed().as_millis() as u32,
                error: message.clone(),
                sample_rate: snap.logging.sample_rate,
                ..Default::default()
            });
            upstream_error_response(&message)
        }
    }
}

/// Multipart sibling of [`proxy`] for the audio upload endpoints
/// (`/v1/audio/transcriptions`, `/v1/audio/translations`). The body is
/// `multipart/form-data`, so the JSON pipeline can't parse it: instead the
/// `model` is read from the form fields and the raw body is forwarded verbatim
/// (content-type + boundary preserved) via [`Forwarder::forward_raw`]. Auth,
/// budgets, rate limits, routing, retries, cooldowns and the circuit breaker
/// match the classic single-pool path; variant routing and per-model param
/// injection do not apply (they're JSON-only), and the route target's upstream
/// model name is not rewritten into the multipart body.
async fn proxy_multipart(state: AppState, headers: HeaderMap, body: Bytes, path: &str) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let started = Instant::now();

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let boundary = match crate::multipart::boundary(&content_type) {
        Some(b) => b,
        None => {
            return crate::error::ApiError::new(
                StatusCode::BAD_REQUEST,
                "expected multipart/form-data body",
            )
            .with_code("invalid_content_type")
            .into_response()
        }
    };
    let model = match crate::multipart::text_field(&body, &boundary, "model") {
        Some(m) => m,
        None => {
            return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "missing model field")
                .with_code("missing_required_parameter")
                .with_param("model")
                .into_response()
        }
    };
    let response_format = crate::multipart::text_field(&body, &boundary, "response_format");

    let snap = state.snapshot.load();

    let vk = match authenticate(&state, &snap, &headers, path) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    if let Some(vk) = &vk {
        if !vk.model_permitted(&model) {
            return crate::error::ApiError::new(
                StatusCode::FORBIDDEN,
                "model not allowed for this key",
            )
            .with_code("model_not_allowed")
            .with_param("model")
            .into_response();
        }
    }

    let scope = vk
        .as_ref()
        .map(|v| ScopeIds {
            org: v.org_id.clone(),
            team: v.team_id.clone(),
            project: v.project_id.clone(),
            key: v.id.clone(),
            business_unit: v.business_unit_id.clone(),
            customer: v.customer_id.clone(),
        })
        .unwrap_or_default();

    if let Some(exceeded) = state.budgets.exceeded(&snap.budgets, &scope).await {
        state.metrics.budget_blocks_total.fetch_add(1, Relaxed);
        return crate::error::ApiError::new(
            StatusCode::PAYMENT_REQUIRED,
            format!(
                "budget exceeded for {:?} '{}' (limit ${:.2})",
                exceeded.scope, exceeded.id, exceeded.limit_usd
            ),
        )
        .with_code("insufficient_quota")
        .into_response();
    }

    if let Some(hit) = state.rate_limiter.check(&snap.rate_limits, &scope).await {
        state.metrics.rate_limit_blocks_total.fetch_add(1, Relaxed);
        let mut resp = crate::error::ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "{} rate limit exceeded for {:?} '{}' (limit {}/min)",
                hit.kind, hit.scope, hit.id, hit.limit
            ),
        )
        .with_code("rate_limit_exceeded")
        .into_response();
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(hit.retry_after));
        return resp;
    }

    // built-in fake-llm answers locally unless a configured route shadows it
    if model == fake_llm::MODEL_NAME && !snap.routes.contains_key(&model) {
        return fake_llm::transcription(response_format.as_deref());
    }

    // route-name-first: a named route always wins, even one whose name
    // contains '/'. only on a miss do we try `provider-slug/model` addressing
    // (ADR-0017), which pins a provider and forwards `model` as the upstream
    // model through the same classic-pool machinery (owned entry held here).
    let pinned = if snap.routes.contains_key(&model) {
        None
    } else {
        snap.resolve_pinned(&model)
    };
    let entry = match snap.routes.get(&model).or(pinned.as_ref()) {
        Some(entry) => entry,
        None => {
            return crate::error::ApiError::new(
                StatusCode::NOT_FOUND,
                format!("no route for model '{model}'"),
            )
            .with_code("model_not_found")
            .with_param("model")
            .into_response()
        }
    };
    if entry.route.targets.is_empty() {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "route has no targets");
    }

    let request_id = headers
        .get(crate::trace::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let trace_id = crate::trace::request_trace_id(&headers);
    let (vk_id, org_id, team_id, project_id) = (
        scope.key.clone(),
        scope.org.clone(),
        scope.team.clone(),
        scope.project.clone(),
    );
    // the trace backend needs the same tenant identity the request log carries,
    // so a collector can route this org's spans to this org's destination (#836)
    crate::trace::record_tenant(&org_id, &team_id, &project_id);
    let (business_unit_id, customer_id) = key_attribution(vk.as_ref());
    let token_recorder = TokenRecorder::new(
        state.rate_limiter.clone(),
        snap.rate_limits.clone(),
        scope.clone(),
    );
    let price = snap.prices.get(&model).cloned();
    // see the chat path: refuse unaccountable traffic before spending upstream
    // tokens on it (#974)
    if let Some(refused) = unpriced_admission(&state, &snap, &scope, &model, price.is_some()) {
        return refused;
    }
    let recorder = SpendRecorder::new(state.budgets.clone(), snap.budgets.clone(), scope);

    let token_ids = parse_vllm_token_ids(&headers);
    let ctx = RouteContext {
        session_key: headers.get("x-session-id").and_then(|v| v.to_str().ok()),
        prompt: None,
        token_ids: token_ids.as_deref(),
        // see the chat path: an adapter only exists when the request addresses
        // something other than the route's own model
        adapter: (model != entry.route.model).then_some(model.as_str()),
    };
    // trace context plus any client headers the operator allowlisted (#564)
    let trace_ctx =
        crate::trace::outbound_headers(&headers, &state.forwarder.forwarded_header_names());
    let trace_headers: Vec<(&str, &str)> = trace_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // an upload can sit upstream for a long time, and the caller can leave in
    // that window exactly as on the chat path (#1083)
    let mut cancel = crate::cancel::CancelGuard::new(
        state.log.clone(),
        started,
        RequestLog {
            ts: crate::logging::started_at(started),
            request_id: request_id.clone(),
            trace_id: trace_id.clone(),
            virtual_key_id: vk_id.clone(),
            org_id: org_id.clone(),
            team_id: team_id.clone(),
            project_id: project_id.clone(),
            business_unit_id: business_unit_id.clone(),
            customer_id: customer_id.clone(),
            model: model.clone(),
            sample_rate: snap.logging.sample_rate,
            ..Default::default()
        },
    );

    let retry = &snap.retry;
    let cooldown = &snap.cooldown;
    let cd_enabled = cooldown.enabled();
    let mut loads = state.loads.snapshot(&model, entry.route.targets.len());
    for (i, target) in entry.route.targets.iter().enumerate() {
        if let Some(l) = loads.get_mut(i) {
            *l = l.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
        }
    }
    let mut tried: Vec<usize> = Vec::with_capacity(entry.route.targets.len());
    let mut last_provider = String::new();
    let mut last_target = model.clone();
    let mut last_error: Option<String> = None;
    let mut outcome: Option<(reqwest::Response, u16, bool)> = None;
    let mut inflight_guard: Option<crate::load::LoadGuard> = None;

    for attempt in 0..=retry.max_retries {
        let idx = match pick_untried(
            entry,
            &ctx,
            &tried,
            &loads,
            &state.cooldowns,
            &state.health,
            &state.breaker,
            &model,
            cd_enabled,
            vk.as_ref(),
        ) {
            Some(i) => i,
            None => break,
        };
        entry.balancer.observe(idx, &ctx);
        tried.push(idx);
        let mut guard = state.loads.begin(&model, idx);
        // the latency model learns cost as a function of prompt size, so it
        // needs this request's size before the guard's drop folds in the
        // sample (#853). inert on every other strategy
        guard.set_prompt_tokens(rolter_balancer::predictor::prompt_tokens(&ctx));

        let target = &entry.route.targets[idx];
        let provider = match snap.providers.get(&target.provider) {
            Some(provider) => provider,
            None => {
                last_error = Some("configured target provider not found".to_string());
                break;
            }
        };
        last_provider = target.provider.clone();
        last_target = target.model.clone().unwrap_or_else(|| model.clone());
        let multi_key = provider.api_keys.len() > 1;
        let key_ns = key_pool_key(&target.provider);
        let picked_key = provider.pick_api_key_indexed(jitter(started), |i| {
            multi_key && state.cooldowns.is_parked(&key_ns, i)
        });
        let api_key = picked_key.as_ref().map(|(_, k)| k.as_str());

        match state
            .provider_queues
            .forward_raw(
                &snap.queue,
                provider,
                path,
                body.clone(),
                &content_type,
                api_key,
                &trace_headers,
            )
            .await
        {
            Ok(response) => {
                let status = response.status().as_u16();
                let key_failure = multi_key && (status == 429 || status == 401);
                if key_failure {
                    if let Some((ki, _)) = &picked_key {
                        let secs = cooldown.duration_secs(retry_after_secs(&response));
                        state.cooldowns.park(&key_ns, *ki, secs.max(1));
                        state
                            .metrics
                            .key_cooldowns_tripped_total
                            .fetch_add(1, Relaxed);
                    }
                    tried.pop();
                    if attempt < retry.max_retries {
                        state.metrics.retries_total.fetch_add(1, Relaxed);
                        sleep(Duration::from_millis(retry_delay_ms(
                            retry,
                            &response,
                            attempt + 1,
                            started,
                        )))
                        .await;
                        continue;
                    }
                } else if is_retryable_status(status) {
                    if cd_enabled {
                        let secs = cooldown.duration_secs(retry_after_secs(&response));
                        state.cooldowns.park(&model, idx, secs);
                        state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
                    }
                    if state.breaker.on_failure(&model, idx) {
                        state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
                    }
                    if attempt < retry.max_retries {
                        state.metrics.retries_total.fetch_add(1, Relaxed);
                        sleep(Duration::from_millis(retry_delay_ms(
                            retry,
                            &response,
                            attempt + 1,
                            started,
                        )))
                        .await;
                        continue;
                    }
                } else if state.breaker.on_success(&model, idx) {
                    state.metrics.breaker_closed_total.fetch_add(1, Relaxed);
                }
                if status < 400 {
                    guard.mark_ok();
                }
                inflight_guard = Some(guard);
                // transcription responses are JSON (or text), never SSE
                outcome = Some((response, status, false));
                break;
            }
            Err(err) => {
                let message = err.to_string();
                if is_queue_admission_error(&message) {
                    last_error = Some(message);
                    break;
                }
                last_error = Some(message);
                if cd_enabled {
                    state
                        .cooldowns
                        .park(&model, idx, cooldown.duration_secs(None));
                    state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
                }
                if state.breaker.on_failure(&model, idx) {
                    state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
                }
                if attempt < retry.max_retries {
                    state.metrics.retries_total.fetch_add(1, Relaxed);
                    sleep(Duration::from_millis(
                        retry.backoff_ms(attempt + 1, jitter(started)),
                    ))
                    .await;
                    continue;
                }
                break;
            }
        }
    }

    let capture_payloads = payload_capture_enabled(&snap.logging.payload_capture, &model, &vk_id);
    // as on the chat path: from here every arm answers the caller and logs its
    // own row, so the guard must not add a phantom disconnect on top
    cancel.disarm();
    match outcome {
        Some((response, status, is_sse)) => {
            let log = RequestLog {
                ts: crate::logging::started_at(started),
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                business_unit_id,
                customer_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: String::new(),
                status,
                stream: 0,
                capture_payloads,
                payload_max_bytes: snap.logging.payload_capture.max_bytes,
                payload_redact_fields: snap.logging.payload_capture.redact_fields.clone(),
                request_payload: if capture_payloads {
                    crate::logging::capture_payload(
                        &body,
                        snap.logging.payload_capture.max_bytes,
                        &snap.logging.payload_capture.redact_fields,
                    )
                } else {
                    String::new()
                },
                sample_rate: snap.logging.sample_rate,
                ..Default::default()
            };
            stream_response(
                response,
                is_sse,
                rolter_proxy::TranslationPlan::passthrough(),
                started,
                state.log.clone(),
                price,
                log,
                recorder,
                token_recorder,
                inflight_guard,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        }
        None => {
            if last_error.is_none() {
                return error_json(StatusCode::SERVICE_UNAVAILABLE, "no target selected");
            }
            state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
            let message = last_error.unwrap_or_default();
            state.log.log(RequestLog {
                ts: crate::logging::started_at(started),
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                business_unit_id,
                customer_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: String::new(),
                status: StatusCode::BAD_GATEWAY.as_u16(),
                stream: 0,
                latency_ms: started.elapsed().as_millis() as u32,
                error: message.clone(),
                sample_rate: snap.logging.sample_rate,
                ..Default::default()
            });
            upstream_error_response(&message)
        }
    }
}

/// The result of a forward loop, threaded back into the shared logging/response
/// tail so the classic and variant paths converge on one exit.
struct ForwardOutcome {
    outcome: Option<(reqwest::Response, u16, bool)>,
    last_provider: String,
    last_target: String,
    last_error: Option<String>,
    inflight_guard: Option<crate::load::LoadGuard>,
    /// chosen variant name for attribution
    variant: String,
    provider_key_fingerprint: Option<String>,
}

/// Namespaces the per-target reliability registries (cooldown/breaker/load) by
/// variant so a target's health under one variant never leaks into another.
pub(crate) fn variant_key(model: &str, variant: &str) -> String {
    format!("{model}::{variant}")
}

/// Namespaces the per-key cooldown registry: keys are parked per provider,
/// shared across every route and variant that uses that provider.
pub(crate) fn key_pool_key(provider: &str) -> String {
    format!("key::{provider}")
}

/// The order a variant's targets are tried: the variant balancer's pick leads
/// (fed the same live in-flight + upstream queue-depth signal as the classic
/// pool), then the remaining targets follow in declared order so the fallback
/// tail stays deterministic. A route without variant balancers (or a pick out
/// of range) degrades to plain declared order.
fn variant_target_order(
    entry: &crate::state::RouteEntry,
    ctx: &RouteContext<'_>,
    vi: usize,
    n: usize,
    loads: &[u64],
) -> Vec<usize> {
    let lead = entry
        .variant_balancers
        .get(vi)
        .and_then(|b| b.pick(ctx, loads))
        .filter(|&i| i < n);
    let mut order = Vec::with_capacity(n);
    if let Some(i) = lead {
        order.push(i);
    }
    order.extend((0..n).filter(|&i| Some(i) != lead));
    order
}

/// Forward through the weighted-variant fallback chain. Samples a primary
/// variant by weight, then flattens the deterministic fallback order into an
/// ordered candidate list and drives it through the same retry/cooldown/breaker
/// machinery as the classic path. Within each variant the route's balancing
/// strategy leads: the variant balancer's pick goes first, the remaining
/// targets follow in declared order as the deterministic fallback tail.
/// Variant-level params are merged over route-level params per candidate before
/// the override policy is applied.
#[allow(clippy::too_many_arguments)]
async fn forward_variants(
    state: &AppState,
    entry: &crate::state::RouteEntry,
    snap: &Snapshot,
    model: &str,
    ctx: &RouteContext<'_>,
    parsed: &Value,
    body: &Bytes,
    path: &str,
    started: Instant,
    trace_headers: &[(&str, &str)],
    key_meta: Option<&KeyMeta>,
) -> ForwardOutcome {
    let route = &entry.route;
    let retry = &snap.retry;
    let cooldown = &snap.cooldown;
    let cd_enabled = cooldown.enabled();

    // primary by weight, then the rest in declared order; flatten each variant's
    // targets into one ordered candidate list, letting the variant's balancer
    // choose which of its targets leads
    let primary = route.sample_variant(jitter(started)).unwrap_or(0);
    let mut candidates: Vec<(usize, usize)> =
        Vec::with_capacity(route.variants.iter().map(|v| v.targets.len()).sum());
    for vi in route.fallback_order(primary) {
        if let Some(v) = route.variants.get(vi) {
            let key = variant_key(model, &v.name);
            let mut loads = state.loads.snapshot(&key, v.targets.len());
            for (i, target) in v.targets.iter().enumerate() {
                if let Some(l) = loads.get_mut(i) {
                    *l = l.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
                }
            }
            for ti in variant_target_order(entry, ctx, vi, v.targets.len(), &loads) {
                if key_meta.is_none_or(|key| key.provider_allowed(&v.targets[ti].provider)) {
                    candidates.push((vi, ti));
                }
            }
        }
    }

    let mut out = ForwardOutcome {
        outcome: None,
        last_provider: String::new(),
        last_target: model.to_string(),
        last_error: None,
        inflight_guard: None,
        variant: String::new(),
        provider_key_fingerprint: None,
    };
    let mut tried: Vec<usize> = Vec::with_capacity(candidates.len());

    for attempt in 0..=retry.max_retries {
        // a candidate is skippable when its target is parked, its provider is
        // unhealthy, or its breaker is open — keyed per variant
        let skip = |&(vi, ti): &(usize, usize)| {
            let v = &route.variants[vi];
            let key = variant_key(model, &v.name);
            (cd_enabled && state.cooldowns.is_parked(&key, ti))
                || !state.health.is_healthy(&v.targets[ti].provider)
                || !state.breaker.allows(&key, ti)
        };
        // prefer an untried, non-skipped candidate; fail open to any untried one
        // when every remaining candidate is parked/unhealthy/open
        let fresh = (0..candidates.len()).find(|ci| !tried.contains(ci) && !skip(&candidates[*ci]));
        let ci = match fresh.or_else(|| (0..candidates.len()).find(|ci| !tried.contains(ci))) {
            Some(ci) => ci,
            None => break, // no untried candidate left to fail over to
        };
        tried.push(ci);
        let (vi, ti) = candidates[ci];
        let v = &route.variants[vi];
        let target = &v.targets[ti];
        let key = variant_key(model, &v.name);
        // let learning strategies (cache-aware) see which target actually served
        if let Some(b) = entry.variant_balancers.get(vi) {
            b.observe(ti, ctx);
        }

        let provider = match snap.providers.get(&target.provider) {
            Some(provider) => provider,
            None => {
                out.last_error = Some("configured target provider not found".to_string());
                break;
            }
        };
        out.variant = v.name.clone();
        out.last_provider = target.provider.clone();
        out.last_target = target.model.clone().unwrap_or_else(|| model.to_string());

        // merge variant params over route params for this candidate's body
        let mut injected = parsed.clone();
        route.apply_variant_params(v, &mut injected);
        let forward_body = serde_json::to_vec(&injected)
            .map(Bytes::from)
            .unwrap_or_else(|_| body.clone());

        // count this attempt as in-flight under the variant's key
        let mut guard = state.loads.begin(&key, ti);
        // the latency model learns cost as a function of prompt size, so it
        // needs this request's size before the guard's drop folds in the
        // sample (#853). inert on every other strategy
        guard.set_prompt_tokens(rolter_balancer::predictor::prompt_tokens(ctx));
        // weighted pick across the provider's key pool, skipping keys parked
        // on a cooldown — same policy as the classic path
        let multi_key = provider.api_keys.len() > 1;
        let key_ns = key_pool_key(&target.provider);
        let picked_key = provider.pick_api_key_indexed(jitter(started), |i| {
            multi_key && state.cooldowns.is_parked(&key_ns, i)
        });
        let api_key = picked_key.as_ref().map(|(_, k)| k.as_str());
        out.provider_key_fingerprint = api_key.map(provider_key_fingerprint);
        let upstream_model = target.model.as_deref();

        match state
            .provider_queues
            .forward_json(
                &snap.queue,
                provider,
                path,
                forward_body,
                api_key,
                upstream_model,
                trace_headers,
            )
            .await
        {
            Ok(response) => {
                let status = response.status().as_u16();
                // key-level failure on a multi-key provider: park the key,
                // keep the candidate in rotation and retry on a sibling key
                let key_failure = multi_key && (status == 429 || status == 401);
                if key_failure {
                    if let Some((ki, _)) = &picked_key {
                        let secs = cooldown.duration_secs(retry_after_secs(&response));
                        state.cooldowns.park(&key_ns, *ki, secs.max(1));
                        state
                            .metrics
                            .key_cooldowns_tripped_total
                            .fetch_add(1, Relaxed);
                    }
                    tried.pop();
                    if attempt < retry.max_retries {
                        state.metrics.retries_total.fetch_add(1, Relaxed);
                        sleep(Duration::from_millis(retry_delay_ms(
                            retry,
                            &response,
                            attempt + 1,
                            started,
                        )))
                        .await;
                        continue;
                    }
                } else if is_retryable_status(status) {
                    if cd_enabled {
                        let secs = cooldown.duration_secs(retry_after_secs(&response));
                        state.cooldowns.park(&key, ti, secs);
                        state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
                    }
                    if state.breaker.on_failure(&key, ti) {
                        state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
                    }
                    if attempt < retry.max_retries {
                        state.metrics.retries_total.fetch_add(1, Relaxed);
                        sleep(Duration::from_millis(retry_delay_ms(
                            retry,
                            &response,
                            attempt + 1,
                            started,
                        )))
                        .await;
                        continue;
                    }
                } else if state.breaker.on_success(&key, ti) {
                    state.metrics.breaker_closed_total.fetch_add(1, Relaxed);
                }
                let is_sse = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|ct| ct.contains("event-stream"))
                    .unwrap_or(false);
                if status < 400 {
                    guard.mark_ok();
                }
                out.inflight_guard = Some(guard);
                out.outcome = Some((response, status, is_sse));
                break;
            }
            Err(err) => {
                let message = err.to_string();
                if is_queue_admission_error(&message) {
                    out.last_error = Some(message);
                    break;
                }
                out.last_error = Some(message);
                if cd_enabled {
                    state.cooldowns.park(&key, ti, cooldown.duration_secs(None));
                    state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
                }
                if state.breaker.on_failure(&key, ti) {
                    state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
                }
                if attempt < retry.max_retries {
                    state.metrics.retries_total.fetch_add(1, Relaxed);
                    sleep(Duration::from_millis(
                        retry.backoff_ms(attempt + 1, jitter(started)),
                    ))
                    .await;
                    continue;
                }
                break;
            }
        }
    }
    out
}

/// Pick a target the request has not tried yet, preferring one that is not on a
/// cooldown. Honours the balancer's choice when it is fresh and healthy; else
/// falls over to the first untried, un-parked target. When every remaining
/// target is parked it fails open to the first untried one so requests still
/// flow rather than 503-ing on a transient wobble.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pick_untried(
    entry: &crate::state::RouteEntry,
    ctx: &RouteContext,
    tried: &[usize],
    loads: &[u64],
    cooldowns: &crate::cooldowns::Cooldowns,
    health: &crate::health::Health,
    breaker: &crate::breaker::Breaker,
    model: &str,
    cd_enabled: bool,
    key_meta: Option<&KeyMeta>,
) -> Option<usize> {
    // a target is skippable when parked on a cooldown, when its provider is
    // currently marked unhealthy by the active prober, or when its circuit
    // breaker is open
    let skip = |i: usize| {
        key_meta.is_some_and(|key| !key.provider_allowed(&entry.route.targets[i].provider))
            || (cd_enabled && cooldowns.is_parked(model, i))
            || !health.is_healthy(&entry.route.targets[i].provider)
            || !breaker.allows(model, i)
    };
    if let Some(i) = entry.balancer.pick(ctx, loads) {
        if !tried.contains(&i) && !skip(i) {
            return Some(i);
        }
    }
    let n = entry.route.targets.len();
    // prefer an untried, non-skipped target; fail open to any untried one when
    // every remaining sibling is parked or unhealthy
    (0..n)
        .find(|i| !tried.contains(i) && !skip(*i))
        .or_else(|| {
            (0..n).find(|i| {
                !tried.contains(i)
                    && key_meta
                        .is_none_or(|key| key.provider_allowed(&entry.route.targets[*i].provider))
            })
        })
}

/// Whether an upstream HTTP status is worth retrying: request timeout, too many
/// requests, or any server-side error.
fn is_retryable_status(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

/// Cheap, dependency-free jitter source in `[0, 1)` derived from the request
/// clock — good enough to decorrelate concurrent retriers.
fn jitter(started: Instant) -> f64 {
    (started.elapsed().subsec_nanos() % 1000) as f64 / 1000.0
}

/// Parse a `Retry-After` header expressed in whole seconds, if present.
fn retry_after_secs(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// Delay before the next retry. Honours a `Retry-After` (whole seconds) header on
/// a 429, capped at 30s; otherwise falls back to exponential backoff with jitter.
fn retry_delay_ms(
    cfg: &rolter_core::RetryConfig,
    response: &reqwest::Response,
    attempt: u32,
    started: Instant,
) -> u64 {
    if response.status().as_u16() == 429 {
        if let Some(secs) = retry_after_secs(response) {
            return (secs.saturating_mul(1000)).min(30_000);
        }
    }
    cfg.backoff_ms(attempt, jitter(started))
}

/// Convert an upstream response into a streaming axum response, teeing the body
/// through [`UsageLoggingStream`] so token usage and latency are logged once the
/// response has been fully forwarded.
#[allow(clippy::too_many_arguments)]
async fn stream_response(
    response: reqwest::Response,
    is_sse: bool,
    translation: rolter_proxy::TranslationPlan,
    started: Instant,
    sink: crate::logging::LogSink,
    price: Option<rolter_core::ModelPriceConfig>,
    log: RequestLog,
    recorder: SpendRecorder,
    token_recorder: TokenRecorder,
    inflight_guard: Option<crate::load::LoadGuard>,
    completion_observer: Option<crate::logging::CompletionObserver>,
    output: Option<&crate::guardrails::OutputGuard<'_>>,
    genai_span: Option<tracing::Span>,
    post_response_plugins: Option<crate::plugin_dispatch::PostResponsePlugins<'_>>,
    pii: Option<&PiiResponseLeg<'_>>,
) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    // surface the routing decision to the client before `log` is moved into the
    // stream wrapper: which provider/target/model/variant served the request and
    // whether it was a cache hit (ROL-58). this streamed path is always a live
    // upstream response (a miss); cache hits are served by `cached_response`
    let decision = DecisionHeaders::from_log(&log);
    // a non-streamed body is buffered when something has to see all of it: a
    // response translation, or output guardrails (#591). the guard only exists
    // when a post-call rule applies to this route, so an unguarded route keeps
    // streaming straight through
    if !is_sse
        && (translation.is_translation()
            || output.is_some()
            || post_response_plugins.is_some()
            || pii.is_some())
    {
        return match response.bytes().await {
            Ok(bytes) => {
                let bytes = translation.translate_response(bytes, false);
                let bytes = match output.filter(|_| status.is_success()) {
                    Some(guard) => match guard.apply(&bytes) {
                        crate::guardrails::OutputDecision::Unchanged => bytes,
                        crate::guardrails::OutputDecision::Masked(masked) => masked,
                        crate::guardrails::OutputDecision::Blocked(rule) => {
                            return guardrail_output_blocked(&rule)
                        }
                    },
                    None => bytes,
                };
                let bytes = match post_response_plugins.filter(|_| status.is_success()) {
                    Some(ctx) => match crate::plugin_dispatch::apply_post_response(&ctx, &bytes)
                        .await
                    {
                        crate::plugin_dispatch::PostResponseOutcome::Bytes(bytes) => {
                            Bytes::from(bytes)
                        }
                        crate::plugin_dispatch::PostResponseOutcome::Block(reason) => {
                            return crate::error::ApiError::new(
                                StatusCode::FORBIDDEN,
                                reason.unwrap_or_else(|| "response withheld by plugin".to_string()),
                            )
                            .with_code("plugin_blocked")
                            .into_response();
                        }
                    },
                    None => bytes,
                };
                // last, so the sanitizer sees the body the caller would have
                // received and a restore is never handed to a plugin
                let bytes = match pii.filter(|_| status.is_success()) {
                    Some(leg) => pii_response_leg_apply(leg, bytes).await,
                    None => bytes,
                };
                buffered_response(
                    bytes,
                    status.as_u16(),
                    content_type,
                    false,
                    started,
                    sink,
                    price,
                    log,
                    recorder,
                    token_recorder,
                    inflight_guard,
                    completion_observer,
                    genai_span,
                )
            }
            Err(err) => error_json(StatusCode::BAD_GATEWAY, &err.to_string()),
        };
    }
    let upstream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>,
    > = Box::pin(response.bytes_stream());
    let translated: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>,
    > = if translation.is_translation() && is_sse {
        Box::pin(rolter_proxy::TranslatedStream::new(upstream, translation))
    } else {
        upstream
    };
    let body = crate::logging::UsageLoggingStream::new(
        translated,
        is_sse,
        started,
        sink,
        price,
        log,
        Some(recorder),
        Some(token_recorder),
        inflight_guard,
    )
    .with_completion_observer(completion_observer)
    .with_genai_span(genai_span);
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type);
    decision.apply(builder.headers_mut());
    builder.body(Body::from_stream(body)).unwrap_or_else(|_| {
        error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build response",
        )
    })
}

/// Everything the sanitizer's response leg needs to run over a buffered body.
/// Built once per request and only when the leg is actually active, so a route
/// with no sanitizer configured carries nothing.
#[derive(Clone)]
struct PiiResponseLeg<'a> {
    state: &'a AppState,
    config: &'a rolter_core::PiiSanitizerConfig,
    model: &'a str,
    route: &'a str,
    trace_id: &'a str,
    tenant: rolter_core::WebhookTenant,
    scope: &'a rolter_core::TokenScope,
    ticket: Option<&'a rolter_core::RestorationTicket>,
    will_restore: bool,
}

/// Run the PII sanitizer's response leg over a buffered response body (#848).
///
/// Two independent steps, in this order: sanitize provider-generated PII, then
/// restore the placeholders this request's own sanitization introduced. Both
/// are best-effort — a failure returns the body unchanged, which is safe: the
/// worst case is a caller receiving placeholders instead of plaintext, never
/// the reverse.
async fn pii_response_leg_apply(leg: &PiiResponseLeg<'_>, bytes: Bytes) -> Bytes {
    let PiiResponseLeg {
        state,
        config,
        model,
        route,
        trace_id,
        tenant,
        scope,
        ticket,
        will_restore,
    } = leg;
    let (will_restore, ticket) = (*will_restore, *ticket);
    let (state, config, scope) = (*state, *config, *scope);
    // a body that does not parse is not JSON we can walk; forwarding it
    // untouched is strictly better than failing a successful response
    let Ok(mut parsed) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };

    if config.sanitizes_response() {
        match crate::pii_sanitizer::sanitize(
            config,
            &state.metrics,
            "response",
            model,
            route,
            trace_id,
            tenant.clone(),
            scope.clone(),
            // the response leg never asks for reversibility: restoring what the
            // *provider* generated would hand back the very PII this step exists
            // to remove
            false,
            &parsed,
        )
        .await
        {
            crate::pii_sanitizer::SanitizeOutcome::Sanitized { content, .. } => parsed = content,
            // a failed response-leg sanitize does not fail the request even
            // when fail-closed: the upstream call already happened and was
            // already billed, so refusing to deliver spends the caller's money
            // and gives them nothing. the counter records it
            crate::pii_sanitizer::SanitizeOutcome::Unchanged
            | crate::pii_sanitizer::SanitizeOutcome::Block(_) => {}
        }
    }

    if will_restore {
        if let Some(ticket) = ticket {
            if let Some(restored) = crate::pii_sanitizer::restore(
                config,
                &state.metrics,
                ticket,
                scope,
                trace_id,
                tenant.clone(),
                &parsed,
            )
            .await
            {
                parsed = restored;
            }
        }
    }

    serde_json::to_vec(&parsed)
        .map(Bytes::from)
        .unwrap_or(bytes)
}

/// Replay a buffered (already fully received) response body through the same
/// [`UsageLoggingStream`] accounting the streamed path uses. Used by the
/// response cache's store path: the body was buffered to write it to Redis, so
/// it's handed on as a single-chunk stream rather than re-fetched. `is_sse`
/// carries the upstream content type through so a buffered SSE completion is
/// still parsed frame-by-frame for its final usage chunk.
#[allow(clippy::too_many_arguments)]
fn buffered_response(
    bytes: Bytes,
    status: u16,
    content_type: String,
    is_sse: bool,
    started: Instant,
    sink: crate::logging::LogSink,
    price: Option<rolter_core::ModelPriceConfig>,
    log: RequestLog,
    recorder: SpendRecorder,
    token_recorder: TokenRecorder,
    inflight_guard: Option<crate::load::LoadGuard>,
    completion_observer: Option<crate::logging::CompletionObserver>,
    genai_span: Option<tracing::Span>,
) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let decision = DecisionHeaders::from_log(&log);
    let stream = futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(bytes)]);
    let body = crate::logging::UsageLoggingStream::new(
        Box::pin(stream),
        is_sse,
        started,
        sink,
        price,
        log,
        Some(recorder),
        Some(token_recorder),
        inflight_guard,
    )
    .with_completion_observer(completion_observer)
    .with_genai_span(genai_span);
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type);
    decision.apply(builder.headers_mut());
    builder.body(Body::from_stream(body)).unwrap_or_else(|_| {
        error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build response",
        )
    })
}

/// Log context captured on the request path for a response served from cache.
struct CacheHitLog {
    request_id: String,
    trace_id: String,
    vk_id: String,
    org_id: String,
    team_id: String,
    project_id: String,
    business_unit_id: String,
    customer_id: String,
    model: String,
    sample_rate: f64,
    started: Instant,
}

/// Apply the unpriced-traffic policy before any upstream tokens are spent
/// (#974).
///
/// A model with no price row accrues zero spend, so it can never exceed a
/// budget however much is served. `block` is the operator saying they would
/// rather refuse traffic than serve what they cannot account for; it is checked
/// here, where the model is known and the upstream call has not been made, so
/// nothing has to be undone.
///
/// The policy applied is the most restrictive of the deployment-wide setting
/// and any override on a budget matching this request's scope chain (#996), so
/// one team can refuse unaccountable traffic without the whole deployment
/// having to.
///
/// Returns the refusal response when the request must not be served.
fn unpriced_admission(
    state: &AppState,
    snap: &Snapshot,
    scope: &ScopeIds,
    model: &str,
    priced: bool,
) -> Option<Response> {
    if priced {
        return None;
    }
    let policy = resolve_unpriced_policy(snap.unpriced_policy, &snap.budgets, scope);
    match decide_unpriced(policy, model, &state.unpriced_warns) {
        UnpricedDecision::Serve => {
            state.metrics.unpriced_served_total.fetch_add(1, Relaxed);
            None
        }
        UnpricedDecision::Refuse => {
            state.metrics.unpriced_blocks_total.fetch_add(1, Relaxed);
            Some(
                crate::error::ApiError::new(
                    StatusCode::PAYMENT_REQUIRED,
                    format!(
                        "model '{model}' has no price, so its cost cannot be counted against a \
                         budget. This deployment, or a budget that applies to this key, is \
                         configured to refuse traffic it cannot account for; set a price for \
                         this model to serve it."
                    ),
                )
                .with_code("model_unpriced")
                .with_param("model")
                .into_response(),
            )
        }
    }
}

/// Governance attribution (`business_unit_id`, `customer_id`) carried by the
/// authenticated key. Config-defined and anonymous keys report neither, so the
/// log row falls back to tenancy attribution alone (#539).
fn key_attribution(vk: Option<&crate::state::KeyMeta>) -> (String, String) {
    vk.map(|v| (v.business_unit_id.clone(), v.customer_id.clone()))
        .unwrap_or_default()
}

fn payload_capture_enabled(
    config: &rolter_core::PayloadCaptureConfig,
    model: &str,
    virtual_key_id: &str,
) -> bool {
    config.enabled
        && (config.models.is_empty() || config.models.iter().any(|name| name == model))
        && (config.virtual_key_ids.is_empty()
            || config.virtual_key_ids.iter().any(|id| id == virtual_key_id))
}

fn semantic_cache_text(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let mut result = String::with_capacity(body.len().min(1024));
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(role) = message.get("role").and_then(Value::as_str) {
                append_normalized(role, &mut result);
            }
            append_semantic_text(message.get("content"), &mut result);
        }
    } else {
        append_semantic_text(value.get("input"), &mut result);
        append_semantic_text(value.get("prompt"), &mut result);
    }

    (!result.is_empty()).then_some(result)
}

fn parse_vllm_token_ids(headers: &HeaderMap) -> Option<Vec<u32>> {
    headers
        .get("x-rolter-vllm-token-ids")?
        .to_str()
        .ok()?
        .split(',')
        .map(|value| value.trim().parse().ok())
        .collect::<Option<Vec<_>>>()
        .filter(|ids| !ids.is_empty())
}

fn append_semantic_text(value: Option<&Value>, out: &mut String) {
    match value {
        Some(Value::String(text)) => append_normalized(text, out),
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    append_normalized(text, out);
                }
            }
        }
        _ => {}
    }
}

fn append_normalized(text: &str, out: &mut String) {
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
}

async fn semantic_embedding(
    state: &AppState,
    snap: &Snapshot,
    config: &rolter_core::SemanticCacheConfig,
    text: &str,
) -> Option<Vec<f32>> {
    let provider = snap.providers.get(&config.provider)?;
    let api_key = provider.resolve_api_key();
    let body = serde_json::to_vec(&json!({
        "model": config.model,
        "input": text,
    }))
    .ok()?;
    let response = state
        .forwarder
        .forward_json(
            provider,
            "/v1/embeddings",
            Bytes::from(body),
            api_key.as_deref(),
            None,
            &[],
        )
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&response.bytes().await.ok()?).ok()?;
    value
        .get("data")?
        .as_array()?
        .first()?
        .get("embedding")?
        .as_array()?
        .iter()
        .map(|value| value.as_f64().map(|number| number as f32))
        .collect()
}

/// Build the client reply for a cache hit: the stored body, decision headers
/// with `x-rolter-cache: HIT`, and a log row marked `cache_hit` with zero
/// cost/tokens (a hit spends nothing upstream, so it is not billed and records
/// no upstream target).
///
/// Output guardrails run here too. The cache stores what the upstream returned,
/// and masking happens on every delivery instead of once at store time — so a
/// rule added after an entry was cached still applies to it, and an entry shared
/// by two routes is masked per the route serving it.
fn cached_response(
    hit: CachedResponse,
    sink: &crate::logging::LogSink,
    ctx: CacheHitLog,
    output: Option<&crate::guardrails::OutputGuard<'_>>,
) -> Response {
    let status = StatusCode::from_u16(hit.status).unwrap_or(StatusCode::OK);
    let mut body = Bytes::from(hit.body);
    if status.is_success() {
        if let Some(guard) = output {
            match guard.apply(&body) {
                crate::guardrails::OutputDecision::Unchanged => {}
                crate::guardrails::OutputDecision::Masked(masked) => body = masked,
                crate::guardrails::OutputDecision::Blocked(rule) => {
                    return guardrail_output_blocked(&rule)
                }
            }
        }
    }
    let latency_ms = ctx.started.elapsed().as_millis() as u32;
    let log = RequestLog {
        ts: crate::logging::started_at(ctx.started),
        request_id: ctx.request_id,
        trace_id: ctx.trace_id,
        virtual_key_id: ctx.vk_id,
        org_id: ctx.org_id,
        team_id: ctx.team_id,
        project_id: ctx.project_id,
        business_unit_id: ctx.business_unit_id,
        customer_id: ctx.customer_id,
        model: ctx.model,
        status: hit.status,
        cache_hit: 1,
        latency_ms,
        ttft_ms: latency_ms,
        sample_rate: ctx.sample_rate,
        ..Default::default()
    };
    let decision = DecisionHeaders::from_log(&log);
    sink.log(log);
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, hit.content_type);
    decision.apply(builder.headers_mut());
    builder.body(Body::from(body)).unwrap_or_else(|_| {
        error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build response",
        )
    })
}

/// The reply when a post-call rule refuses to deliver a completion.
///
/// 403, not 400 or 502: nothing was malformed and no upstream failed — the
/// gateway is refusing to hand over content it holds. That also keeps it out of
/// the range clients retry on, which matters because a retry would be blocked
/// again. The rule name is safe to return; it never carries the matched text.
fn guardrail_output_blocked(rule: &str) -> Response {
    crate::error::ApiError::new(
        StatusCode::FORBIDDEN,
        format!("response withheld by guardrail '{rule}'"),
    )
    .with_code("guardrail_blocked")
    .into_response()
}

/// `x-rolter-*` response headers that expose the routing decision for a request:
/// the provider, target, resolved model and A/B variant that served it, and
/// whether the response came from the cache. Purely observational — clients can
/// log or branch on them, and they make the gateway's target selection debuggable
/// without turning on ClickHouse logging (ROL-58).
struct DecisionHeaders {
    provider: String,
    target: String,
    model: String,
    variant: String,
    cache_hit: bool,
}

impl DecisionHeaders {
    fn from_log(log: &RequestLog) -> Self {
        Self {
            provider: log.provider.clone(),
            target: log.target.clone(),
            model: log.model.clone(),
            variant: log.variant.clone(),
            cache_hit: log.cache_hit != 0,
        }
    }

    /// Insert the non-empty decision headers into `headers`. Values that are not
    /// valid header content are skipped rather than failing the response.
    fn apply(&self, headers: Option<&mut HeaderMap>) {
        let Some(headers) = headers else {
            return;
        };
        for (name, value) in [
            ("x-rolter-provider", self.provider.as_str()),
            ("x-rolter-target", self.target.as_str()),
            ("x-rolter-model", self.model.as_str()),
            ("x-rolter-variant", self.variant.as_str()),
        ] {
            if value.is_empty() {
                continue;
            }
            if let Ok(v) = HeaderValue::from_str(value) {
                headers.insert(name, v);
            }
        }
        headers.insert(
            "x-rolter-cache",
            HeaderValue::from_static(if self.cache_hit { "HIT" } else { "MISS" }),
        );
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::extract::State;
    use axum::http::HeaderValue;

    use rolter_core::{
        BalancingStrategy, GatewayConfig, ModelRoute, Target, VirtualKeyConfig, VirtualKeyRecord,
    };

    use super::*;

    #[test]
    fn queue_admission_errors_are_actionable() {
        let full = upstream_error_response("provider queue full");
        assert_eq!(full.status(), StatusCode::TOO_MANY_REQUESTS);
        let timeout = upstream_error_response("provider queue wait timed out");
        assert_eq!(timeout.status(), StatusCode::TOO_MANY_REQUESTS);
        let dropped = upstream_error_response("provider queue request dropped");
        assert_eq!(dropped.status(), StatusCode::SERVICE_UNAVAILABLE);
        let upstream = upstream_error_response("connection refused");
        assert_eq!(upstream.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn queue_admission_errors_survive_the_upstream_error_wrapper() {
        // the forwarding loop only ever sees `Error::Upstream`'s Display form,
        // so the wrapped message is what must classify (#639)
        let wrapped = "upstream error: provider queue full";
        assert!(is_queue_admission_error(wrapped));
        assert_eq!(
            queue_admission_message(wrapped),
            Some("provider queue full")
        );
        assert_eq!(
            upstream_error_response(wrapped).status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert!(!is_queue_admission_error(
            "upstream error: connection refused"
        ));
    }

    /// #882: a request-translation failure is the caller's to fix, so it must
    /// carry a 400 and a machine-readable code rather than a generic 502 that
    /// points the operator at the provider.
    #[tokio::test]
    async fn translation_request_errors_are_client_errors() {
        let cases = [
            (
                "config error: role_capability: developer role unsupported",
                "role_capability_unsupported",
                "developer role unsupported",
            ),
            (
                "config error: unsupported_content_part: the interactions upstream has no \
                 equivalent for content part type 'input_audio'",
                "unsupported_content_part",
                "content part type 'input_audio'",
            ),
            (
                "config error: gemini_interactions_unsupported: this endpoint has no \
                 interactions equivalent",
                "gemini_interactions_unsupported",
                "no interactions equivalent",
            ),
        ];
        for (message, code, fragment) in cases {
            let response = upstream_error_response(message);
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "{message} was not a client error"
            );
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["error"]["code"], code);
            let rendered = body["error"]["message"].as_str().unwrap();
            assert!(rendered.contains(fragment), "{rendered}");
            // the `Error::Config` wrapping never reaches the caller
            assert!(!rendered.starts_with("config error: "), "{rendered}");
        }
    }

    /// A config error that is not a translation failure keeps its 502 — this
    /// classification is an allowlist, not a blanket downgrade.
    #[test]
    fn other_config_errors_are_still_gateway_errors() {
        assert_eq!(
            upstream_error_response("config error: no provider key configured").status(),
            StatusCode::BAD_GATEWAY
        );
        assert!(translation_request_error("upstream error: role_capability: x").is_none());
    }

    #[test]
    fn semantic_text_normalizes_chat_content_only() {
        let body = br#"{
            "model":"gpt-4o","temperature":0.7,
            "messages":[
                {"role":"system","content":"You are helpful."},
                {"role":"user","content":[{"type":"text","text":"hello   world"}]}
            ]
        }"#;
        assert_eq!(
            semantic_cache_text(body).as_deref(),
            Some("system You are helpful. user hello world")
        );
    }

    #[test]
    fn semantic_text_normalizes_responses_input_and_prompt() {
        let body = br#"{
            "input":[{"type":"input_text","text":"  first\nvalue "}],
            "prompt":" second\tvalue "
        }"#;
        assert_eq!(
            semantic_cache_text(body).as_deref(),
            Some("first value second value")
        );
        assert_eq!(semantic_cache_text(br#"{"input":[]}"#), None);
        assert_eq!(semantic_cache_text(b"not json"), None);
    }

    fn config_with_keys() -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.routes.push(ModelRoute {
            model: "gpt-4o".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![Target {
                provider: "openai".to_string(),
                model: None,
                weight: 1,
            }],
        });
        config.routes.push(ModelRoute {
            model: "claude".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![Target {
                provider: "anthropic".to_string(),
                model: None,
                weight: 1,
            }],
        });
        config.virtual_keys.push(VirtualKeyConfig {
            key: "sk-gpt-only".to_string(),
            name: None,
            models: vec!["gpt-4o".to_string()],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        });
        config
    }

    #[test]
    fn a_provider_key_is_named_rather_than_answered_with_a_bare_rejection() {
        // the mistake this exists for: an operator pastes the credential a
        // provider gave rolter into the client that should be holding the
        // credential rolter gave them. both are "api keys" in the product's own
        // vocabulary, and the old answer was an unhelpful `invalid api key`
        for (key, vendor) in [
            ("sk-ant-api03-abcdef", "Anthropic"),
            ("sk-or-v1-abcdef", "OpenRouter"),
            ("sk-proj-abcdef", "OpenAI"),
            ("AIzaSyAbcdef", "Google AI Studio"),
            ("gsk_abcdef", "Groq"),
            ("xai-abcdef", "xAI"),
            ("nvapi-abcdef", "NVIDIA"),
        ] {
            assert_eq!(provider_key_vendor(key), Some(vendor), "{key}");
            let message = invalid_key_message(key);
            assert!(message.contains(vendor), "{message}");
            assert!(message.contains(ROLTER_KEY_PREFIX), "{message}");
        }
    }

    #[test]
    fn an_unrecognised_openai_shaped_key_still_says_which_credential_is_wanted() {
        let message = invalid_key_message("sk-somethingelse");
        assert!(message.contains("provider credential"), "{message}");
        assert!(message.contains(ROLTER_KEY_PREFIX), "{message}");
    }

    #[test]
    fn our_own_key_shape_is_never_reported_as_a_provider_key() {
        // a revoked, expired or mistyped rolter key is not this mistake, and
        // telling its holder to go and find a virtual key would be nonsense
        for key in [
            "sk-rolter-deadbeef",
            "sk-rolter-",
            "rolter_sess_abcdef",
            "some-opaque-token",
            "",
        ] {
            assert_eq!(provider_key_vendor(key), None, "{key}");
            assert_eq!(invalid_key_message(key), "invalid api key", "{key}");
        }
    }

    async fn models_in_response(resp: Response) -> Vec<String> {
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        value["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn list_models_requires_auth_when_keys_configured() {
        let state = AppState::new(&config_with_keys());
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_models_filters_by_key_allow_list() {
        let state = AppState::new(&config_with_keys());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-gpt-only"),
        );
        let resp = list_models(State(state), headers).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(models_in_response(resp).await, vec!["gpt-4o".to_string()]);
    }

    #[tokio::test]
    async fn list_models_open_when_no_keys_configured() {
        let state = AppState::new(&GatewayConfig::default());
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // regression for #626: an empty key set used to mean "auth disabled"
    // unconditionally, so revoking the last virtual key silently opened the
    // whole data plane. a managed gateway must fail closed instead.
    #[tokio::test]
    async fn managed_gateway_denies_when_the_key_set_is_empty() {
        let mut state = AppState::new(&GatewayConfig::default());
        state.managed_auth = true;
        let resp = list_models(State(state.clone()), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // a garbage key is rejected too — nothing can match an empty set
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-garbage"),
        );
        let resp = list_models(State(state), headers).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // #1162: the Security screen's "enforce virtual keys on inference" toggle
    // was stored, rendered, and read by nothing. these four tests are the
    // reason it cannot go back to being decorative.
    #[tokio::test]
    async fn the_security_screen_can_close_an_unmanaged_gateway() {
        let mut config = GatewayConfig::default();
        config.security.virtual_key_required = true;
        let state = AppState::new(&config);
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_config_file_still_overrides_the_security_screen_in_both_directions() {
        // the file is a deliberate local override of a database the operator
        // running this process may not even be able to reach
        let mut config = GatewayConfig::default();
        config.security.virtual_key_required = true;
        config.server.require_auth = Some(false);
        let state = AppState::new(&config);
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let mut config = GatewayConfig::default();
        config.security.virtual_key_required = false;
        config.server.require_auth = Some(true);
        let state = AppState::new(&config);
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_bypass_route_is_matched_exactly_and_never_by_prefix() {
        let mut config = config_with_keys();
        config.security.auth_bypass_routes = vec!["/v1/models".to_string()];
        let state = AppState::new(&config);
        // named: served with no key at all, and nothing attributed to one
        let resp = list_models(State(state.clone()), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // a longer path that merely starts with the named one is not bypassed
        let snap = state.snapshot.load();
        assert!(snap.security.bypasses_auth("/v1/models"));
        assert!(!snap.security.bypasses_auth("/v1/models/gpt-4o"));
        assert!(!snap.security.bypasses_auth("/v1/chat/completions"));
        assert!(authenticate(&state, &snap, &HeaderMap::new(), "/v1/chat/completions").is_err());
    }

    #[test]
    fn a_required_header_must_match_its_value_not_merely_be_present() {
        let mut policy = rolter_core::SecurityPolicyConfig::default();
        policy
            .required_headers
            .insert("x-mesh-id".to_string(), "edge".to_string());

        assert_eq!(policy.missing_required_header(|_| None), Some("x-mesh-id"));
        assert_eq!(
            policy.missing_required_header(|_| Some("elsewhere")),
            Some("x-mesh-id")
        );
        assert_eq!(policy.missing_required_header(|_| Some("edge")), None);
    }

    #[tokio::test]
    async fn require_auth_config_denies_an_empty_key_set_even_unmanaged() {
        let mut config = GatewayConfig::default();
        config.server.require_auth = Some(true);
        let state = AppState::new(&config);
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_auth_false_keeps_a_managed_gateway_keyless() {
        // explicit opt-in to the keyless path wins over the managed default
        let mut config = GatewayConfig::default();
        config.server.require_auth = Some(false);
        let mut state = AppState::new(&config);
        state.managed_auth = true;
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn list_models_includes_builtin_fake_llm() {
        let state = AppState::new(&GatewayConfig::default());
        let resp = list_models(State(state), HeaderMap::new()).await;
        assert_eq!(
            models_in_response(resp).await,
            vec![crate::fake_llm::MODEL_NAME.to_string()]
        );
    }

    #[tokio::test]
    async fn fake_llm_serves_chat_completions_without_config() {
        let state = AppState::new(&GatewayConfig::default());
        let body = Bytes::from(r#"{"model": "fake-llm", "messages": []}"#);
        let resp = chat_completions(State(state), HeaderMap::new(), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "chat.completion");
        assert_eq!(value["model"], "fake-llm");
    }

    #[tokio::test]
    async fn fake_llm_serves_messages_without_config() {
        let state = AppState::new(&GatewayConfig::default());
        let body = Bytes::from(r#"{"model": "fake-llm", "messages": []}"#);
        let resp = messages(State(state), HeaderMap::new(), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["type"], "message");
        assert_eq!(value["stop_reason"], "end_turn");
    }

    #[tokio::test]
    async fn fake_llm_serves_responses_without_config() {
        let state = AppState::new(&GatewayConfig::default());
        let body = Bytes::from(r#"{"model": "fake-llm", "input": "hello"}"#);
        let resp = responses(State(state), HeaderMap::new(), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["object"], "response");
    }

    #[tokio::test]
    async fn response_lifecycle_is_not_forwarded() {
        let state = AppState::new(&GatewayConfig::default());
        let resp = unsupported_response_lifecycle(
            State(state),
            HeaderMap::new(),
            OriginalUri("/v1/responses/resp_other_tenant/compact".parse().unwrap()),
            Path("resp_other_tenant".to_string()),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["code"], "response_lifecycle_unsupported");
    }

    #[tokio::test]
    async fn fake_llm_not_served_on_legacy_completions() {
        let state = AppState::new(&GatewayConfig::default());
        let body = Bytes::from(r#"{"model": "fake-llm", "prompt": "hi"}"#);
        let resp = completions(State(state), HeaderMap::new(), body).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn fake_llm_respects_virtual_key_auth() {
        let state = AppState::new(&config_with_keys());
        let body = Bytes::from(r#"{"model": "fake-llm", "messages": []}"#);
        let resp = chat_completions(State(state), HeaderMap::new(), body).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    fn bearer(key: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {key}")).unwrap(),
        );
        headers
    }

    fn config_with_one_key(vk: VirtualKeyConfig) -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.virtual_keys.push(vk);
        config
    }

    #[tokio::test]
    async fn db_virtual_key_authenticates_by_digest_with_scope() {
        // a database-defined key: gateway stores the pre-computed digest and the
        // scope identity, never the plaintext
        let mut config = GatewayConfig::default();
        let pepper = config.server.resolve_key_pepper();
        config.db_virtual_keys.push(VirtualKeyRecord {
            access_policy: None,
            key_hash: rolter_auth::hash_key(&pepper, "sk-db-key"),
            id: "vk-1".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "proj-1".to_string(),
            user_id: "user-1".to_string(),
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
            business_unit_id: String::new(),
            customer_id: String::new(),
        });
        let state = AppState::new(&config);

        // the right plaintext authenticates
        let resp = list_models(State(state.clone()), bearer("sk-db-key")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        // a wrong key does not
        let resp = list_models(State(state.clone()), bearer("sk-wrong")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // the snapshot carries scope identity, indexed by digest not plaintext
        let snap = state.snapshot.load();
        assert!(!snap.keys.contains_key("sk-db-key"));
        let meta = snap
            .keys
            .get(&rolter_auth::hash_key(&snap.pepper, "sk-db-key"))
            .expect("db key present by digest");
        assert_eq!(meta.id, "vk-1");
        assert_eq!(meta.org_id, "org-1");
        assert_eq!(meta.project_id, "proj-1");
    }

    #[tokio::test]
    async fn db_virtual_key_carries_cost_attribution_into_logs() {
        let mut config = GatewayConfig::default();
        let pepper = config.server.resolve_key_pepper();
        config.db_virtual_keys.push(VirtualKeyRecord {
            access_policy: None,
            key_hash: rolter_auth::hash_key(&pepper, "sk-attributed"),
            id: "vk-1".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "proj-1".to_string(),
            user_id: "user-1".to_string(),
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
            business_unit_id: "bu-1".to_string(),
            customer_id: "cust-1".to_string(),
        });
        let state = AppState::new(&config);
        let snap = state.snapshot.load();
        let meta = snap
            .keys
            .get(&rolter_auth::hash_key(&snap.pepper, "sk-attributed"))
            .expect("db key present by digest");
        assert_eq!(meta.business_unit_id, "bu-1");
        assert_eq!(meta.customer_id, "cust-1");
        assert_eq!(
            key_attribution(Some(meta)),
            ("bu-1".to_string(), "cust-1".to_string())
        );
        // config-defined and anonymous keys stay unattributed
        assert_eq!(key_attribution(None), (String::new(), String::new()));
    }

    #[tokio::test]
    async fn expired_key_is_rejected() {
        let vk = VirtualKeyConfig {
            key: "sk-expired".to_string(),
            name: None,
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
            cache: None,
        };
        let state = AppState::new(&config_with_one_key(vk));
        let resp = list_models(State(state), bearer("sk-expired")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn disabled_key_is_rejected() {
        let vk = VirtualKeyConfig {
            key: "sk-disabled".to_string(),
            name: None,
            models: vec![],
            providers: vec![],
            disabled: true,
            expires_at: None,
            cache: None,
        };
        let state = AppState::new(&config_with_one_key(vk));
        let resp = list_models(State(state), bearer("sk-disabled")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unexpired_active_key_authenticates() {
        let vk = VirtualKeyConfig {
            key: "sk-live".to_string(),
            name: None,
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            cache: None,
        };
        let state = AppState::new(&config_with_one_key(vk));
        let resp = list_models(State(state), bearer("sk-live")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn plaintext_key_is_not_retained_in_snapshot() {
        // keys are indexed by peppered digest, never by the raw secret
        let state = AppState::new(&config_with_keys());
        let snap = state.snapshot.load();
        assert!(!snap.keys.contains_key("sk-gpt-only"));
        assert!(snap
            .keys
            .contains_key(&rolter_auth::hash_key(&snap.pepper, "sk-gpt-only")));
    }

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn variant_order_led_by_balancer_pick() {
        struct Fixed(usize);
        impl rolter_balancer::LoadBalancer for Fixed {
            fn name(&self) -> &'static str {
                "fixed"
            }
            fn pick(&self, _: &RouteContext, _: &[u64]) -> Option<usize> {
                Some(self.0)
            }
        }
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            targets: Vec::new(),
            variants: vec![rolter_core::Variant {
                name: "v".to_string(),
                weight: 1,
                params: Default::default(),
                targets: vec![
                    Target {
                        provider: "a".to_string(),
                        model: None,
                        weight: 1,
                    },
                    Target {
                        provider: "b".to_string(),
                        model: None,
                        weight: 1,
                    },
                ],
            }],
        };
        let ctx = RouteContext::default();
        // the balancer's pick leads; declared order forms the fallback tail
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]),
            variant_balancers: vec![Box::new(Fixed(1))],
            route: route.clone(),
        };
        assert_eq!(variant_target_order(&entry, &ctx, 0, 2, &[]), vec![1, 0]);
        // an out-of-range pick degrades to plain declared order
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]),
            variant_balancers: vec![Box::new(Fixed(9))],
            route: route.clone(),
        };
        assert_eq!(variant_target_order(&entry, &ctx, 0, 2, &[]), vec![0, 1]);
        // no balancer built for the variant: declared order
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]),
            variant_balancers: Vec::new(),
            route,
        };
        assert_eq!(variant_target_order(&entry, &ctx, 0, 2, &[]), vec![0, 1]);
    }

    #[test]
    fn snapshot_builds_one_balancer_per_variant() {
        let mut config = config_with_keys();
        config.routes.push(rolter_core::ModelRoute {
            model: "ab".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            targets: Vec::new(),
            variants: vec![
                rolter_core::Variant {
                    name: "control".to_string(),
                    weight: 1,
                    params: Default::default(),
                    targets: vec![Target {
                        provider: "a".to_string(),
                        model: None,
                        weight: 1,
                    }],
                },
                rolter_core::Variant {
                    name: "canary".to_string(),
                    weight: 1,
                    params: Default::default(),
                    targets: vec![Target {
                        provider: "b".to_string(),
                        model: None,
                        weight: 1,
                    }],
                },
            ],
        });
        let snap = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        assert_eq!(snap.routes["ab"].variant_balancers.len(), 2);
    }

    #[test]
    fn pick_untried_fails_over_to_sibling() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![
                Target {
                    provider: "a".to_string(),
                    model: None,
                    weight: 1,
                },
                Target {
                    provider: "b".to_string(),
                    model: None,
                    weight: 1,
                },
            ],
        };
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let first = pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false, None).unwrap();
        // with the first target excluded, the fallback must choose the other one
        let second =
            pick_untried(&entry, &ctx, &[first], &[], &cd, &hh, &bb, "m", false, None).unwrap();
        assert_ne!(first, second);
        // both tried: nothing left to fail over to
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &[first, second],
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None
            ),
            None
        );
    }

    #[test]
    fn pick_untried_skips_parked_target() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![
                Target {
                    provider: "a".to_string(),
                    model: None,
                    weight: 1,
                },
                Target {
                    provider: "b".to_string(),
                    model: None,
                    weight: 1,
                },
            ],
        };
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::new();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        // park target 0: selection must avoid it and pick 1
        cd.park("m", 0, 60);
        assert_eq!(
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true, None),
            Some(1)
        );
        // park both: fail open to an untried target rather than returning None
        cd.park("m", 1, 60);
        assert!(pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true, None).is_some());
    }

    #[test]
    fn pick_untried_skips_unhealthy_provider() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![
                Target {
                    provider: "a".to_string(),
                    model: None,
                    weight: 1,
                },
                Target {
                    provider: "b".to_string(),
                    model: None,
                    weight: 1,
                },
            ],
        };
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::new();
        let bb = crate::breaker::Breaker::default();
        // mark provider "a" (target 0) unhealthy: selection must pick target 1
        hh.set("a", false);
        assert_eq!(
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false, None),
            Some(1)
        );
        // both providers unhealthy: fail open rather than returning None
        hh.set("b", false);
        assert!(pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false, None).is_some());
    }

    #[test]
    fn pick_untried_skips_open_breaker() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![
                Target {
                    provider: "a".to_string(),
                    model: None,
                    weight: 1,
                },
                Target {
                    provider: "b".to_string(),
                    model: None,
                    weight: 1,
                },
            ],
        };
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::new(true, 1, 60);
        // trip target 0 open: selection must avoid it and pick 1
        assert!(bb.on_failure("m", 0));
        assert_eq!(
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false, None),
            Some(1)
        );
        // both open: fail open to an untried target rather than returning None
        assert!(bb.on_failure("m", 1));
        assert!(pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false, None).is_some());
    }

    /// A key whose owner holds the given access-profile policy.
    fn key_with_policy(policy: rolter_core::ModelPolicy) -> KeyMeta {
        KeyMeta {
            access_policy: Some(policy),
            ..Default::default()
        }
    }

    #[test]
    fn access_policy_denies_a_model_the_key_list_would_allow() {
        let key = key_with_policy(rolter_core::ModelPolicy {
            denied_models: vec!["secret-*".to_string()],
            ..Default::default()
        });
        assert!(key.model_permitted("gpt-4o"));
        assert!(!key.model_permitted("secret-model"));
    }

    #[test]
    fn the_key_list_and_the_access_policy_are_anded() {
        // the key names one model; the owner's profile permits a different one.
        // neither may widen the other, so nothing gets through
        let key = KeyMeta {
            models: vec!["gpt-4o".to_string()],
            access_policy: Some(rolter_core::ModelPolicy {
                allowed_models: vec!["claude-opus".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(!key.model_permitted("gpt-4o"));
        assert!(!key.model_permitted("claude-opus"));

        // and the intersection does pass
        let overlapping = KeyMeta {
            models: vec!["gpt-4o".to_string(), "claude-opus".to_string()],
            access_policy: Some(rolter_core::ModelPolicy {
                allowed_models: vec!["claude-opus".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(overlapping.model_permitted("claude-opus"));
        assert!(!overlapping.model_permitted("gpt-4o"));
    }

    #[test]
    fn no_access_policy_leaves_model_selection_exactly_as_it_was() {
        let key = KeyMeta {
            models: vec!["gpt-4o".to_string()],
            ..Default::default()
        };
        assert!(key.access_policy.is_none());
        assert!(key.model_permitted("gpt-4o"));
        assert!(!key.model_permitted("other"));

        // and an unrestricted key still reaches everything
        let open = KeyMeta::default();
        assert!(open.model_permitted("anything"));
        assert!(open.route_permitted("anything"));
    }

    #[test]
    fn a_denied_route_is_refused_whatever_its_providers_are() {
        let route = ModelRoute {
            model: "internal-tools".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![Target {
                provider: "a".to_string(),
                model: None,
                weight: 1,
            }],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
        };
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1]),
            variant_balancers: Vec::new(),
            route,
        };

        // the key's provider list would admit this route
        let denied = KeyMeta {
            access_policy: Some(rolter_core::ModelPolicy {
                denied_routes: vec!["internal-*".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(denied.provider_allowed("a"));
        assert!(!key_allows_route(&denied, &entry));

        // a policy on the model axis alone leaves the route reachable
        let model_axis_only = KeyMeta {
            access_policy: Some(rolter_core::ModelPolicy {
                denied_models: vec!["internal-*".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(key_allows_route(&model_axis_only, &entry));
    }

    #[test]
    fn provider_policy_excludes_disallowed_targets_without_fail_open() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![
                Target {
                    provider: "a".to_string(),
                    model: None,
                    weight: 1,
                },
                Target {
                    provider: "b".to_string(),
                    model: None,
                    weight: 1,
                },
            ],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
        };
        let entry = crate::state::RouteEntry {
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let key = KeyMeta {
            providers: vec!["b".to_string()],
            ..Default::default()
        };
        let ctx = RouteContext::default();
        assert!(key_allows_route(&key, &entry));
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &[],
                &[],
                &crate::cooldowns::Cooldowns::default(),
                &crate::health::Health::default(),
                &crate::breaker::Breaker::default(),
                "m",
                false,
                Some(&key),
            ),
            Some(1)
        );
        let denied = KeyMeta {
            providers: vec!["missing".to_string()],
            ..Default::default()
        };
        assert!(!key_allows_route(&denied, &entry));
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &[],
                &[],
                &crate::cooldowns::Cooldowns::default(),
                &crate::health::Health::default(),
                &crate::breaker::Breaker::default(),
                "m",
                false,
                Some(&denied),
            ),
            None
        );
    }

    #[test]
    fn model_visibility_allows_only_the_configured_key_or_team() {
        let mut config = config_with_keys();
        config.routes[0]
            .advanced
            .visibility
            .allowed_key_ids
            .push("allowed-key".to_string());
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        let entry = snapshot.routes.get("gpt-4o").unwrap();
        assert!(model_visible_to(
            Some(&KeyMeta {
                id: "allowed-key".to_string(),
                ..Default::default()
            }),
            entry
        ));
        assert!(!model_visible_to(
            Some(&KeyMeta {
                id: "other-key".to_string(),
                ..Default::default()
            }),
            entry
        ));
    }
}
