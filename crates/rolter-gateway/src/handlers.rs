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
use crate::trace::RequestId;
use crate::upstream_failure::{self, StatusFailure};
use rolter_core::failover::Failure;

/// Liveness probe. Stays `200` while draining: the process is healthy, it is
/// just no longer taking new traffic.
pub async fn healthz() -> &'static str {
    "ok"
}

/// Readiness probe. Reports `503` once the control plane has marked this node
/// as draining, so a load balancer stops routing to it while in-flight
/// requests finish (#543), and `503 snapshot_format_unsupported` while a
/// gateway that has never applied a snapshot is refused one for its format,
/// which is what holds a rollout whose new gateways lead the control plane
/// (ADR-0038).
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if state.draining.load(std::sync::atomic::Ordering::Relaxed) {
        return (StatusCode::SERVICE_UNAVAILABLE, "draining");
    }
    // the more specific of the two answers for a gateway with no snapshot:
    // it says why none arrived and what to do about it
    if state.unready_for_snapshot_format() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "snapshot_format_unsupported",
        );
    }
    if state.awaiting_first_snapshot() {
        return (StatusCode::SERVICE_UNAVAILABLE, "awaiting_snapshot");
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
    for entry in snap.routes.values() {
        let Some(counts) = entry.balancer.decisions() else {
            continue;
        };
        // the namespace, which is the route's name unless another org's route
        // shares it, so two orgs' series stay apart (ADR-0040)
        let model: &str = &entry.namespace;
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
    // spec-ref: prometheus-text
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

/// Add the catalog metadata a route sets to its `/v1/models` entry (#2939).
///
/// Only the machine-meaningful fields are served, and only when set, so an
/// entry for a route that sets none is byte-identical to what it always was.
/// `description` is deliberately absent: it is a free-text note for the team
/// and may hold internal routing notes.
fn add_model_metadata(item: &mut Value, advanced: &rolter_core::AdvancedModelConfig) {
    let Some(object) = item.as_object_mut() else {
        return;
    };
    if !advanced.capabilities.is_empty() {
        object.insert("capabilities".to_string(), json!(advanced.capabilities));
    }
    if let Some(model_type) = advanced.model_type.as_deref().filter(|t| !t.is_empty()) {
        object.insert("model_type".to_string(), json!(model_type));
    }
    // zero is not a window; a row written around validation is not advertised
    if let Some(context_length) = advanced.context_length.filter(|n| *n > 0) {
        object.insert("context_length".to_string(), json!(context_length));
    }
}

/// OpenAI-compatible model listing built from configured routes, filtered to
/// the models the caller's virtual key is allowed to see.
pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let snap = state.snapshot.load();
    let vk = match authenticate(&state, &snap, &headers, "/v1/models") {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    // the builtin is shadowed only by a route the caller could resolve: another
    // org's route of that name must not take it away from this one
    let builtin = std::iter::once(fake_llm::MODEL_NAME)
        .filter(|m| snap.named_route_for(m, vk.as_ref()).is_none())
        .map(str::to_string);
    // bare route ids (and the builtin) are owned_by "rolter"
    let mut data: Vec<Value> = snap
        .routes
        .iter()
        .filter(|(_, model, entry)| {
            // only the route this caller's name resolves to: another org's
            // route of the same name is not this caller's (ADR-0040), and an
            // org's route narrowed to another project gives way to the global
            // one (#3546)
            snap.named_route_for(model, vk.as_ref())
                .is_some_and(|resolved| std::ptr::eq(resolved, *entry))
                // the listing only narrows for an authenticated caller; the
                // request path is what refuses an anonymous one
                && vk
                    .as_ref()
                    .is_none_or(|key| authorize_route(Some(key), entry).is_ok())
        })
        .map(|(_, model, entry)| (model.to_string(), Some(&entry.route.advanced)))
        .chain(builtin.map(|model| (model, None)))
        .filter(|(m, _)| vk.as_ref().is_none_or(|vk| vk.model_permitted(m)))
        .map(|(m, advanced)| {
            let mut item = json!({"id": m, "object": "model", "owned_by": "rolter"});
            if let Some(advanced) = advanced {
                add_model_metadata(&mut item, advanced);
            }
            item
        })
        .collect();

    // provider-slug/model ids (ADR-0017): make every provider-addressable model
    // discoverable alongside the route ids. a provider's models are the upstream
    // models named by the configured routes' targets (and variants) *plus* the
    // catalogue its health probe reported, so the listing matches what
    // `Snapshot::resolve_pinned` will actually route rather than the subset a
    // route happens to mention (#1647). owned_by names the provider so a client
    // can group by it. sorted + deduped for a stable listing, and filtered by
    // the same key allow-list
    let key_org = vk.as_ref().map_or("", |vk| vk.org_id.as_str());
    let key_project = vk.as_ref().map_or("", |vk| vk.project_id.as_str());
    // a provider is admitted on its org plus its optional project
    // scope: one scoped to another project is neither addressable nor listed
    // (#1919)
    let admitted_scoped = |tenancy: Option<&rolter_core::Tenancy>| {
        rolter_core::Tenancy::admits_scoped(tenancy, key_org, key_project)
    };
    // another org's providers are neither addressable nor listed (#1844)
    // nor is a slug this caller resolves to another provider (ADR-0040). an
    // org's provider scoped to another project gives way to the global one of
    // its slug, as `Snapshot::resolve_for` does (#3546)
    let name_to_slug: std::collections::HashMap<&str, &str> = snap
        .providers_by_slug
        .iter()
        .filter(|(_, slug, id)| {
            snap.providers_by_slug
                .resolve_admitted(key_org, slug, |id| {
                    snap.providers
                        .get(id.as_str())
                        .is_none_or(|provider| admitted_scoped(provider.tenancy.as_ref()))
                })
                .is_some_and(|(_, own)| std::ptr::eq(own, *id))
        })
        .map(|(_, slug, id)| (id.as_str(), slug))
        .collect();
    // provider name -> upstream models it serves, reused to expand group ids
    let mut provider_models: std::collections::HashMap<&str, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for entry in snap.routes.values() {
        let route = &entry.route;
        // the whole tenancy gate, not the org alone: a route narrowed to
        // another project, or reaching a provider scoped to one, names models
        // this key may not learn from the listing (#1919)
        if !entry.in_tenancy_of(vk.as_ref()) {
            continue;
        }
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
        }
    }
    // widen with each provider's probed catalogue. the cache is empty until the
    // first refresh lands (and stays empty if it is turned off), in which case
    // this is exactly the route-derived listing it always was
    for name in snap.providers.keys() {
        let Some(models) = state.model_catalog.models(name) else {
            continue;
        };
        let known = provider_models.entry(name.as_str()).or_default();
        known.extend(models.iter().cloned());
    }
    let mut pinned: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for (provider, models) in &provider_models {
        let Some(slug) = name_to_slug.get(provider) else {
            continue;
        };
        for upstream in models {
            let id = format!("{slug}/{upstream}");
            if vk
                .as_ref()
                .is_none_or(|vk| vk.model_permitted(&id) && vk.provider_allowed(provider))
            {
                pinned.insert((id, snap.provider_name(provider).to_string()));
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
    for (_, slug, entry) in snap.groups_by_slug.iter() {
        let group = &entry.config;
        let resolved = snap
            .groups_by_slug
            .resolve_admitted(key_org, slug, |own| {
                own.in_tenancy_of(vk.as_ref(), &snap.providers)
            })
            .is_some_and(|(_, own)| std::ptr::eq(own, entry));
        if !resolved {
            continue;
        }
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

    // endpoint support belongs to the resolved address, including pins and
    // groups; dashboards must not infer it from a model's spelling
    for item in &mut data {
        let id = item["id"].as_str().unwrap_or_default();
        let capabilities =
            if id == fake_llm::MODEL_NAME && snap.named_route_for(id, vk.as_ref()).is_none() {
                Some(json!({"decisions": true, "chat": true}))
            } else {
                snap.resolve_for(id, vk.as_ref()).map(|entry| {
                    json!({
                        "decisions": crate::system_one::endpoint_refusal(
                            "/v1/systemone", &entry.route, &snap.providers
                        ).is_none(),
                        "chat": crate::system_one::endpoint_refusal(
                            "/v1/chat/completions", &entry.route, &snap.providers
                        ).is_none()
                    })
                })
            };
        if let Some(capabilities) = capabilities {
            item["rolter_capabilities"] = capabilities;
        }
    }
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

/// Why the data plane refused a caller the model or route it addressed.
///
/// Every variant maps to one OpenAI-style `403`, so the reason a caller is
/// denied is the same whichever endpoint and body encoding they came through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessDenial {
    /// The key's own model allow-list or its owner's access profile does not
    /// permit the requested model id.
    ModelNotAllowed,
    /// The route's visibility restricts it to other teams or keys.
    NotVisible,
    /// The owner's access profile denies the whole named route (#791).
    RoutePolicy,
    /// The key's provider allow-list excludes every provider on the route.
    NoProvider,
    /// The key's provider allow-list excludes the one provider a stored
    /// response lives on, so a lifecycle call cannot be forwarded to it.
    ProviderNotAllowed,
    /// The route a stored response was created on is no longer configured.
    /// A lifecycle call fails closed rather than skip the route checks (#1779).
    RouteRemoved,
}

impl AccessDenial {
    /// The message and machine-readable `code` this denial answers with.
    ///
    /// Split from [`Self::into_response`] so a realtime session that loses
    /// access mid-stream can say the same thing in an `error` event.
    pub(crate) fn message_and_code(self) -> (&'static str, &'static str) {
        match self {
            Self::ModelNotAllowed => ("model not allowed for this key", "model_not_allowed"),
            Self::NotVisible => ("model is not visible to this key", "model_not_allowed"),
            Self::RoutePolicy => (
                "route is not allowed for this key's access profile",
                "route_not_allowed",
            ),
            Self::NoProvider => (
                "no provider on this route is allowed for this key",
                "provider_not_allowed",
            ),
            Self::ProviderNotAllowed => (
                "the provider holding this response is not allowed for this key",
                "provider_not_allowed",
            ),
            Self::RouteRemoved => (
                "the route this response was created on is no longer configured",
                "route_not_allowed",
            ),
        }
    }

    pub(crate) fn into_response(self) -> Response {
        let (message, code) = self.message_and_code();
        crate::error::ApiError::new(StatusCode::FORBIDDEN, message)
            .with_code(code)
            .with_param("model")
            .into_response()
    }
}

/// The model gate every data-plane endpoint applies to the id the caller
/// named, before any budget, plugin, rate limit or route resolution runs.
///
/// Paired with [`authorize_route`]: this one sees only the requested string,
/// that one sees the route it resolved to. A request is authorized only once
/// both have passed.
pub(crate) fn authorize_model(key: Option<&KeyMeta>, model: &str) -> Result<(), AccessDenial> {
    match key {
        Some(key) if !key.model_permitted(model) => Err(AccessDenial::ModelNotAllowed),
        _ => Ok(()),
    }
}

/// The route gate every data-plane endpoint applies once the addressed model
/// has resolved to a route, before anything is sent upstream.
///
/// This is the single authorization contract for a resolved route, shared by
/// the JSON pipeline, the multipart audio uploads and the Realtime upgrade so
/// that access never depends on the request's content type (#1485). The checks
/// run in a fixed order so the caller is told which gate refused them: route
/// visibility, then the owner's route policy, then the key's provider
/// allow-list. The per-target provider filter applied during balancing still
/// runs afterwards; this gate only guarantees that at least one target is
/// reachable at all.
pub(crate) fn authorize_route(
    key: Option<&KeyMeta>,
    entry: &crate::state::RouteEntry,
) -> Result<(), AccessDenial> {
    if !model_visible_to(key, entry) {
        return Err(AccessDenial::NotVisible);
    }
    if let Some(key) = key {
        // checked before the composite gate so the caller is told which of the
        // two denied them: the route policy their owner holds, or the key's
        // own provider allow-list (#791)
        if !key.route_permitted(&entry.route.model) {
            return Err(AccessDenial::RoutePolicy);
        }
        if !key_allows_route(key, entry) {
            return Err(AccessDenial::NoProvider);
        }
    }
    Ok(())
}

/// Enforce the part of model visibility that is available on the gateway
/// request path. User restrictions remain a control-plane authorization
/// concern because a virtual key intentionally carries no user identity.
pub(crate) fn model_visible_to(key: Option<&KeyMeta>, entry: &crate::state::RouteEntry) -> bool {
    // another org's route is never served, whatever its lists say: its targets
    // spend that org's provider credentials (#1844). nor is a route its admin
    // narrowed to another project. resolution already skips both, so this only
    // guards a caller that holds an entry it did not resolve for this key
    if !entry.in_tenancy_of(key) {
        return false;
    }
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
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/chat/completions").await
}

pub async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/completions").await
}

/// OpenAI Responses API. This intentionally shares the JSON proxy pipeline so
/// provider-native fields and SSE events remain byte-for-byte passthrough.
pub async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/responses").await
}

pub async fn retrieve_response(
    state: State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(
        state,
        headers,
        request_id,
        path,
        uri,
        LifecycleOperation::Retrieve,
    )
    .await
}

pub async fn delete_response(
    state: State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(
        state,
        headers,
        request_id,
        path,
        uri,
        LifecycleOperation::Delete,
    )
    .await
}

pub async fn cancel_response(
    state: State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(
        state,
        headers,
        request_id,
        path,
        uri,
        LifecycleOperation::Cancel,
    )
    .await
}

pub async fn response_input_items(
    state: State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    path: Path<String>,
    uri: OriginalUri,
) -> Response {
    response_lifecycle(
        state,
        headers,
        request_id,
        path,
        uri,
        LifecycleOperation::InputItems,
    )
    .await
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

    /// The value of the request-log row's `lifecycle_operation` column.
    fn label(self) -> &'static str {
        match self {
            Self::Retrieve => "retrieve",
            Self::Delete => "delete",
            Self::Cancel => "cancel",
            Self::InputItems => "input_items",
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
    request_id: RequestId,
    Path(response_id): Path<String>,
    OriginalUri(uri): OriginalUri,
    operation: LifecycleOperation,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let started = Instant::now();
    let snap = state.snapshot.load();
    let vk = match authenticate(&state, &snap, &headers, uri.path()) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    let tenant = tenant_scope(vk.as_ref());
    // from here the caller is known, so every answer below is attributable and
    // leaves one request-log row, the same rule a model request follows. a
    // lifecycle call is not billable: the row carries no tokens and no cost,
    // and nothing below charges a budget or a rate-limit window (#2836)
    let scope = request_scope(vk.as_ref());
    let call = LifecycleLog {
        state: &state,
        headers: &headers,
        request_id: request_id.as_str(),
        scope: &scope,
        operation: operation.label(),
        sample_rate: snap.logging.sample_rate,
        started,
    };
    let Some(route) = state.response_registry.get(&tenant, &response_id) else {
        return call.log(None, UpstreamLeg::NotReached, response_not_found());
    };
    // the stored provider is its stable `(org, name)` (#3461), found again by
    // its runtime id in this snapshot, which may have qualified it since
    let provider_id = route.provider_id(&snap);
    // the registry proves the caller created this response, not that it may
    // still reach the route it came from: access revoked since then must stop
    // the retrieve, cancel, delete and input-items calls too (#1779)
    if let Err(denial) = authorize_lifecycle(
        &snap,
        vk.as_ref(),
        &route,
        provider_id.unwrap_or(&route.provider),
    ) {
        return call.log(
            Some(&route),
            UpstreamLeg::NotReached,
            denial.into_response(),
        );
    }
    if !operation.supported(route.capabilities) {
        return call.log(
            Some(&route),
            UpstreamLeg::NotReached,
            crate::error::ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "response lifecycle operation is not supported by the originating provider contract",
            )
            .with_code("response_lifecycle_unsupported")
            .into_response(),
        );
    }
    let Some(provider) = provider_id.and_then(|id| snap.providers.get(id)) else {
        return call.log(Some(&route), UpstreamLeg::NotReached, response_not_found());
    };
    if provider.kind != rolter_core::ProviderKind::Openai {
        return call.log(Some(&route), UpstreamLeg::NotReached, response_not_found());
    }
    let resolved_keys = provider.resolve_api_keys();
    let api_key = match &route.provider_key_fingerprint {
        Some(expected) => resolved_keys
            .into_iter()
            .map(|(key, _)| key)
            .find(|key| provider_key_fingerprint(key) == *expected),
        None if resolved_keys.is_empty() => None,
        None => {
            return call.log(Some(&route), UpstreamLeg::NotReached, response_not_found());
        }
    };
    if route.provider_key_fingerprint.is_some() && api_key.is_none() {
        return call.log(Some(&route), UpstreamLeg::NotReached, response_not_found());
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
            let upstream_status = response.status().as_u16();
            call.log(
                Some(&route),
                UpstreamLeg::Answered(upstream_status),
                lifecycle_response(response, &route, snap.provider_name(&provider.name)),
            )
        }
        Err(err) => {
            let message = err.to_string();
            call.log(
                Some(&route),
                UpstreamLeg::Failed(&message),
                upstream_error_response(&message),
            )
        }
    }
}

/// What a lifecycle call did upstream, for its request-log row.
enum UpstreamLeg<'a> {
    /// The gateway answered without calling the provider: an unknown or
    /// foreign response id, access since revoked, an operation the response's
    /// provider does not support.
    NotReached,
    /// The provider answered with this status.
    Answered(u16),
    /// The provider could not be reached; this is why.
    Failed(&'a str),
}

/// Writes the request-log row for a call on a stored response (#2836).
///
/// `GET`, `DELETE`, `cancel` and `input_items` on `/v1/responses/{id}` used to
/// answer and leave nothing behind, success included, so a lifecycle call that
/// failed upstream was visible only as the `502` its caller saw. The rule that
/// every request from an identified caller leaves one row now holds for them
/// too (see [`Refusals`]).
///
/// The row names the model and the provider and target the response was
/// created on, which come from the stored route, and the operation in
/// `lifecycle_operation`. It has no tokens and no cost, because the call
/// generates nothing, and it is not marked `unpriced` or `usage_unknown` for
/// the same reason. A call the gateway refused names the model but no provider
/// or target and has `attempts = 0`, which is how every refusal row reads as
/// "never reached an upstream"; one that went upstream has `attempts = 1`, the
/// provider's status in `upstream_status` when it answered, and `0` when the
/// connection failed first.
///
/// The latency is the time to the provider's answer, not the time the body
/// takes to reach the caller.
struct LifecycleLog<'a> {
    state: &'a AppState,
    headers: &'a HeaderMap,
    request_id: &'a str,
    scope: &'a ScopeIds,
    operation: &'static str,
    sample_rate: f64,
    started: Instant,
}

impl LifecycleLog<'_> {
    /// Log `response` as this call's outcome and hand it back. `route` is the
    /// stored route when the call got as far as finding one.
    fn log(
        &self,
        route: Option<&crate::response_registry::ResponseRoute>,
        leg: UpstreamLeg<'_>,
        response: Response,
    ) -> Response {
        let model = route.map(|route| route.model.as_str()).unwrap_or_default();
        let model = &model[..model.floor_char_boundary(REFUSAL_MODEL_MAX_BYTES)];
        let ctx = RequestLogCtx::new(
            self.headers,
            self.request_id,
            self.scope,
            model,
            self.sample_rate,
            self.started,
        );
        let mut row = RequestLog {
            status: response.status().as_u16(),
            lifecycle_operation: self.operation.to_string(),
            ..ctx.into_log()
        };
        // a stored identity is the row's display name and org as they are; a
        // runtime id is turned into both as the row is queued
        let provider = |row: &mut RequestLog, route: &crate::response_registry::ResponseRoute| {
            row.provider = route.provider.clone();
            if let Some(org) = &route.provider_org {
                row.provider_org_id = org.clone();
            }
        };
        match (leg, route) {
            (UpstreamLeg::Answered(status), Some(route)) => {
                provider(&mut row, route);
                row.target = route.target.clone();
                row.upstream_status = status;
                row.attempts = 1;
                if status >= 400 {
                    row.error = upstream_failure::upstream_error_text(status, &[]);
                }
            }
            (UpstreamLeg::Failed(message), Some(route)) => {
                provider(&mut row, route);
                row.target = route.target.clone();
                row.attempts = 1;
                row.error = rolter_core::redact::redact_urls_in_text(message);
                row.error
                    .truncate(row.error.floor_char_boundary(REFUSAL_ERROR_MAX_BYTES));
            }
            // a leg that went upstream always has the route it went to; the
            // arm exists so that a refusal reads the same on every path
            _ => {
                row.error = refusal_error(&response);
                row.outcome = Some(refusal_outcome(&response));
            }
        }
        self.state.log.log_lifecycle(row);
        response
    }
}

/// Re-authorize a lifecycle call against the current snapshot, exactly as a
/// fresh request for the same response would be authorized.
///
/// The requested model id must still pass [`authorize_model`], the route that
/// served the response must still pass [`authorize_route`], and the key must
/// still allow the one provider holding the response. The served route is
/// resolved the way the request path resolves an address: a configured route
/// name first, then `provider-slug/model` addressing. When neither resolves the
/// route was removed after the response was created, and the call is refused:
/// with the route gone there is nothing left to check visibility and policy
/// against, and allowing it would let a deleted route keep serving calls for
/// the registry's lifetime.
///
/// `provider_id` is the runtime id the stored provider has in `snap`, which a
/// key's provider allow-list is checked against.
fn authorize_lifecycle(
    snap: &crate::state::Snapshot,
    key: Option<&KeyMeta>,
    route: &crate::response_registry::ResponseRoute,
    provider_id: &str,
) -> Result<(), AccessDenial> {
    authorize_model(key, &route.model)?;
    let entry = snap
        .resolve_for(&route.route, key)
        .ok_or(AccessDenial::RouteRemoved)?;
    authorize_route(key, &entry)?;
    if key.is_some_and(|key| !key.provider_allowed(provider_id)) {
        return Err(AccessDenial::ProviderNotAllowed);
    }
    Ok(())
}

/// `provider` is the display name of the provider holding the response; the
/// route carries its runtime id (ADR-0040).
fn lifecycle_response(
    response: reqwest::Response,
    route: &crate::response_registry::ResponseRoute,
    provider: &str,
) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut upstream_headers = response.headers().clone();
    // `Connection`, every field it nominates and the RFC 9110 set, as the
    // control-plane and MCP relays strip them (#3393). the length is the
    // upstream's framing, and the body is re-streamed
    rolter_proxy::hop_by_hop::strip(&mut upstream_headers);
    upstream_headers.remove(header::CONTENT_LENGTH);
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
            ("x-rolter-provider", provider),
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
    request_id: RequestId,
    OriginalUri(uri): OriginalUri,
    Path(_response_id): Path<String>,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let started = Instant::now();
    let snap = state.snapshot.load();
    let vk = match authenticate(&state, &snap, &headers, uri.path()) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    let scope = request_scope(vk.as_ref());
    // the two routes this serves are fixed, so the label is a closed set and a
    // caller cannot mint one
    let operation = match uri.path().rsplit('/').next() {
        Some("compact") => "compact",
        Some("input_tokens") => "input_tokens",
        _ => "unsupported",
    };
    LifecycleLog {
        state: &state,
        headers: &headers,
        request_id: request_id.as_str(),
        scope: &scope,
        operation,
        sample_rate: snap.logging.sample_rate,
        started,
    }
    .log(
        None,
        UpstreamLeg::NotReached,
        crate::error::ApiError::new(
            StatusCode::NOT_IMPLEMENTED,
            "response lifecycle operations are not supported; Rolter does not route model-less response identifiers",
        )
        .with_code("response_lifecycle_unsupported")
        .into_response(),
    )
}

fn tenant_scope(vk: Option<&KeyMeta>) -> String {
    vk.map(|key| key.tenant_key.clone())
        .unwrap_or_else(|| "anonymous".to_string())
}

fn provider_key_fingerprint(key: &str) -> String {
    rolter_auth::hex::encode(&Sha256::digest(key.as_bytes()))
}

pub async fn messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/messages").await
}

pub async fn embeddings(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/embeddings").await
}

pub async fn rerank(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/rerank").await
}

/// `POST /v1/systemone`: a System One typed decision, forwarded unchanged to a
/// decision server (#3150).
pub async fn system_one(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(
        state,
        headers,
        request_id,
        body,
        rolter_core::upstream::SYSTEM_ONE_PATH,
    )
    .await
}

pub async fn images_generations(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/images/generations").await
}

pub async fn audio_speech(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(state, headers, request_id, body, "/v1/audio/speech").await
}

pub async fn audio_transcriptions(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy_multipart(state, headers, request_id, body, "/v1/audio/transcriptions").await
}

pub async fn audio_translations(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy_multipart(state, headers, request_id, body, "/v1/audio/translations").await
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
            state.metrics.observe_rejection("required_header_missing");
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

/// The 401 for a missing, unknown, disabled or expired virtual key. OpenAI
/// answers all of those with code `invalid_api_key` (only the message differs),
/// and SDKs branch on it; the realtime close sends the same code (#1881).
fn invalid_api_key_json(message: &str) -> Response {
    crate::error::ApiError::new(StatusCode::UNAUTHORIZED, message)
        .with_code("invalid_api_key")
        .into_response()
}

/// Tenant identity forwarded to a guardrail webhook or plugin as metadata.
/// Shared by both call sites so the envelope always carries the same shape.
pub(crate) fn plugin_tenant(scope: &ScopeIds) -> rolter_core::WebhookTenant {
    rolter_core::WebhookTenant {
        org: (!scope.org.is_empty()).then(|| scope.org.clone()),
        team: (!scope.team.is_empty()).then(|| scope.team.clone()),
        project: (!scope.project.is_empty()).then(|| scope.project.clone()),
        key: (!scope.key.is_empty()).then(|| scope.key.clone()),
    }
}

/// Preserve a queue-admission failure as a client-actionable overload response
/// instead of flattening it into a generic upstream 502 after failover is
/// exhausted. Other forwarding failures keep their existing gateway-error form,
/// with every URL in the transport error masked: `reqwest` quotes the address it
/// could not reach, userinfo and credential query values included, and the
/// caller is not the operator who configured that address (#2919).
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
    error_json(
        StatusCode::BAD_GATEWAY,
        &rolter_core::redact::redact_urls_in_text(message),
    )
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

/// The peppered digest of the key a request presented, the handle a live
/// session keeps to look its key up again ([`recheck_session_access`]).
pub(crate) fn presented_key_digest(snap: &Snapshot, headers: &HeaderMap) -> Option<String> {
    extract_key(headers).map(|key| rolter_auth::hash_key(&snap.pepper, &key))
}

/// Why a live realtime session lost the access it was opened with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccessRevoked {
    pub(crate) status: StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

/// Re-run the upgrade's key, model and route gates against `snap` for a
/// session that is already open (#1881), returning the key as `snap` now has
/// it.
///
/// Authentication happens once, at the WebSocket upgrade, so without this a
/// disabled, expired or deleted key would keep its session until the client
/// left. The gates are the upgrade's own: the key lookup [`authenticate`] does,
/// then [`authorize_model`], the route lookup and [`authorize_route`]. On top
/// of those, `provider` is the one the session is pinned to, and it must still
/// be on the key's provider allow-list (#2384): the route gate only asks that
/// some provider on the route is, and the session cannot move to another.
pub(crate) fn recheck_session_access<'a>(
    snap: &'a Snapshot,
    digest: &str,
    model: &str,
    provider: &str,
) -> Result<&'a KeyMeta, AccessRevoked> {
    let denied = |denial: AccessDenial| {
        let (message, code) = denial.message_and_code();
        AccessRevoked {
            status: StatusCode::FORBIDDEN,
            code,
            message: message.to_string(),
        }
    };
    let key = match snap.keys.get(digest) {
        Some(key) if key.is_active(Utc::now()) => key,
        // revoked, expired and deleted read alike, as they do at the upgrade
        Some(_) | None => {
            return Err(AccessRevoked {
                status: StatusCode::UNAUTHORIZED,
                code: "invalid_api_key",
                message: "invalid api key".to_string(),
            })
        }
    };
    authorize_model(Some(key), model).map_err(denied)?;
    let Some(entry) = snap.named_route_for(model, Some(key)) else {
        return Err(AccessRevoked {
            status: StatusCode::NOT_FOUND,
            code: "model_not_found",
            message: format!("no route for model '{model}'"),
        });
    };
    authorize_route(Some(key), entry).map_err(denied)?;
    if !key.provider_allowed(provider) {
        return Err(AccessRevoked {
            status: StatusCode::FORBIDDEN,
            code: "provider_not_allowed",
            message: "the provider this session is pinned to is no longer allowed for this key"
                .to_string(),
        });
    }
    Ok(key)
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
        // not even have. otherwise the deployment mode decides. nothing in the
        // snapshot does: the only thing a control plane could add here is a
        // way to open a managed gateway, and that stays a decision for
        // whoever runs the process (#2357)
        let required = snap.require_auth.unwrap_or(state.managed_auth);
        if required {
            state.metrics.observe_rejection("missing_key");
            return Err(invalid_api_key_json(MISSING_KEY_MESSAGE));
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
                    state.metrics.observe_rejection("invalid_key");
                    Err(invalid_api_key_json(&invalid_key_message(&key)))
                }
            }
        }
        None => {
            state.metrics.observe_rejection("missing_key");
            Err(invalid_api_key_json(MISSING_KEY_MESSAGE))
        }
    }
}

/// Shared proxy pipeline: parse, authenticate, balance, forward, stream back.
/// Stage 2 of the proxy pipeline: the scope identity a request is attributed
/// to (#1041).
///
/// Drives budget enforcement and log attribution both, which is why it is
/// resolved once and shared rather than derived at each use. An unauthenticated
/// request has no scope, and [`ScopeIds::default`] is the unattributed one.
pub(crate) fn request_scope(vk: Option<&KeyMeta>) -> ScopeIds {
    vk.map(|v| ScopeIds {
        org: v.org_id.clone(),
        team: v.team_id.clone(),
        project: v.project_id.clone(),
        key: v.id.clone(),
        business_unit: v.business_unit_id.clone(),
        customer: v.customer_id.clone(),
        user: v.user_id.clone(),
    })
    .unwrap_or_default()
}

/// Stage 3 of the proxy pipeline: refuse before spending upstream tokens when
/// any applicable budget is already spent (#1041).
///
/// Returns the refusal to hand back, or `None` to continue. Shared by `proxy`,
/// `proxy_multipart` and the realtime upgrade, which enforce this identically
/// and used to be kept in step by hand.
pub(crate) async fn budget_refusal(
    state: &AppState,
    snap: &Snapshot,
    scope: &ScopeIds,
) -> Option<Response> {
    let exceeded = state.budgets.exceeded(&snap.budgets, scope).await?;
    state.metrics.budget_blocks_total.fetch_add(1, Relaxed);
    Some(
        crate::error::ApiError::new(
            StatusCode::PAYMENT_REQUIRED,
            budget_exceeded_message(&exceeded),
        )
        .with_code("insufficient_quota")
        .into_response(),
    )
}

/// The wording of a budget refusal, shared with the realtime session that a
/// spent budget closes mid-session (#1396), so a client reads the same
/// sentence whichever way it met the cap.
pub(crate) fn budget_exceeded_message(exceeded: &rolter_core::BudgetConfig) -> String {
    format!(
        "budget exceeded for {:?} '{}' (limit ${:.2})",
        exceeded.scope, exceeded.id, exceeded.limit_usd
    )
}

/// The throughput stage of the proxy pipeline: refuse with 429 and
/// `Retry-After` when a matching request or token window is already at
/// capacity. Admission also charges the request to every applicable `rpm`
/// window.
///
/// Shared by `proxy`, `proxy_multipart` and the realtime upgrade (#1396), so
/// a realtime session is limited per key, team, project and org exactly like
/// an HTTP request rather than by the process-local session cap alone.
pub(crate) async fn rate_limit_refusal(
    state: &AppState,
    snap: &Snapshot,
    scope: &ScopeIds,
) -> Option<Response> {
    let hit = state.rate_limiter.check(&snap.rate_limits, scope).await?;
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
    Some(resp)
}

/// The route's own throughput stage (#2936): refuse with 429 and `Retry-After`
/// when the route's `advanced.limits` caps are spent, else return the slot of
/// its `concurrency` cap to hold for the request, if it sets one.
///
/// The caps are the route's, whoever calls: they sit on top of the caller's
/// limits, which [`rate_limit_refusal`] has already admitted. `concurrency` is
/// taken first and costs nothing to refuse; `rpm` and `tpm` follow, so a
/// request turned away for want of a slot never takes an `rpm` slot. A request
/// refused here has still taken one from the caller's own `rpm`.
pub(crate) async fn route_limit_refusal(
    state: &AppState,
    entry: &crate::state::RouteEntry,
) -> Result<Option<crate::route_limits::ConcurrencyPermit>, Box<Response>> {
    let limits = &entry.route.advanced.limits;
    // the common route sets no cap: nothing is looked up, allocated or counted
    if limits.concurrency.is_none() && entry.throughput.is_none() {
        return Ok(None);
    }
    let route = entry.route.model.as_str();
    let mut permit = None;
    if let Some(cap) = limits.concurrency {
        let org = entry
            .route
            .tenancy
            .as_ref()
            .map_or("", |tenancy| tenancy.org_id.as_str());
        permit = state.route_concurrency.try_acquire(org, route, cap);
        if permit.is_none() {
            state.metrics.observe_route_limit(route, "concurrency");
            return Err(Box::new(route_limit_response(
                format!("route '{route}' is serving its limit of {cap} concurrent requests"),
                1,
            )));
        }
    }
    if let Some(window) = &entry.throughput {
        if let Some(hit) = state.rate_limiter.check_route(window).await {
            state.metrics.observe_route_limit(route, hit.kind);
            return Err(Box::new(route_limit_response(
                format!(
                    "{} rate limit exceeded for route '{route}' (limit {}/min)",
                    hit.kind, hit.limit
                ),
                hit.retry_after,
            )));
        }
    }
    Ok(permit)
}

fn route_limit_response(message: String, retry_after: u64) -> Response {
    let mut resp = crate::error::ApiError::new(StatusCode::TOO_MANY_REQUESTS, message)
        .with_code("rate_limit_exceeded")
        .into_response();
    resp.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    resp
}

/// What a pipeline stage hands on: the value the stages after it need, or the
/// answer that ends the request here. An answer is a refusal the stage has
/// already logged, or a response it served itself (the built-in model, a
/// cache hit), so it is no error and the pipeline returns it as it is.
#[must_use]
enum Stage<T> {
    Next(T),
    Answer(Response),
}

/// The value a [`Stage`] passed on, or an early return of its answer.
macro_rules! proceed {
    ($stage:expr) => {
        match $stage {
            Stage::Next(value) => value,
            Stage::Answer(answer) => return answer,
        }
    };
}

/// What every stage of a request pipeline reads once the caller is known: the
/// snapshot the request is served under, the endpoint, the caller's key, and
/// the [`Refusals`] that logs whatever a stage turns away (#1041).
///
/// Shared by [`proxy`] and [`proxy_multipart`]. It holds only borrows and the
/// refusal log's plugin events, so a stage is handed `&RequestCtx` and nothing
/// is cloned to call it.
///
/// `'s` is the gateway's state and the snapshot, which outlive the request;
/// `'r` is what the handler owns for this request (the key, its scope, the
/// model name), which it moves into the log row once the stages are done.
struct RequestCtx<'s, 'r> {
    state: &'s AppState,
    snap: &'s Snapshot,
    /// the endpoint the request was sent to
    path: &'s str,
    vk: Option<&'r KeyMeta>,
    refuse: Refusals<'r>,
}

impl<'s, 'r> RequestCtx<'s, 'r> {
    fn state(&self) -> &'s AppState {
        self.state
    }

    /// The caller's scope (empty for an anonymous request).
    fn scope(&self) -> &'r ScopeIds {
        self.refuse.scope
    }

    /// The model the caller asked for.
    fn model(&self) -> &'r str {
        self.refuse.model
    }

    /// Log `refusal` as this request's outcome and hand it back.
    fn refused(&self, refusal: Response) -> Response {
        self.refuse.log(refusal)
    }

    /// The model gate: the key may ask for this model at all.
    fn admit_model(&self) -> Stage<()> {
        match authorize_model(self.vk, self.model()) {
            Ok(()) => Stage::Next(()),
            Err(denial) => Stage::Answer(self.refused(denial.into_response())),
        }
    }

    /// Refuse before spending upstream tokens when any applicable budget is
    /// already spent.
    async fn admit_budget(&self) -> Stage<()> {
        self.refuse_if(budget_refusal(self.state(), self.snap, self.scope()).await)
    }

    /// The throughput cap: reject before forwarding when a matching
    /// request/token window is already at capacity (admission also counts the
    /// request).
    async fn admit_rate(&self) -> Stage<()> {
        self.refuse_if(rate_limit_refusal(self.state(), self.snap, self.scope()).await)
    }

    /// Refuse before the upstream call when the operator will not serve
    /// traffic they cannot account for (#974).
    fn admit_unpriced(&self, model: &str, priced: bool) -> Stage<()> {
        self.refuse_if(unpriced_admission(
            self.state(),
            self.snap,
            self.scope(),
            model,
            priced,
        ))
    }

    /// Consult the request-stage plugins of `stage` (`pre_route` or
    /// `pre_upstream`) for this key's org and project, in position order
    /// (#509). Block rejects; transform rewrites `parsed` in place, and the
    /// stage passes on whether it changed. `route` is the resolved route's
    /// model, empty before one is resolved. The calls made are noted for
    /// every row the request leaves (#2243).
    async fn request_plugins(
        &self,
        stage: rolter_core::PluginStage,
        route: &str,
        parsed: &mut Value,
    ) -> Stage<bool> {
        let scope = self.scope();
        let plugins = self.snap.plugins.for_stage(
            stage,
            &scope.org,
            (!scope.project.is_empty()).then_some(scope.project.as_str()),
        );
        if plugins.is_empty() {
            return Stage::Next(false);
        }
        let state = self.state();
        let tenant = plugin_tenant(scope);
        let mut events = Vec::new();
        let outcome = crate::plugin_dispatch::dispatch(
            &plugins,
            stage,
            &state.metrics,
            &state.side_client,
            self.model(),
            route,
            self.refuse.request_id,
            &tenant,
            &mut events,
            parsed,
        )
        .await;
        self.refuse.note_plugin_events(events);
        match outcome {
            crate::plugin_dispatch::DispatchOutcome::Allow(content) => {
                let transformed = content != *parsed;
                *parsed = content;
                Stage::Next(transformed)
            }
            crate::plugin_dispatch::DispatchOutcome::Block(reason) => Stage::Answer(
                self.refused(
                    crate::error::ApiError::new(
                        StatusCode::BAD_REQUEST,
                        reason.unwrap_or_else(|| "request blocked by plugin".to_string()),
                    )
                    .with_code("plugin_blocked")
                    .into_response(),
                ),
            ),
        }
    }

    /// The built-in `fake-llm` model answers locally, with `reply`, unless a
    /// configured route shadows it for this caller; another org's route of
    /// that name does not. Any other model goes on.
    fn builtin(&self, reply: impl FnOnce() -> fake_llm::Reply) -> Stage<()> {
        let model = self.model();
        if model != fake_llm::MODEL_NAME || self.snap.named_route_for(model, self.vk).is_some() {
            return Stage::Next(());
        }
        let refuse = &self.refuse;
        let ctx = RequestLogCtx::new(
            refuse.headers,
            refuse.request_id,
            refuse.scope,
            model,
            refuse.sample_rate,
            refuse.started,
        );
        Stage::Answer(log_builtin(&self.state().log, ctx, reply()))
    }

    /// The answer when no route serves the model in the caller's tenancy.
    fn route_miss(&self) -> Response {
        self.refused(
            crate::error::ApiError::new(
                StatusCode::NOT_FOUND,
                format!("no route for model '{}'", self.model()),
            )
            .with_code("model_not_found")
            .with_param("model")
            .into_response(),
        )
    }

    /// The gates on the route that serves the request: it has something to
    /// dial, the key may use it, and a decision model is not asked a chat or
    /// upload endpoint it has no answer for (#3150). An upload must not reach
    /// a route the caller could not address with a chat request (#1485).
    fn admit_route(&self, entry: &crate::state::RouteEntry) -> Stage<()> {
        if entry.route.targets.is_empty() && !entry.route.has_variants() {
            return Stage::Answer(self.refused(error_json(
                StatusCode::SERVICE_UNAVAILABLE,
                "route has no targets",
            )));
        }
        if let Err(denial) = authorize_route(self.vk, entry) {
            return Stage::Answer(self.refused(denial.into_response()));
        }
        self.refuse_if(
            crate::system_one::endpoint_refusal(self.path, &entry.route, &self.snap.providers)
                .map(IntoResponse::into_response),
        )
    }

    /// The route's own caps, once the route is known to be the one that
    /// serves the request; passes on the permit to hold to the end of the
    /// response (#2936).
    async fn route_permit(
        &self,
        entry: &crate::state::RouteEntry,
    ) -> Stage<Option<crate::route_limits::ConcurrencyPermit>> {
        match route_limit_refusal(self.state(), entry).await {
            Ok(permit) => Stage::Next(permit),
            Err(refusal) => Stage::Answer(self.refused(*refusal)),
        }
    }

    /// Refuse a `max_tokens` (or its newer names) above the route's output
    /// token limit.
    fn output_token_cap(&self, entry: &crate::state::RouteEntry, parsed: &Value) -> Stage<()> {
        let Some(max_output) = entry.route.advanced.limits.output_tokens else {
            return Stage::Next(());
        };
        for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
            if parsed
                .get(field)
                .and_then(Value::as_u64)
                .is_some_and(|value| value > u64::from(max_output))
            {
                return Stage::Answer(
                    self.refused(
                        crate::error::ApiError::new(
                            StatusCode::BAD_REQUEST,
                            format!(
                                "{field} exceeds this model's output token limit of {max_output}"
                            ),
                        )
                        .with_code("max_tokens_exceeded")
                        .with_param(field)
                        .into_response(),
                    ),
                );
            }
        }
        Stage::Next(())
    }

    /// Versioned prompt templates: render the immutable versions active for
    /// this route and inject their decorator messages around the caller's own
    /// messages (ROL-256). Variable substitution is structural (values land as
    /// inert JSON strings); an unknown, missing or oversized variable rejects
    /// with a client error. The template-variables field is rolter-only and
    /// never reaches an upstream: a body that carried it loses it whether or
    /// not a template renders. Passes on whether the body must be
    /// re-serialized.
    fn prompt_templates(
        &self,
        entry: &crate::state::RouteEntry,
        parsed: &mut Value,
    ) -> Stage<bool> {
        let scope = self.scope();
        let mut template_applied = false;
        let carried_template_vars = parsed
            .get(rolter_core::prompt_templates::TEMPLATE_VARS_FIELD)
            .is_some();
        let template_scope = rolter_core::PromptTemplateRequestScope {
            route_model: &entry.route.model,
            org_id: &scope.org,
            project_id: &scope.project,
            virtual_key_id: &scope.key,
        };
        let metrics = &self.state().metrics;
        if self.snap.prompt_templates.active_for_scope(&template_scope) {
            match crate::prompt_templates::apply_for_scope(
                &self.snap.prompt_templates,
                &template_scope,
                self.path,
                parsed,
            ) {
                Ok(report) => {
                    if report.decorations > 0 {
                        template_applied = true;
                        metrics
                            .prompt_template_decorations_total
                            .fetch_add(report.decorations as u64, Relaxed);
                    }
                }
                Err(err) => {
                    metrics
                        .prompt_template_rejections_total
                        .fetch_add(1, Relaxed);
                    return Stage::Answer(
                        self.refused(
                            crate::error::ApiError::new(StatusCode::BAD_REQUEST, err.message())
                                .with_code("invalid_prompt_template")
                                .into_response(),
                        ),
                    );
                }
            }
        } else if carried_template_vars {
            if let Some(object) = parsed.as_object_mut() {
                object.remove(rolter_core::prompt_templates::TEMPLATE_VARS_FIELD);
            }
        }
        Stage::Next(template_applied | carried_template_vars)
    }

    /// Built-in guardrails: scan request content before proxying (ROL-261).
    /// A block rule rejects with an OpenAI-compatible error; redactions
    /// rewrite `parsed` in place, and the stage passes on whether any did.
    /// What each rule did is appended to `events` for the request's rows
    /// (#2158); raw matches are never logged. The route's override decides
    /// which rules apply, and a route that disables every rule skips the JSON
    /// walk entirely (#590).
    fn builtin_guardrails(
        &self,
        entry: &crate::state::RouteEntry,
        parsed: &mut Value,
        events: &mut Vec<String>,
    ) -> Stage<bool> {
        let guardrails = &self.snap.guardrails;
        if !guardrails.pre_call_active_for(&entry.guardrails) {
            return Stage::Next(false);
        }
        let metrics = &self.state().metrics;
        match crate::guardrails::apply_input(guardrails, &entry.guardrails, self.path, parsed) {
            Ok(report) => {
                *events = report.events();
                if report.redactions > 0 {
                    metrics
                        .guardrail_redactions_total
                        .fetch_add(report.redactions as u64, Relaxed);
                }
                Stage::Next(report.redactions > 0)
            }
            Err(blocked) => {
                metrics.guardrail_blocks_total.fetch_add(1, Relaxed);
                // the rules that ran before the blocking one may already have
                // redacted this request, and the rollup should count that
                let rule = blocked.rule;
                let events = blocked.report.events();
                Stage::Answer(
                    self.refuse.log_with_events(
                        crate::error::ApiError::new(
                            StatusCode::BAD_REQUEST,
                            format!("request blocked by guardrail '{rule}'"),
                        )
                        .with_code("guardrail_blocked")
                        .into_response(),
                        events,
                    ),
                )
            }
        }
    }

    /// The semantic guardrail policy for this request (#459): the most
    /// specific enabled policy for this key, route, project or org, else the
    /// providers no policy lists, picks the providers that judge it.
    /// `post_call_only` keeps a policy only when it runs on the response,
    /// for a request no request-stage provider can read. `trace_id` is the
    /// id the providers are called under.
    fn semantic_guard(
        &self,
        entry: &crate::state::RouteEntry,
        trace_id: &str,
        post_call_only: bool,
    ) -> Option<crate::semantic_guards::SemanticGuard<'s>> {
        let (state, snap) = (self.state, self.snap);
        let scope = self.scope();
        snap.semantic_guards
            .resolve(&crate::semantic_guards::policy_request(
                scope,
                &entry.route.model,
            ))
            .filter(|policy| !post_call_only || policy.runs_at(rolter_core::WebhookStage::PostCall))
            .map(|policy| crate::semantic_guards::SemanticGuard {
                guards: &snap.semantic_guards,
                policy,
                metrics: &state.metrics,
                egress: &state.side_client,
                local: &state.local_classifiers,
                model: self.model().to_string(),
                route: entry.route.model.clone(),
                trace_id: trace_id.to_string(),
                tenant: plugin_tenant(scope),
            })
    }

    /// The semantic policy's pre_call providers (#459): block answers with an
    /// OpenAI-compatible error; transform replaces the request body, and the
    /// stage passes on whether one did; a call with no verdict resolves per
    /// that provider's fail-open/closed mode. Only the assembled envelope is
    /// sent; content is never logged.
    async fn guardrail_webhook(
        &self,
        guard: Option<&crate::semantic_guards::SemanticGuard<'_>>,
        parsed: &mut Value,
        events: &mut Vec<String>,
    ) -> Stage<bool> {
        let Some(guard) = guard.filter(|guard| guard.runs_at(rolter_core::WebhookStage::PreCall))
        else {
            return Stage::Next(false);
        };
        match guard
            .check(rolter_core::WebhookStage::PreCall, parsed, false, events)
            .await
        {
            crate::semantic_guards::Verdict::Allow(None) => Stage::Next(false),
            crate::semantic_guards::Verdict::Allow(Some(content)) => {
                *parsed = content;
                Stage::Next(true)
            }
            crate::semantic_guards::Verdict::Block { reason, .. } => Stage::Answer(
                self.refuse.log_with_events(
                    crate::error::ApiError::new(StatusCode::BAD_REQUEST, reason)
                        .with_code("guardrail_blocked")
                        .into_response(),
                    std::mem::take(events),
                ),
            ),
        }
    }

    /// The external PII sanitizer's request leg (#848). Unlike the webhook,
    /// it returns transformed *content* (entities replaced by deterministic
    /// placeholders) plus an opaque ticket the same service accepts back to
    /// reverse the transform. The mapping never enters this process, so
    /// there is nothing here to leak into a log, metric or span. Passes on
    /// whether the body changed and the ticket, if any.
    async fn sanitize_request(
        &self,
        entry: &crate::state::RouteEntry,
        pii_scope: &rolter_core::TokenScope,
        will_restore: bool,
        parsed: &mut Value,
    ) -> Stage<(bool, Option<rolter_core::RestorationTicket>)> {
        let config = &self.snap.pii_sanitizer;
        if !config.sanitizes_request() {
            return Stage::Next((false, None));
        }
        let state = self.state();
        match crate::pii_sanitizer::sanitize(
            config,
            &state.metrics,
            &state.side_client,
            "request",
            self.model(),
            &entry.route.model,
            self.refuse.request_id,
            plugin_tenant(self.scope()),
            pii_scope.clone(),
            will_restore,
            parsed,
        )
        .await
        {
            crate::pii_sanitizer::SanitizeOutcome::Unchanged => Stage::Next((false, None)),
            crate::pii_sanitizer::SanitizeOutcome::Sanitized {
                content,
                ticket,
                replaced,
            } => {
                // `replaced > 0` short-circuits the deep compare for the case
                // that matters; a service that rewrote the body while reporting
                // nothing is still honoured, it just costs a comparison
                let sanitized = replaced > 0 || content != *parsed;
                *parsed = content;
                Stage::Next((sanitized, ticket))
            }
            crate::pii_sanitizer::SanitizeOutcome::Block(reason) => Stage::Answer(
                self.refused(
                    crate::error::ApiError::new(StatusCode::BAD_GATEWAY, reason)
                        .with_code("pii_sanitizer_unavailable")
                        .into_response(),
                ),
            ),
        }
    }

    /// The built-in post_call rules for this route (#591). They mask the
    /// response body, which means buffering it in full, so a streamed request
    /// is refused under `streaming_post_call = "reject"` (the default) and
    /// otherwise served without them; the caller decides what a stream that
    /// got here gets.
    fn output_guard<'e>(
        &self,
        entry: &'e crate::state::RouteEntry,
        stream: bool,
    ) -> Stage<Option<crate::guardrails::OutputGuard<'e>>>
    where
        's: 'e,
    {
        let (state, snap) = (self.state, self.snap);
        let guard = crate::guardrails::OutputGuard::resolve(
            &snap.guardrails,
            &entry.guardrails,
            &state.metrics,
            self.path,
        );
        if guard.is_some()
            && stream
            && snap.guardrails.streaming_post_call() == rolter_core::StreamingPostCall::Reject
        {
            return Stage::Answer(self.refused(output_rules_stream_refusal(state)));
        }
        Stage::Next(guard)
    }

    /// The semantic policy's post_call providers judge a whole response, and
    /// the policy, never the caller's `stream`, says what a stream gets:
    /// refused here before the upstream call, or buffered in full and judged
    /// once (ADR-0037).
    fn admit_semantic_leg(&self, leg: Option<SemanticLeg<'_, '_>>) -> Stage<()> {
        match leg.filter(|leg| leg.rejects_stream()) {
            Some(leg) => Stage::Answer(self.refused(policy_stream_refusal(self.state, leg.guard))),
            None => Stage::Next(()),
        }
    }

    /// Refuse a stream the sanitizer's `streaming` setting rejects
    /// (#848, #911).
    fn admit_pii_stream(&self, mode: Option<rolter_core::StreamingResponse>) -> Stage<()> {
        if mode == Some(rolter_core::StreamingResponse::Reject) {
            return Stage::Answer(self.refused(pii_stream_refusal(self.state)));
        }
        Stage::Next(())
    }

    /// The post_response plugins for this key's org and project (#509). A
    /// plugin judges a whole body and a stream never is one, so a stream is
    /// refused when any of them is fail closed ("never deliver what I have not
    /// approved"); a fail-open plugin has already chosen delivery without its
    /// verdict, which is all a stream would get (#1776). The chain is built
    /// only when `build` is set, and only when a plugin applies.
    fn post_response_plugins(
        &self,
        entry: &crate::state::RouteEntry,
        stream: bool,
        build: bool,
        trace_id: &str,
    ) -> Stage<Option<crate::plugin_dispatch::PostResponsePlugins<'s>>> {
        let (state, scope) = (self.state, self.scope());
        let plugins = self.snap.plugins.for_stage(
            rolter_core::PluginStage::PostResponse,
            &scope.org,
            (!scope.project.is_empty()).then_some(scope.project.as_str()),
        );
        if stream {
            if let Some(refusal) = plugin_stream_refusal(state, &plugins) {
                return Stage::Answer(self.refused(refusal));
            }
        }
        Stage::Next((build && !plugins.is_empty()).then(|| {
            crate::plugin_dispatch::PostResponsePlugins {
                plugins,
                metrics: &state.metrics,
                egress: &state.side_client,
                model: self.model().to_string(),
                route: entry.route.model.clone(),
                trace_id: trace_id.to_string(),
                tenant: plugin_tenant(scope),
            }
        }))
    }

    /// Answer with `refusal`, logged, when there is one; else go on.
    fn refuse_if(&self, refusal: Option<Response>) -> Stage<()> {
        match refusal {
            Some(refusal) => Stage::Answer(self.refused(refusal)),
            None => Stage::Next(()),
        }
    }
}

/// The body sent upstream: the caller's, untouched, unless something must be
/// re-serialized: the route forwards as another model, sets param defaults
/// (temperature, max_tokens, ...) the override policy merges over the
/// caller's, or a stage `rewrote` the parsed body. Falls back to the
/// untouched body if re-serialization fails.
fn forward_body(
    entry: &crate::state::RouteEntry,
    model: &str,
    parsed: &Value,
    body: &Bytes,
    rewrote: bool,
) -> Bytes {
    let effective_model = &entry.route.model;
    let rewrite_model = effective_model != model;
    let has_forward_params = entry.route.params.keys().any(|key| key != POLICY_PARAM);
    if !has_forward_params && !rewrite_model && !rewrote {
        return body.clone();
    }
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
}

/// The streamed restore of a stream the sanitizer restores through its
/// streaming endpoint (#911). It needs a token for this request's own scope;
/// with none (nothing was replaced, or the service cannot reverse) there is
/// nothing to restore and the stream is served as it is.
fn pii_stream_restore(
    state: &AppState,
    config: &rolter_core::PiiSanitizerConfig,
    ticket: Option<&rolter_core::RestorationTicket>,
    scope: &rolter_core::TokenScope,
    trace_id: &str,
    tenant: &rolter_core::WebhookTenant,
) -> Option<crate::pii_stream::StreamRestore> {
    let token = ticket.and_then(|ticket| {
        let token = ticket.token_for(scope);
        if token.is_none() {
            // as `pii_sanitizer::restore` does: never reached today, and
            // counted if a change ever makes it reachable
            state.metrics.pii_restore_errors_total.fetch_add(1, Relaxed);
            tracing::warn!(
                "refusing to restore a pii token outside the tenant and route that produced it"
            );
        }
        token
    })?;
    Some(crate::pii_stream::StreamRestore::new(
        config,
        &state.side_client,
        &state.metrics,
        token,
        trace_id,
        tenant.clone(),
    ))
}

/// Complexity-tier selection: the route's policy picks a tier route by the
/// request's already-bounded byte count, and never retains prompt text. If an
/// operator removes a tier route or a key cannot access it, the requested
/// route is kept and that safe fallback is exposed in bounded telemetry.
fn complexity_tier<'e>(
    snap: &'e Snapshot,
    vk: Option<&KeyMeta>,
    metrics: &crate::metrics::Metrics,
    entry: &'e crate::state::RouteEntry,
    request_bytes: usize,
) -> &'e crate::state::RouteEntry {
    let Some((tier, tier_route)) =
        rolter_balancer::complexity::ComplexityPolicy::from_params(&entry.route.params)
            .ok()
            .flatten()
            .and_then(|policy| policy.select(request_bytes).cloned())
            .map(|tier| (tier.name, tier.route))
    else {
        return entry;
    };
    let selected = snap.named_route_for(&tier_route, vk).filter(|candidate| {
        (!candidate.route.targets.is_empty() || candidate.route.has_variants())
            && authorize_route(vk, candidate).is_ok()
    });
    let fallback = selected.is_none();
    let chosen = selected.unwrap_or(entry);
    metrics.observe_complexity(&entry.namespace, &tier, &chosen.namespace, fallback);
    chosen
}

async fn proxy(
    state: AppState,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
    path: &str,
) -> Response {
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
    let scope = request_scope(vk.as_ref());
    // from here the caller is known, so a refusal is attributable and is
    // logged. the three answers above are not: they come before any key was
    // looked up, and a row nobody owns is one anonymous traffic could write
    // without limit (#2807)
    let req = RequestCtx {
        state: &state,
        snap: &snap,
        path,
        vk: vk.as_ref(),
        refuse: Refusals {
            state: &state,
            headers: &headers,
            request_id: request_id.as_str(),
            scope: &scope,
            model: &model,
            sample_rate: snap.logging.sample_rate,
            started,
            plugin_events: parking_lot::Mutex::new(Vec::new()),
        },
    };
    let refuse = &req.refuse;
    proceed!(req.admit_model());

    // block before spending upstream tokens when any applicable budget is spent
    proceed!(req.admit_budget().await);

    // pre_route plugins: consult before the route is resolved, while only the
    // key's scope and raw request are known (#509)
    let trace_id = request_id.as_str();
    let pre_route_transformed = proceed!(
        req.request_plugins(rolter_core::PluginStage::PreRoute, "", &mut parsed)
            .await
    );

    // throughput cap: reject before forwarding when a matching request/token
    // window is already at capacity (admission also counts the request)
    proceed!(req.admit_rate().await);

    // built-in fake-llm answers locally unless a configured route shadows it
    // for this caller; another org's route of that name does not
    proceed!(req.builtin(|| fake_llm::respond(path, &parsed)));

    // route-name-first: a named route wins, even one whose name contains '/'.
    // only on a miss do we try `provider-slug/model` addressing (ADR-0017),
    // which pins a provider and forwards `model` as the upstream model through
    // the same classic-pool machinery (owned entry held here). a route outside
    // the caller's tenancy is a miss, so it shadows nothing and the caller
    // gets the unknown-model answer rather than learning it exists
    //
    // route resolution through the key's access checks is one attributable
    // stage; `strategy` and `candidates` are recorded once the route is known
    let route_stage = crate::trace::stage_span!(
        "route.select",
        route = tracing::field::Empty,
        strategy = tracing::field::Empty,
        candidates = tracing::field::Empty
    );
    let resolved = snap.resolve_for(&model, vk.as_ref());
    let Some(requested) = resolved.as_deref() else {
        return req.route_miss();
    };
    // what the requested route's metric series are labelled by: its runtime
    // namespace, which tells two orgs' same-named routes apart (#3462). shared
    // with the route's entry, so each row takes a reference, not a copy
    let route_namespace = std::sync::Arc::clone(&requested.namespace);
    let entry = complexity_tier(&snap, vk.as_ref(), &state.metrics, requested, body.len());
    proceed!(req.admit_route(entry));
    // the route survived every access check: record what was chosen and close
    // the stage, so what follows is not nested under route selection
    route_stage.record("route", entry.route.model.as_str());
    route_stage.record("strategy", tracing::field::debug(&entry.route.strategy));
    route_stage.record("candidates", entry.route.targets.len());
    drop(route_stage);

    proceed!(req.output_token_cap(entry, &parsed));

    // the route's own caps, once the route is known to be the one that serves
    // the request. the permit is held to the end of the response (#2936)
    let mut route_permit = proceed!(req.route_permit(entry).await);

    // versioned prompt templates, before guardrails so any appended
    // user/assistant content is still scanned (ROL-256)
    let template_applied = proceed!(req.prompt_templates(entry, &mut parsed));

    // both the built-in scan and the webhook consult are pre-call guardrail work,
    // so they share one stage: a request slowed by a sluggish guardrail service
    // shows up here rather than inside the upstream leg. never carries the
    // matched content — only whether a rule fired
    let guardrails_pre_stage = crate::trace::stage_span!(
        "guardrails.pre",
        redacted = tracing::field::Empty,
        webhook = !snap.semantic_guards.is_empty()
    );

    // what each guardrail did to this request, for its log row (#2158)
    let mut guardrail_events: Vec<String> = Vec::new();
    // built-in guardrails, then the semantic policy's pre_call providers
    let guardrail_redacted =
        proceed!(req.builtin_guardrails(entry, &mut parsed, &mut guardrail_events));
    let semantic_guard = req.semantic_guard(entry, trace_id, false);
    let webhook_transformed = proceed!(
        req.guardrail_webhook(semantic_guard.as_ref(), &mut parsed, &mut guardrail_events)
            .await
    );

    // external PII sanitizer (#848)
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
    let (pii_sanitized, pii_ticket) = proceed!(
        req.sanitize_request(entry, &pii_scope, will_restore, &mut parsed)
            .await
    );

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
    let plugin_transformed = proceed!(
        req.request_plugins(
            rolter_core::PluginStage::PreUpstream,
            &entry.route.model,
            &mut parsed,
        )
        .await
    );
    // what the plugin stages did, for the rows this request leaves (#2243)
    let plugin_events = refuse.plugin_events();
    guardrails_pre_stage.record(
        "redacted",
        guardrail_redacted
            || webhook_transformed
            || plugin_transformed
            || pre_route_transformed
            || pii_sanitized,
    );
    drop(guardrails_pre_stage);

    let effective_model = entry.route.model.clone();
    let forward_body = forward_body(
        entry,
        &model,
        &parsed,
        &body,
        guardrail_redacted
            || webhook_transformed
            || plugin_transformed
            || pre_route_transformed
            || template_applied
            || pii_sanitized,
    );

    // capture log fields independent of the chosen target
    let stream = parsed
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    // post-call rules mask the response body, which means buffering it in
    // full. read after the webhook stage so it judges the body that will
    // actually be sent upstream (#591)
    let output_guard = proceed!(req.output_guard(entry, stream));
    // a stream that got here is one the operator opted out of masking for
    let output_guard = if stream { None } else { output_guard };

    // there is no setting that streams with the policy's post_call providers
    // skipped (ADR-0037)
    let semantic_leg = semantic_guard
        .as_ref()
        .filter(|guard| guard.runs_at(rolter_core::WebhookStage::PostCall))
        .map(|guard| SemanticLeg { guard, stream });
    proceed!(req.admit_semantic_leg(semantic_leg));

    // the sanitizer's buffered response leg needs the whole body for the same
    // reason the output guardrails do: an entity — or a placeholder — can
    // straddle any SSE chunk boundary, and neither detection nor reversal is
    // correct on a fragment. `streaming` decides what a stream gets: refused,
    // passed through, or — for restoration only — restored through the
    // sanitizer's streaming endpoint (#848, #911)
    let pii_touches_response = snap.pii_sanitizer.touches_response(restore_requested);
    let pii_stream_mode = if stream {
        snap.pii_sanitizer.stream_handling(restore_requested)
    } else {
        None
    };
    proceed!(req.admit_pii_stream(pii_stream_mode));
    // a stream that got here is one the operator opted out of the response leg
    // for, or one restored below; the request leg still ran, so PII still did
    // not reach the provider
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
        // the same decision the log row makes below; with capture off the
        // pre-restore body has no reader, so it is never kept (#3240)
        capture: payload_capture_enabled(&snap.logging.payload_capture, &model, &scope.key),
    });
    // a streamed restore, taken by whichever path answers
    let mut pii_stream = if pii_stream_mode == Some(rolter_core::StreamingResponse::Restore) {
        pii_stream_restore(
            &state,
            &snap.pii_sanitizer,
            pii_ticket.as_ref(),
            &pii_scope,
            &pii_trace_id,
            &pii_tenant,
        )
    } else {
        None
    };
    // set by the ensure_request_id middleware, under whichever header the
    // snapshot names
    let request_id = request_id.as_str().to_string();
    // adopt the caller's distributed trace when one was propagated inbound
    let trace_id = crate::trace::request_trace_id(&headers);

    // post_response plugins: same buffering constraint as output guardrails
    // above; a stream that got here keeps streaming without them
    let post_response_plugins =
        proceed!(req.post_response_plugins(entry, stream, !stream, &trace_id));
    // the trace backend needs the same tenant identity the request log carries,
    // so a collector can route this org's spans to this org's destination (#836)
    crate::trace::record_tenant(&scope.org, &scope.team, &scope.project);
    // records this request's tokens against its rate limits once usage is known
    let token_recorder = TokenRecorder::new(
        state.rate_limiter.clone(),
        snap.rate_limits.clone(),
        scope.clone(),
    )
    .with_route(entry.throughput.clone());
    let price = snap.prices.get(&effective_model).cloned();
    // refuse before the upstream call when the operator will not serve traffic
    // they cannot account for (#974)
    proceed!(req.admit_unpriced(&effective_model, price.is_some()));
    // who asked for what, which every row the request leaves from here starts
    // from: the scope identity (empty for config-defined keys) and the
    // governance dimensions the key's spend rolls up to (#539)
    let ident = RequestLogCtx {
        request_id,
        trace_id,
        vk_id: scope.key.clone(),
        org_id: scope.org.clone(),
        team_id: scope.team.clone(),
        project_id: scope.project.clone(),
        business_unit_id: scope.business_unit.clone(),
        customer_id: scope.customer.clone(),
        model,
        route_namespace: Some(route_namespace),
        sample_rate: snap.logging.sample_rate,
        started,
        guardrail_events,
        plugin_events,
    };
    // records this request's cost against its budgets once cost_usd is known
    let recorder = SpendRecorder::new(state.budgets.clone(), snap.budgets.clone(), scope);
    let model = ident.model.as_str();

    let session_key = headers.get("x-session-id").and_then(|v| v.to_str().ok());
    // prefix affinity reads the conversation, not the JSON envelope every
    // request to the model shares (#1851); a shape it cannot read keeps the
    // raw body, as before
    let affinity = crate::prompt_affinity::affinity_for(entry, path, &parsed);
    let token_ids = parse_vllm_token_ids(&headers);
    // the affinity text is only the prompt's leading bytes; route_context also
    // hands over the whole prompt's length and digest, which the predictor's
    // token estimate and consistent_hash need
    let ctx = crate::prompt_affinity::route_context(
        affinity.as_ref(),
        // the raw body stands in for a prompt this cannot read, but only on a
        // route whose balancers read one (#1929)
        entry.reads_prompt().then_some(&body[..]),
        session_key,
        token_ids.as_deref(),
        // adapter identity only exists when the request addresses something
        // other than the route's own model — i.e. a passthrough provider-group
        // route, which is how vLLM addresses LoRA adapters over shared base
        // weights. On a single-model route the two are equal and this stays
        // None, which keeps adapter scoring inert: treating one model as an
        // adapter would pin the whole route to whichever target served first
        (model != effective_model).then_some(model),
    );
    // trace context plus any client headers the operator allowlisted (#564).
    //
    // this is the *fallback* set, used when no OTLP pipeline is installed: it
    // copies the caller's trace headers verbatim, as before #805. With a
    // pipeline installed the trace context is re-injected per attempt from
    // inside that attempt's `upstream.request` span, so the provider call
    // parents to the gateway rather than to the caller (see
    // `attempt_trace_context`)
    let forwarded_names = state.forwarder.forwarded_header_names();
    let trace_ctx = crate::trace::outbound_headers(&headers, &forwarded_names);
    let trace_headers: Vec<(&str, &str)> = trace_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let fwd = Forward {
        state: &state,
        snap: &snap,
        entry,
        model,
        ctx: &ctx,
        payload: ForwardPayload::Json {
            parsed: &parsed,
            body: &body,
            forward_body: &forward_body,
        },
        path,
        started,
        trace_headers: &trace_headers,
        inbound: (&headers, &forwarded_names),
        key_meta: vk.as_ref(),
    };

    // every delivery of this request runs the same post-response policy,
    // whether the answer is live, a cacheable miss or a hit (#1477)
    let policy = DeliveryPolicy {
        output: output_guard.as_ref(),
        semantic: semantic_leg,
        plugins: post_response_plugins.as_ref(),
        pii: pii_leg.as_ref(),
        transcript: false,
    };
    let cache = proceed!(
        cache_lookup(
            &state,
            &fwd,
            &forward_body,
            stream,
            &ident,
            policy,
            &mut pii_stream
        )
        .await
    );

    // from here the handler is waiting on an upstream that may take minutes. if
    // the caller hangs up in that window axum drops this future and the request
    // would vanish with no row at all, so an armed guard owns the row until a
    // path that logs it takes over (#1083)
    let mut cancel = crate::cancel::CancelGuard::new(
        state.log.clone(),
        started,
        RequestLog {
            stream: stream as u8,
            ..ident.clone().row()
        },
    );
    let mut out = forward(&fwd, &mut cancel).await;
    // the slot rides on the guard, so a streamed response keeps it until its
    // last byte; without a guard no response outlives this function
    if let Some(guard) = out.inflight_guard.as_mut() {
        guard.hold_route_permit(route_permit.take());
    }

    let capture_payloads =
        payload_capture_enabled(&snap.logging.payload_capture, model, &ident.vk_id);
    // from here every arm answers the caller and logs its row: the streamed
    // path hands it to `UsageLoggingStream` (which marks it if the client then
    // leaves), and each failure arm below writes its own
    cancel.disarm();
    // token counts, latency and ttft are filled by the stream wrapper once
    // the body has been fully forwarded to the client
    let Some((mut response, status, is_sse)) = out.outcome.take() else {
        return unanswered(&state, &snap, ident, out, stream);
    };
    // what translation did to the caller's parameters, decided before
    // the upstream was called, so it heads a stream too (#1586)
    let param_adjustments = response
        .extensions_mut()
        .remove::<rolter_proxy::ParamAdjustments>()
        .unwrap_or_default();
    let translation = if status < 400 {
        translation_plan(&snap, path, &out.last_provider, &out.last_target)
    } else {
        rolter_proxy::TranslationPlan::passthrough()
    };
    let response_observer = if path == "/v1/responses" && status < 400 {
        response_observer(&state, &snap, entry, vk.as_ref(), &mut out, model, is_sse)
    } else {
        None
    };
    let ForwardOutcome {
        last_provider,
        last_target,
        variant,
        first_variant,
        sampled_variant,
        attempts,
        fallback_reason,
        inflight_guard,
        genai_span,
        ..
    } = out;
    let log = RequestLog {
        // the org that owns the provider in the snapshot that served
        // the request; the row keeps the runtime id until it is queued
        provider_org_id: snap.provider_label(&last_provider).1.to_string(),
        provider: last_provider,
        target: last_target,
        variant,
        first_variant,
        sampled_variant,
        status,
        upstream_status: status,
        attempts,
        fallback_reason,
        stream: stream as u8,
        unit_basis: crate::unit_billing::UnitBasis::for_json(path, &parsed),
        request_shape: crate::request_shape::RequestShape::of(path, &parsed),
        capture_payloads,
        payload_max_bytes: snap.logging.payload_capture.max_bytes,
        payload_redactor: snap.payload_redactor.clone(),
        request_payload: if capture_payloads {
            crate::logging::capture_payload(
                &forward_body,
                snap.logging.payload_capture.max_bytes,
                &snap.payload_redactor,
            )
        } else {
            String::new()
        },
        param_adjustments,
        ..ident.row()
    };
    let acct = Accounting {
        started,
        sink: state.log.clone(),
        price,
        log,
        recorder,
        token_recorder,
        inflight_guard,
        genai_span,
    };
    // a request that did not ask for a stream keeps the legs that judge
    // a whole body: the built-in output rules, the post_response
    // plugins and the sanitizer's response leg. none of them can read
    // an event stream (a non-JSON body is delivered untouched), so an
    // upstream that streams anyway would get its answer past them on
    // both the live and the cacheable path. it is withheld instead,
    // still billed from the body it sent, rather than buffered and
    // shown to legs that would pass it unread (#3471)
    if is_sse && !stream && status < 400 && policy.has_whole_body_leg() {
        return withhold_unrequested_stream(response, &translation, acct).await;
    }
    let answered = (response, status, is_sse);
    // on a cache-eligible route, buffer a successful response (JSON or a
    // full SSE stream) so it can be stored in Redis, then replay the
    // buffered bytes through the same accounting stream (byte-for-byte
    // identical token/cost/log handling to the uncached path)
    if let Some(store) = cache.store.filter(|_| status < 400) {
        return store_and_deliver(
            answered,
            &translation,
            acct,
            response_observer,
            policy,
            store,
            pii_stream,
        )
        .await;
    }
    let (response, _, is_sse) = answered;
    stream_response(
        response,
        is_sse,
        translation,
        acct,
        response_observer,
        policy,
        pii_stream.take(),
    )
    .await
}

/// Forward through the route's variant chain when it has variants, else
/// through its classic pool.
async fn forward(fwd: &Forward<'_>, cancel: &mut crate::cancel::CancelGuard) -> ForwardOutcome {
    if fwd.entry.route.has_variants() {
        forward_variants(fwd, cancel).await
    } else {
        forward_classic(fwd, cancel).await
    }
}

/// How a successful answer from `provider` is translated back to the
/// dialect the caller spoke on `path`.
fn translation_plan(
    snap: &Snapshot,
    path: &str,
    provider: &str,
    target: &str,
) -> rolter_proxy::TranslationPlan {
    snap.providers
        .get(provider)
        .map(|provider| {
            rolter_proxy::TranslationPlan::resolve(
                path,
                provider.kind,
                provider.role_profile_for(Some(target)),
            )
        })
        .unwrap_or_else(rolter_proxy::TranslationPlan::passthrough)
}

/// What records a successful `/v1/responses` answer in the response
/// registry, so a later lifecycle call can find its provider, target and
/// key (#2836). Takes the attempt's key fingerprint out of `out`.
fn response_observer(
    state: &AppState,
    snap: &Snapshot,
    entry: &crate::state::RouteEntry,
    vk: Option<&KeyMeta>,
    out: &mut ForwardOutcome,
    model: &str,
    is_sse: bool,
) -> Option<crate::logging::CompletionObserver> {
    let capabilities = snap
        .providers
        .get(&out.last_provider)
        .map(|provider| provider.kind == rolter_core::ProviderKind::Openai)
        .unwrap_or(false);
    let template = state.response_registry.template(
        tenant_scope(vk),
        out.last_provider.clone(),
        out.last_target.clone(),
        model.to_string(),
        out.provider_key_fingerprint.take(),
        if capabilities {
            crate::response_registry::LifecycleCapabilities::NATIVE_OPENAI
        } else {
            crate::response_registry::LifecycleCapabilities::UNSUPPORTED
        },
    )?;
    // the provider's stable identity, not its runtime id, which changes once
    // another org shares the name (#3461)
    let (name, org) = snap.provider_label(&out.last_provider);
    let template = template
        .with_route(entry.route.model.clone())
        .with_provider_identity(name.to_string(), org.to_string());
    let registry = state.response_registry.clone();
    Some(Box::new(move |body: &[u8]| {
        registry.record_body(template, is_sse, body);
    }) as crate::logging::CompletionObserver)
}

/// What the cache lookup leaves for the response stage: where to store a
/// successful answer, when the route caches.
struct CacheMiss<'a> {
    store: Option<CacheStore<'a>>,
}

/// Where a cacheable miss is stored once it has been buffered.
struct CacheStore<'a> {
    /// what holds the cache and counts what it stores
    state: &'a AppState,
    key: String,
    ttl_secs: u64,
    max_entry_bytes: u64,
    /// the semantic index the answer joins, when the route's semantic cache
    /// embedded the request (#1476)
    semantic: Option<SemanticStore>,
}

struct SemanticStore {
    index_key: String,
    entry_id: String,
    embedding: Vec<f32>,
    max_candidates: usize,
}

/// The exact-match and semantic response cache (ROL-56; streaming added in
/// ROL-235), one `cache.lookup` stage: a cache that is slow to answer is as
/// interesting as one that misses, and both delay the same request.
///
/// Eligible for routes that opted in, when the global switch is on and Redis
/// is wired. Both non-streaming JSON and streaming SSE responses are
/// cacheable: a streaming miss is buffered in full, stored, and replayed
/// instantly on a later hit. The key hashes the post-injection forward body,
/// so admin param defaults are part of the identity; per-key routes also mix
/// in the key id. A virtual key may override its route's cache decision
/// (ROL-235 part 2): `Some(false)` opts the key out, `Some(true)` opts it in
/// even on a route that did not; `None` inherits the route flag. The global
/// switch still gates all of it. A hit is answered under `policy`, as a live
/// answer would be, and takes `pii_stream`. `state` is `fwd`'s, apart so
/// the store it passes on outlives the request's own borrows.
async fn cache_lookup<'a>(
    state: &'a AppState,
    fwd: &Forward<'_>,
    forward_body: &Bytes,
    stream: bool,
    ident: &RequestLogCtx,
    policy: DeliveryPolicy<'_, '_>,
    pii_stream: &mut Option<crate::pii_stream::StreamRestore>,
) -> Stage<CacheMiss<'a>> {
    let Forward {
        snap,
        entry,
        path,
        key_meta,
        ..
    } = *fwd;
    let cache_stage = crate::trace::stage_span!(
        "cache.lookup",
        hit = tracing::field::Empty,
        kind = tracing::field::Empty
    );
    let cache_ttl = entry.route.cache_ttl_secs(snap.cache.default_ttl_secs);
    let cache_eligible = path != "/v1/responses"
        && key_meta
            .and_then(|k| k.cache_override)
            .unwrap_or_else(|| entry.route.cache_enabled());
    if !(snap.cache.enabled && state.response_cache.is_enabled() && cache_eligible) {
        // every lookup that could answer this request has missed
        cache_stage.record("hit", false);
        return Stage::Next(CacheMiss { store: None });
    }
    let scope_seg = cache_scope(entry, &ident.vk_id);
    let key = ResponseCache::make_key(&snap.cache.namespace, path, scope_seg, forward_body);
    if let Some(hit) = state.response_cache.get(&key).await {
        state.metrics.cache_hits_total.fetch_add(1, Relaxed);
        cache_stage.record("hit", true);
        cache_stage.record("kind", "exact");
        return Stage::Answer(
            cached_response(hit, &state.log, ident.clone(), policy, pii_stream.take()).await,
        );
    }
    state.metrics.cache_misses_total.fetch_add(1, Relaxed);
    let mut semantic_store = None;
    if let Some(semantic) = entry.route.cache.as_ref().and_then(|c| c.semantic.as_ref()) {
        // only shapes whose whole meaning is in their text are looked up, and
        // only against candidates that agree on every other field (#1476)
        let embedded = match crate::semantic::classify(path, forward_body) {
            Some(request) => semantic_embedding(state, snap, semantic, &request.text)
                .await
                .map(|embedding| (request, embedding)),
            None => None,
        };
        if let Some((request, embedding)) = embedded {
            let index_key = ResponseCache::semantic_index_key(
                &snap.cache.namespace,
                path,
                &entry.route.model,
                scope_seg,
                &request.partition,
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
                .filter(|hit| crate::semantic::replay_matches_mode(stream, &hit.content_type))
            {
                state
                    .metrics
                    .semantic_cache_hits_total
                    .fetch_add(1, Relaxed);
                cache_stage.record("hit", true);
                cache_stage.record("kind", "semantic");
                return Stage::Answer(
                    cached_response(hit, &state.log, ident.clone(), policy, pii_stream.take())
                        .await,
                );
            }
            state
                .metrics
                .semantic_cache_misses_total
                .fetch_add(1, Relaxed);
            semantic_store = Some(SemanticStore {
                index_key,
                entry_id: key.clone(),
                embedding,
                max_candidates: semantic.max_candidates,
            });
        }
    }
    // every lookup that could answer this request has missed
    cache_stage.record("hit", false);
    Stage::Next(CacheMiss {
        store: Some(CacheStore {
            state,
            key,
            ttl_secs: cache_ttl,
            max_entry_bytes: snap.cache.max_entry_bytes,
            semantic: semantic_store,
        }),
    })
}

/// Buffer a successful, cacheable answer, store what the upstream said, then
/// deliver it under the request's policy through the same accounting stream
/// as a live answer (byte-for-byte identical token/cost/log handling).
/// Streaming hits later replay these frames instantly; the final-chunk usage
/// is still parsed because the frames are handed on with `is_sse` preserved.
async fn store_and_deliver(
    (response, status, is_sse): (reqwest::Response, u16, bool),
    translation: &rolter_proxy::TranslationPlan,
    mut acct: Accounting,
    observer: Option<crate::logging::CompletionObserver>,
    policy: DeliveryPolicy<'_, '_>,
    store: CacheStore<'_>,
    mut pii_stream: Option<crate::pii_stream::StreamRestore>,
) -> Response {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => return body_read_failed(&acct.sink, acct.log, acct.started, &err),
    };
    let bytes = translation.translate_response(bytes, is_sse);
    // what the provider charged, read before any policy below can rewrite or
    // withhold the body (#1478)
    let billed = crate::logging::parse_usage_with(is_sse, &bytes, translation.thinking_count());
    // skip storing bodies over the configured ceiling (0 = no limit); they're
    // still served normally
    let metrics = &store.state.metrics;
    let limit = store.max_entry_bytes;
    if limit == 0 || bytes.len() as u64 <= limit {
        let cached = CachedResponse {
            status,
            content_type: content_type.clone(),
            body: bytes.to_vec(),
            param_adjustments: acct.log.param_adjustments.log_entries(),
        };
        let cache = &store.state.response_cache;
        cache.put(&store.key, &cached, store.ttl_secs).await;
        if let Some(semantic) = store.semantic {
            cache
                .semantic_put(
                    &semantic.index_key,
                    &semantic.entry_id,
                    semantic.embedding,
                    &cached,
                    store.ttl_secs,
                    semantic.max_candidates,
                )
                .await;
            metrics.semantic_cache_stores_total.fetch_add(1, Relaxed);
        }
        metrics.cache_stores_total.fetch_add(1, Relaxed);
    } else {
        metrics.cache_too_large_total.fetch_add(1, Relaxed);
    }
    // the whole post-response policy runs on the way out, never on the way
    // in: the entry stored above is what the upstream said, so a restore is
    // never persisted, and every later delivery of it runs its own request's
    // policy (see `cached_response`, #848, #1477)
    let status_code = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    // the units a unit-priced endpoint is billed by (an image count) come from
    // the upstream body too, before a plugin rewrites or a guard withholds it
    // (#3404)
    acct.log.billed_units = acct.log.unit_basis.units(&bytes);
    let mut post_events = PolicyEvents::default();
    let applied = policy.apply(status_code, &mut post_events, bytes).await;
    post_events.record(&mut acct.log);
    let (bytes, pre_restore) = match applied {
        Delivery::Send { body, pre_restore } => (body, pre_restore),
        // the upstream call happened and is billed on a refusal as on a
        // delivery (#1478)
        Delivery::Refuse { response, reason } => {
            return withheld_response(response, reason, billed, is_sse, acct)
        }
    };
    buffered_response(
        Buffered {
            bytes,
            pre_restore,
            billed,
            status,
            content_type,
            is_sse,
        },
        acct,
        observer,
        pii_stream.take(),
    )
}

/// The answer and its one row when no upstream answered (#2807): the last
/// attempt never got a response, every target it was left with answered a
/// retryable status, or the balancer had nothing to pick. Shared by both
/// pipelines.
fn unanswered(
    state: &AppState,
    snap: &Snapshot,
    ident: RequestLogCtx,
    out: ForwardOutcome,
    stream: bool,
) -> Response {
    let ForwardOutcome {
        last_provider,
        last_target,
        last_attempt_recorded,
        last_error,
        last_status,
        variant,
        first_variant,
        sampled_variant,
        attempts,
        fallback_reason,
        ..
    } = out;
    // which of the three ends this is decides the answer and what the row
    // says; all three log exactly one row
    let no_target = last_error.is_none() && last_status.is_none();
    let (response, status, error, upstream_status) = match (last_error, last_status) {
        // the last attempt never got a response
        (Some(message), _) => {
            let response = upstream_error_response(&message);
            // what the caller was given, which is not always a 502: a
            // full provider queue answers 429 or 503
            let status = response.status().as_u16();
            (
                response,
                status,
                rolter_core::redact::redact_urls_in_text(&message),
                0,
            )
        }
        // the last attempt was answered with a retryable status and no
        // target was left. the response was dropped by the loop, so
        // this is where its status survives: a 429 stays a rate limit
        // for the caller instead of becoming "no target selected"
        (None, Some(failed)) => {
            let answer = upstream_failure::exhausted(
                &ident.model,
                &failed,
                attempts,
                snap.cooldown.enabled(),
            );
            (answer.response, answer.status, answer.error, failed.status)
        }
        // the balancer had nothing to pick, so no upstream was tried
        (None, None) => (
            error_json(StatusCode::SERVICE_UNAVAILABLE, "no target selected"),
            StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            "no target selected".to_string(),
            0,
        ),
    };
    let latency_ms = ident.started.elapsed().as_millis() as u32;
    let row = RequestLog {
        provider_org_id: if no_target {
            String::new()
        } else {
            snap.provider_label(&last_provider).1.to_string()
        },
        provider: if no_target {
            String::new()
        } else {
            last_provider
        },
        target: if no_target {
            String::new()
        } else {
            last_target
        },
        variant,
        first_variant,
        sampled_variant,
        status,
        upstream_status,
        attempts,
        fallback_reason,
        stream: stream as u8,
        latency_ms,
        error,
        ..ident.row()
    };
    if no_target {
        // nothing was tried, so there is no target to charge
        state.log.log_refusal(row);
    } else if last_attempt_recorded {
        // the loop ran out of targets after a superseded attempt: that
        // attempt is already counted against its target, so this row
        // carries the request only (#1646)
        state.log.log_recorded_attempt(row);
    } else {
        state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
        state.log.log(row);
    }
    response
}

/// Multipart sibling of [`proxy`] for the audio upload endpoints
/// (`/v1/audio/transcriptions`, `/v1/audio/translations`). The body is
/// `multipart/form-data`, so the JSON pipeline can't parse it: instead the
/// `model` is read from the form fields and the raw body is forwarded verbatim
/// (content-type + boundary preserved) via [`Forwarder::forward_raw`]. Auth,
/// the model and route authorization gates ([`authorize_model`],
/// [`authorize_route`]), budgets, rate limits, routing, retries, cooldowns and
/// the circuit breaker match the chat path, variant routing included: a route
/// with variants samples one and dials its targets through
/// [`forward_variants`], as chat does (#3494). Per-model and per-variant param
/// injection do not apply (they're JSON-only), and the target's upstream model
/// name is not rewritten into the multipart body.
async fn proxy_multipart(
    state: AppState,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
    path: &str,
) -> Response {
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
    // one pass over the upload for every field the handler reads: `stream`
    // is usually absent, and looking for it alone would scan the file part
    let [model, response_format, stream_field] =
        crate::multipart::text_fields(&body, &boundary, ["model", "response_format", "stream"]);
    let model = match model {
        Some(m) => m,
        None => {
            return crate::error::ApiError::new(StatusCode::BAD_REQUEST, "missing model field")
                .with_code("missing_required_parameter")
                .with_param("model")
                .into_response()
        }
    };
    // `gpt-4o-transcribe` and `gpt-4o-mini-transcribe` answer `stream=true`
    // with `transcript.text.*` events; the response stage treats such a
    // request as the chat path treats `"stream": true` (#3432)
    // spec-ref: openai-audio
    let stream = stream_field.is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));

    let snap = state.snapshot.load();

    let vk = match authenticate(&state, &snap, &headers, path) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    let scope = request_scope(vk.as_ref());
    // as on the chat path: a refusal of a caller we know is logged (#2807)
    let req = RequestCtx {
        state: &state,
        snap: &snap,
        path,
        vk: vk.as_ref(),
        refuse: Refusals {
            state: &state,
            headers: &headers,
            request_id: request_id.as_str(),
            scope: &scope,
            model: &model,
            sample_rate: snap.logging.sample_rate,
            started,
            plugin_events: parking_lot::Mutex::new(Vec::new()),
        },
    };
    proceed!(req.admit_model());
    proceed!(req.admit_budget().await);
    proceed!(req.admit_rate().await);

    // built-in fake-llm answers locally unless a configured route shadows it
    // for this caller; another org's route of that name does not
    proceed!(req.builtin(|| {
        fake_llm::Reply::untokenized(fake_llm::transcription(response_format.as_deref()))
    }));

    // the same resolution as the JSON pipeline: a named route in the caller's
    // tenancy first, then `provider-slug/model` addressing (ADR-0017)
    let resolved = snap.resolve_for(&model, vk.as_ref());
    let Some(entry) = resolved.as_deref() else {
        return req.route_miss();
    };
    // the series label of the route, as on the JSON pipeline (#3462)
    let route_namespace = std::sync::Arc::clone(&entry.namespace);
    // the same route gates as the JSON pipeline (#1485, #3150)
    proceed!(req.admit_route(entry));
    // the route's own caps, as on the JSON pipeline (#2936)
    let mut route_permit = proceed!(req.route_permit(entry).await);

    let request_id = request_id.as_str().to_string();
    let trace_id = crate::trace::request_trace_id(&headers);
    // the trace backend needs the same tenant identity the request log carries,
    // so a collector can route this org's spans to this org's destination (#836)
    crate::trace::record_tenant(&scope.org, &scope.team, &scope.project);
    let token_recorder = TokenRecorder::new(
        state.rate_limiter.clone(),
        snap.rate_limits.clone(),
        scope.clone(),
    )
    .with_route(entry.throughput.clone());
    let price = snap.prices.get(&model).cloned();

    // the response stage of the guardrails, on the transcript (#3331). the
    // request is audio, which no request-stage rule, provider, plugin or
    // sanitizer reads, so those do not run; the answer is text, so everything
    // that judges a chat completion judges it, in the same order: the
    // built-in post_call rules, the semantic policy's post_call providers, the
    // post_response plugins, then the sanitizer's response leg. no
    // placeholder went upstream, so nothing is restored. a streamed
    // transcription gets what a streamed chat completion gets, refused before
    // the upstream call or served as the chat path serves it (#3432). the
    // refusals before the call run in the chat path's order, the sanitizer's
    // stream check before the plugins' and the unpriced policy last, so a
    // request two of them would refuse gets the same answer on both (#3538)
    let output_guard = proceed!(req.output_guard(entry, stream));
    let semantic_guard = req.semantic_guard(entry, &trace_id, true);
    let semantic_leg = semantic_guard
        .as_ref()
        .map(|guard| SemanticLeg { guard, stream });
    // `reject` refuses before dialling; `buffer` holds the stream up to the
    // cap, judges its transcript text and replays it (`stream_response`)
    proceed!(req.admit_semantic_leg(semantic_leg));
    // the log row takes these before the response leg runs
    let pii_model = model.clone();
    let pii_trace_id = trace_id.clone();
    let pii_scope = rolter_core::TokenScope {
        org: scope.org.clone(),
        team: scope.team.clone(),
        project: scope.project.clone(),
        route: entry.route.model.clone(),
    };
    let pii_sanitizes = snap.pii_sanitizer.sanitizes_response();
    if stream && pii_sanitizes {
        proceed!(req.admit_pii_stream(snap.pii_sanitizer.stream_handling(false)));
    }
    // the legs a stream cannot be shown to are built whatever the form field
    // says, and set aside only once the upstream has actually answered with
    // an event stream: a model that ignores `stream` answers JSON, which is
    // judged like any other transcript, so the caller cannot switch a leg
    // off by asking for a stream (#3432)
    let post_response_plugins = proceed!(req.post_response_plugins(entry, stream, true, &trace_id));
    // see the chat path: refuse unaccountable traffic before spending upstream
    // tokens on it (#974)
    proceed!(req.admit_unpriced(&model, price.is_some()));
    let pii_leg = pii_sanitizes.then(|| PiiResponseLeg {
        state: &state,
        config: &snap.pii_sanitizer,
        model: &pii_model,
        route: &entry.route.model,
        trace_id: &pii_trace_id,
        tenant: plugin_tenant(&scope),
        scope: &pii_scope,
        ticket: None,
        will_restore: false,
        capture: payload_capture_enabled(&snap.logging.payload_capture, &model, &scope.key),
    });

    // who asked for what, as on the chat path; an upload's rows carry no
    // request-stage guardrail or plugin events, since none ran
    let ident = RequestLogCtx {
        request_id,
        trace_id,
        vk_id: scope.key.clone(),
        org_id: scope.org.clone(),
        team_id: scope.team.clone(),
        project_id: scope.project.clone(),
        business_unit_id: scope.business_unit.clone(),
        customer_id: scope.customer.clone(),
        model,
        route_namespace: Some(route_namespace),
        sample_rate: snap.logging.sample_rate,
        started,
        guardrail_events: Vec::new(),
        plugin_events: Vec::new(),
    };
    let recorder = SpendRecorder::new(state.budgets.clone(), snap.budgets.clone(), scope);
    let model = ident.model.as_str();

    let token_ids = parse_vllm_token_ids(&headers);
    let ctx = RouteContext::new()
        .with_session_key(headers.get("x-session-id").and_then(|v| v.to_str().ok()))
        .with_token_ids(token_ids.as_deref())
        // see the chat path: an adapter only exists when the request addresses
        // something other than the route's own model
        .with_adapter((model != entry.route.model).then_some(model));
    // trace context plus any client headers the operator allowlisted (#564);
    // the fallback set, as on the chat path, for when no pipeline is installed
    let forwarded_names = state.forwarder.forwarded_header_names();
    let trace_ctx = crate::trace::outbound_headers(&headers, &forwarded_names);
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
            stream: u8::from(stream),
            ..ident.clone().row()
        },
    );

    // a route with variants dials only them, as on the chat path: the same
    // sampled primary, the same candidates and fallback chain (#3494). the
    // classic pool is walked by the loop the chat path uses, with the upload
    // forwarded as it came
    let fwd = Forward {
        state: &state,
        snap: &snap,
        entry,
        model,
        ctx: &ctx,
        payload: ForwardPayload::Multipart {
            body: &body,
            content_type: &content_type,
        },
        path,
        started,
        trace_headers: &trace_headers,
        inbound: (&headers, &forwarded_names),
        key_meta: vk.as_ref(),
    };
    let mut out = forward(&fwd, &mut cancel).await;
    // the slot rides on the guard, as on the chat path
    if let Some(guard) = out.inflight_guard.as_mut() {
        guard.hold_route_permit(route_permit.take());
    }

    let capture_payloads =
        payload_capture_enabled(&snap.logging.payload_capture, model, &ident.vk_id);
    // as on the chat path: from here every arm answers the caller and logs its
    // own row, so the guard must not add a phantom disconnect on top
    cancel.disarm();
    let Some((response, status, is_sse)) = out.outcome.take() else {
        return unanswered(&state, &snap, ident, out, stream);
    };
    // the stream the caller asked for came back as one: the legs that cannot
    // read it are those the operator let a stream pass (the rejecting
    // settings refused it before dialling), as on the chat path. an event
    // stream the request did not announce keeps them, and `stream_response`
    // withholds it (#3471)
    let (output_guard, post_response_plugins, pii_leg) = if is_sse && stream {
        (None, None, None)
    } else {
        (output_guard, post_response_plugins, pii_leg)
    };
    let ForwardOutcome {
        last_provider,
        last_target,
        variant,
        first_variant,
        sampled_variant,
        attempts,
        fallback_reason,
        inflight_guard,
        genai_span,
        ..
    } = out;
    let log = RequestLog {
        // the org that owns the provider in the snapshot that served the
        // request; the row keeps the runtime id until it is queued
        provider_org_id: snap.provider_label(&last_provider).1.to_string(),
        provider: last_provider,
        target: last_target,
        variant,
        first_variant,
        sampled_variant,
        status,
        upstream_status: status,
        attempts,
        fallback_reason,
        stream: u8::from(stream),
        unit_basis: crate::unit_billing::UnitBasis::for_multipart(path),
        capture_payloads,
        payload_max_bytes: snap.logging.payload_capture.max_bytes,
        payload_redactor: snap.payload_redactor.clone(),
        request_payload: if capture_payloads {
            crate::logging::capture_payload(
                &body,
                snap.logging.payload_capture.max_bytes,
                &snap.payload_redactor,
            )
        } else {
            String::new()
        },
        ..ident.row()
    };
    stream_response(
        response,
        is_sse,
        rolter_proxy::TranslationPlan::passthrough(),
        Accounting {
            started,
            sink: state.log.clone(),
            price,
            log,
            recorder,
            token_recorder,
            inflight_guard,
            genai_span,
        },
        None,
        DeliveryPolicy {
            output: output_guard.as_ref(),
            semantic: semantic_leg,
            plugins: post_response_plugins.as_ref(),
            pii: pii_leg.as_ref(),
            transcript: true,
        },
        None,
    )
    .await
}

/// The result of a forward loop, threaded back into the shared logging/response
/// tail so the classic and variant paths converge on one exit.
struct ForwardOutcome {
    outcome: Option<(reqwest::Response, u16, bool)>,
    last_provider: String,
    last_target: String,
    /// whether the last attempt was already recorded against its target
    last_attempt_recorded: bool,
    /// the last attempt's transport failure, when it never got a response
    last_error: Option<String>,
    /// the last retryable status an upstream answered with, when the loop ran
    /// out of targets while holding no response to hand back (#2807)
    last_status: Option<StatusFailure>,
    inflight_guard: Option<crate::load::LoadGuard>,
    /// chosen variant name for attribution
    variant: String,
    /// the variant the first attempt dialled; empty on the classic pool and
    /// when nothing was dialled (#3505)
    first_variant: String,
    /// the variant the weighted split sampled as primary, before cooldown,
    /// breaker and health skipping; empty on the classic pool (#3547)
    sampled_variant: String,
    provider_key_fingerprint: Option<String>,
    /// upstream attempts made, the one that answered included. Saturates, so
    /// a retry budget past 255 cannot wrap the count to zero
    attempts: u8,
    /// the failure that sent the request on to the last target it tried, as
    /// its class token; empty when the first dial answered (#1584)
    fallback_reason: String,
    /// the answering attempt's `upstream.request` span, which every dialled
    /// attempt opens (#808, #3535); `None` when no pipeline is installed, so
    /// the stream wrapper parses nothing to record onto a disabled span
    genai_span: Option<tracing::Span>,
}

/// The scope a route's cached answers, exact and semantic, are kept under: the
/// caller's key for a per-key route, otherwise the org that owns the route,
/// and nothing for a global route from the gateway's own file.
///
/// The org is mixed in whether or not another org shares the name (ADR-0040):
/// keying an unshared route by its bare name would let its entries outlive
/// the route and answer a later route of the same name in another org.
pub(crate) fn cache_scope<'a>(entry: &'a crate::state::RouteEntry, key_id: &'a str) -> &'a str {
    if entry.route.cache_per_key() {
        key_id
    } else {
        rolter_core::namespace::org_of(entry.route.tenancy.as_ref())
    }
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

/// Which of `provider`'s keys are parked on a cooldown under `key_ns`, read in
/// one lock acquisition rather than one per key (#3249). A single-key provider
/// has nothing to choose between, so it reads nothing.
pub(crate) fn parked_keys(
    state: &AppState,
    provider: &rolter_core::ProviderConfig,
    key_ns: &str,
) -> crate::target_mask::TargetMask {
    if provider.api_keys.len() > 1 {
        state.cooldowns.parked_mask(key_ns, provider.api_keys.len())
    } else {
        Default::default()
    }
}

/// The order a variant's targets are tried: the variant balancer's pick leads
/// (fed the same live in-flight + upstream queue-depth signal as the classic
/// pool, and told which targets are `eligible` as the classic pool's balancer
/// is), then the remaining targets follow in declared order so the fallback
/// tail stays deterministic. A route without variant balancers (or a pick out
/// of range) degrades to plain declared order.
fn variant_target_order(
    entry: &crate::state::RouteEntry,
    ctx: &RouteContext<'_>,
    vi: usize,
    n: usize,
    loads: &[u64],
    eligible: &dyn Fn(usize) -> bool,
) -> Vec<usize> {
    let lead = entry
        .variant_balancers
        .get(vi)
        .and_then(|b| b.pick_eligible(ctx, loads, eligible))
        .filter(|&i| i < n);
    let mut order = Vec::with_capacity(n);
    if let Some(i) = lead {
        order.push(i);
    }
    order.extend((0..n).filter(|&i| Some(i) != lead));
    order
}

/// Which targets of one pool cannot lead an attempt, read once per attempt:
/// parked on a cooldown, on an unhealthy provider, or behind an open breaker.
///
/// Each registry answers for the whole pool under one lock, so an attempt takes
/// three acquisitions however many targets the pool has. Asking the registries
/// per target, as the eligibility closure used to, took about three per target
/// and ran once per target the balancer weighed (#1942).
struct Blocked {
    parked: crate::target_mask::TargetMask,
    unhealthy: crate::target_mask::TargetMask,
    open: crate::target_mask::TargetMask,
}

impl Blocked {
    /// Read the registries for the first `n` targets of the pool `model`,
    /// whose providers are `providers` in target order. `cd_enabled` is the
    /// route's cooldown switch: with it off nothing counts as parked.
    fn read<'a>(
        cooldowns: &crate::cooldowns::Cooldowns,
        health: &crate::health::Health,
        breaker: &crate::breaker::Breaker,
        model: &str,
        cd_enabled: bool,
        n: usize,
        providers: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Self {
            parked: if cd_enabled {
                cooldowns.parked_mask(model, n)
            } else {
                Default::default()
            },
            unhealthy: health.unhealthy_mask(providers),
            open: breaker.open_mask(model, n),
        }
    }

    /// Whether target `idx` is parked, unhealthy or behind an open breaker.
    fn contains(&self, idx: usize) -> bool {
        self.parked.contains(idx) || self.unhealthy.contains(idx) || self.open.contains(idx)
    }
}

/// The `(variant, target)` pairs an attempt walks, in order: `primary` first,
/// then the rest in declared order, each variant's targets flattened with the
/// one its balancer picks from the eligible targets in the lead.
/// The variant keys are `variant_keys`, one per variant in declared order,
/// built once by the caller and reused by its forward loop.
fn variant_candidates(
    state: &AppState,
    entry: &crate::state::RouteEntry,
    variant_keys: &[String],
    ctx: &RouteContext<'_>,
    primary: usize,
    key_meta: Option<&KeyMeta>,
    cd_enabled: bool,
) -> Vec<(usize, usize)> {
    let route = &entry.route;
    let mut candidates: Vec<(usize, usize)> =
        Vec::with_capacity(route.variants.iter().map(|v| v.targets.len()).sum());
    for vi in route.fallback_order(primary) {
        if let (Some(v), Some(key)) = (route.variants.get(vi), variant_keys.get(vi)) {
            let mut loads = state.loads.snapshot(key, v.targets.len());
            for (i, target) in v.targets.iter().enumerate() {
                if let Some(l) = loads.get_mut(i) {
                    *l = l.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
                }
            }
            // the variant's balancer weighs only targets it can lead with
            // (#1851), asked of the registries once, not once per target
            let blocked = Blocked::read(
                &state.cooldowns,
                &state.health,
                &state.breaker,
                key,
                cd_enabled,
                v.targets.len(),
                v.targets.iter().map(|t| t.provider.as_str()),
            );
            let eligible = |ti: usize| {
                v.targets.get(ti).is_some_and(|target| {
                    key_meta.is_none_or(|meta| meta.provider_allowed(&target.provider))
                        && !blocked.contains(ti)
                        // a restricted fallback is never the balancer's lead
                        // (#1584)
                        && !target.is_restricted()
                })
            };
            for ti in variant_target_order(entry, ctx, vi, v.targets.len(), &loads, &eligible) {
                if key_meta.is_none_or(|key| key.provider_allowed(&v.targets[ti].provider)) {
                    candidates.push((vi, ti));
                }
            }
        }
    }
    candidates
}

/// What a forward loop forwards on each attempt.
#[derive(Clone, Copy)]
enum ForwardPayload<'a> {
    /// a JSON body: a variant merges its params into `parsed`, the classic
    /// pool sends `forward_body`, which already carries the route's
    Json {
        parsed: &'a Value,
        body: &'a Bytes,
        forward_body: &'a Bytes,
    },
    /// a multipart upload, forwarded verbatim with its content type, as
    /// [`proxy_multipart`] forwards one to the classic pool (#3494)
    Multipart {
        body: &'a Bytes,
        content_type: &'a str,
    },
}

/// What a forward loop reads from the request it forwards. Shared by the
/// classic pool and the variant chain, and by the JSON and multipart
/// pipelines; built once per request from borrows, so handing it to a loop
/// copies nothing.
#[derive(Clone, Copy)]
struct Forward<'a> {
    state: &'a AppState,
    snap: &'a Snapshot,
    entry: &'a crate::state::RouteEntry,
    /// the model the caller asked for, which a target without its own
    /// upstream model forwards as
    model: &'a str,
    ctx: &'a RouteContext<'a>,
    payload: ForwardPayload<'a>,
    path: &'a str,
    started: Instant,
    /// trace context plus the client headers the operator allowlisted (#564)
    trace_headers: &'a [(&'a str, &'a str)],
    /// the caller's headers and the allowlisted names, which every attempt
    /// re-injects its trace context from when a pipeline is installed (#805)
    inbound: (&'a HeaderMap, &'a [String]),
    key_meta: Option<&'a KeyMeta>,
}

/// Forward through the weighted-variant fallback chain. Samples a primary
/// variant by weight, then flattens the deterministic fallback order into an
/// ordered candidate list and drives it through the same retry/cooldown/breaker
/// machinery as the classic path. Within each variant the route's balancing
/// strategy leads: the variant balancer's pick goes first, the remaining
/// targets follow in declared order as the deterministic fallback tail.
/// Variant-level params are merged over route-level params per candidate before
/// the override policy is applied. A multipart upload takes the same variant
/// and the same candidates, and is forwarded as it came (#3494).
async fn forward_variants(
    fwd: &Forward<'_>,
    cancel: &mut crate::cancel::CancelGuard,
) -> ForwardOutcome {
    let Forward {
        state,
        snap,
        entry,
        model,
        ctx,
        payload,
        path,
        started,
        trace_headers,
        key_meta,
        ..
    } = *fwd;
    let route = &entry.route;
    let cd_enabled = snap.cooldown.enabled();

    let primary = route.sample_variant(jitter(started)).unwrap_or(0);
    // each variant's registry key, built once per request for the candidate
    // list and the loop below rather than once per candidate per attempt
    // (#3249)
    let variant_keys: Vec<String> = route
        .variants
        .iter()
        .map(|v| variant_key(&entry.namespace, &v.name))
        .collect();
    let candidates = variant_candidates(
        state,
        entry,
        &variant_keys,
        ctx,
        primary,
        key_meta,
        cd_enabled,
    );

    let mut run = ForwardLoop::new(fwd, ForwardPool::Variants(&candidates));
    // the split's own pick, which `next_candidate` may skip while its targets
    // are parked, unhealthy or behind an open breaker (#3547). recorded on the
    // variant path only, so a classic route allocates nothing for it
    if let Some(sampled) = route.variants.get(primary) {
        run.out.sampled_variant.clone_from(&sampled.name);
        cancel.sampled(&sampled.name);
    }
    for attempt in 0..=run.max_retries {
        let ci = match next_candidate(state, route, &candidates, &variant_keys, &run, cd_enabled) {
            Some(ci) => ci,
            None => break, // no untried candidate left to fail over to
        };
        run.tried.push(ci);
        run.out.fallback_reason = run
            .last_failure
            .as_ref()
            .map(Failure::token)
            .unwrap_or_default();
        let (vi, ti) = candidates[ci];
        let v = &route.variants[vi];
        let target = &v.targets[ti];
        let key = variant_keys[vi].as_str();
        // let learning strategies (cache-aware) see which target actually served
        if let Some(b) = entry.variant_balancers.get(vi) {
            b.observe(ti, ctx);
        }

        let provider = match snap.providers.get(&target.provider) {
            Some(provider) => provider,
            None => {
                // nothing was sent, so the probe slot this target took goes back
                state.breaker.release(key, ti);
                run.out.last_error = Some("configured target provider not found".to_string());
                break;
            }
        };
        run.out.variant = v.name.clone();
        run.out.last_provider = target.provider.clone();
        run.out.last_target = target.model.clone().unwrap_or_else(|| model.to_string());
        // see the classic path: an abandoned request names the target it left
        cancel.attribute(
            &run.out.last_provider,
            &run.out.last_target,
            &run.out.variant,
        );

        // merge variant params over route params for this candidate's body
        let forward_body = match payload {
            ForwardPayload::Json { parsed, body, .. } => {
                let mut injected = parsed.clone();
                route.apply_variant_params(v, &mut injected);
                serde_json::to_vec(&injected)
                    .map(Bytes::from)
                    .unwrap_or_else(|_| body.clone())
            }
            // the params are JSON-only and the upload names its own model, so
            // the form goes as it came, as on the classic pool
            ForwardPayload::Multipart { body, .. } => body.clone(),
        };

        // count this attempt as in-flight under the variant's key
        let mut guard = state.loads.begin(key, ti);
        // the latency model learns cost as a function of prompt size, so it
        // needs this request's size before the guard's drop folds in the
        // sample (#853). inert on every other strategy
        guard.set_prompt_tokens(rolter_balancer::predictor::prompt_tokens(ctx));
        // weighted pick across the provider's key pool, skipping keys parked
        // on a cooldown — same policy as the classic path
        let picked = PickedKey::pick(state, provider, &target.provider, started);
        let api_key = picked.api_key();
        run.out.provider_key_fingerprint = picked.fingerprint();
        let upstream_model = target.model.as_deref();

        // traced as a classic attempt is, under the target actually dialled
        // (#3535)
        let upstream_stage = genai_attempt_span(fwd, target, attempt);
        // handed on only when it records somewhere: the stream wrapper parses
        // the answer's body for every span it holds (#3535)
        run.out.genai_span = (!upstream_stage.is_disabled()).then(|| upstream_stage.clone());
        let injected_ctx = attempt_trace_context(fwd, &upstream_stage);
        let injected_refs;
        let attempt_headers: &[(&str, &str)] = match &injected_ctx {
            Some(injected) => {
                injected_refs = injected
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect::<Vec<_>>();
                &injected_refs
            }
            None => trace_headers,
        };

        let dial = run.begin_attempt(attempt, key, ti, &picked);
        let forwarded = match payload {
            ForwardPayload::Json { .. } => {
                state
                    .provider_queues
                    .forward_json(
                        &snap.queue,
                        snap.generation,
                        provider,
                        path,
                        forward_body,
                        api_key,
                        upstream_model,
                        attempt_headers,
                        &entry.overrides,
                    )
                    .instrument(upstream_stage.clone())
                    .await
            }
            ForwardPayload::Multipart { content_type, .. } => {
                state
                    .provider_queues
                    .forward_raw(
                        &snap.queue,
                        snap.generation,
                        provider,
                        path,
                        forward_body,
                        content_type,
                        api_key,
                        attempt_headers,
                        &entry.overrides,
                    )
                    .instrument(upstream_stage.clone())
                    .await
            }
        };
        let next = match forwarded {
            Ok(response) => {
                record_attempt_status(&upstream_stage, response.status().as_u16());
                run.settle_response(&dial, response, guard, cancel).await
            }
            Err(err) => {
                let message = err.to_string();
                record_attempt_error(&upstream_stage, &message);
                run.settle_error(&dial, message, cancel).await
            }
        };
        if next == Next::Stop {
            break;
        }
    }
    run.out
}

/// The candidate the variant loop dials next: an untried one that may follow
/// the last failure, preferring one that is not parked, unhealthy or behind an
/// open breaker.
fn next_candidate(
    state: &AppState,
    route: &rolter_core::ModelRoute,
    candidates: &[(usize, usize)],
    variant_keys: &[String],
    run: &ForwardLoop<'_>,
    cd_enabled: bool,
) -> Option<usize> {
    let open = |ci: &usize| {
        !run.tried.contains(ci)
            && candidate_admits(route, candidates[*ci], run.last_failure.as_ref())
    };
    // scoped so the borrow of the non-`Sync` cells ends before any await
    let fresh = {
        // a candidate is skippable when its target is parked, its provider is
        // unhealthy, or its breaker is open (or probing) — keyed per variant
        // the candidates come grouped by variant, so one slot holding the
        // variant last read means each variant's registries are read at
        // most once per attempt, with nothing allocated to hold them
        let read: std::cell::RefCell<Option<(usize, Blocked)>> = Default::default();
        let skip = |&(vi, ti): &(usize, usize)| {
            let mut read = read.borrow_mut();
            if read.as_ref().is_none_or(|(held, _)| *held != vi) {
                let v = &route.variants[vi];
                *read = Some((
                    vi,
                    Blocked::read(
                        &state.cooldowns,
                        &state.health,
                        &state.breaker,
                        &variant_keys[vi],
                        cd_enabled,
                        v.targets.len(),
                        v.targets.iter().map(|t| t.provider.as_str()),
                    ),
                ));
            }
            read.as_ref()
                .is_some_and(|(_, blocked)| blocked.contains(ti))
        };
        // prefer an untried, non-skipped candidate whose breaker admits it (the
        // read above leaves an elapsed open window to `allows`, which takes the
        // one half-open probe); fail open to any untried one when every
        // remaining candidate is parked/unhealthy/open
        (0..candidates.len()).find(|&ci| {
            let (vi, ti) = candidates[ci];
            open(&ci) && !skip(&candidates[ci]) && state.breaker.allows(&variant_keys[vi], ti)
        })
    };
    // and only a candidate that may follow the last failure: a restricted
    // fallback never leads and follows only a failure it takes over (#1584)
    fresh.or_else(|| (0..candidates.len()).find(|ci| open(ci)))
}

/// Forward through the route's classic target pool: pick a target, dial it,
/// and on a transient failure retry on a fresh one (exponential backoff +
/// jitter), parking failed keys and targets and feeding the circuit breaker.
/// Retries happen before any body bytes reach the client, so a partial
/// response is never duplicated. Each attempt opens one `upstream.request`
/// span and injects its trace context from inside it (#805); an upload is
/// forwarded as it came.
async fn forward_classic(
    fwd: &Forward<'_>,
    cancel: &mut crate::cancel::CancelGuard,
) -> ForwardOutcome {
    let Forward {
        state,
        snap,
        entry,
        model,
        ctx,
        payload,
        path,
        started,
        trace_headers,
        key_meta,
        ..
    } = *fwd;
    let cd_enabled = snap.cooldown.enabled();
    // live per-target in-flight counts steer load-aware strategies away from busy
    // targets; the count for the chosen target is held for the whole request.
    // scraped upstream queue depth (when enabled) is folded in so the balancer
    // also sees pressure that hasn't reached this gateway's own counters
    let mut loads = state
        .loads
        .snapshot(&entry.namespace, entry.route.targets.len());
    for (i, target) in entry.route.targets.iter().enumerate() {
        if let Some(l) = loads.get_mut(i) {
            *l = l.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
        }
    }
    let pool = TargetPool {
        entry,
        ctx,
        loads: &loads,
        cooldowns: &state.cooldowns,
        health: &state.health,
        breaker: &state.breaker,
        namespace: &entry.namespace,
        cd_enabled,
        key_meta,
    };
    let mut run = ForwardLoop::new(fwd, ForwardPool::Classic);
    for attempt in 0..=run.max_retries {
        let idx = match pool.pick(&run.tried, run.last_failure.as_ref()) {
            Some(i) => i,
            None => break, // no untried target left to fail over to
        };
        entry.balancer.observe(idx, ctx);
        run.tried.push(idx);
        run.out.fallback_reason = run
            .last_failure
            .as_ref()
            .map(Failure::token)
            .unwrap_or_default();
        // count this attempt as in-flight; the guard falls out of scope (and
        // decrements) on retry, or is moved into the stream wrapper on success
        let mut guard = state.loads.begin(&entry.namespace, idx);
        // the latency model learns cost as a function of prompt size, so it
        // needs this request's size before the guard's drop folds in the
        // sample (#853). inert on every other strategy
        guard.set_prompt_tokens(rolter_balancer::predictor::prompt_tokens(ctx));

        let target = &entry.route.targets[idx];
        let provider = match snap.providers.get(&target.provider) {
            Some(provider) => provider,
            None => {
                // nothing was sent, so the probe slot this target took goes back
                state.breaker.release(&entry.namespace, idx);
                run.out.last_error = Some("configured target provider not found".to_string());
                break;
            }
        };
        run.out.last_provider = target.provider.clone();
        run.out.last_target = target.model.clone().unwrap_or_else(|| model.to_string());
        // a caller who leaves from here on left this target, and the row
        // says so (#1816)
        cancel.attribute(&run.out.last_provider, &run.out.last_target, "");
        // weighted pick across the provider's key pool, skipping keys
        // parked on a cooldown (single-key providers yield their one key)
        let picked = PickedKey::pick(state, provider, &target.provider, started);
        let api_key = picked.api_key();
        // only a JSON request can be a `/v1/responses` call, the one reader
        if matches!(payload, ForwardPayload::Json { .. }) {
            run.out.provider_key_fingerprint = picked.fingerprint();
        }
        let upstream_model = target.model.as_deref();

        // every attempt is traced (see `genai_attempt_span`), an upload under
        // its audio operation (#3535)
        let upstream_stage = genai_attempt_span(fwd, target, attempt);
        // handed on only when it records somewhere: the stream wrapper parses
        // the answer's body for every span it holds (#3535)
        run.out.genai_span = (!upstream_stage.is_disabled()).then(|| upstream_stage.clone());
        let injected_ctx = attempt_trace_context(fwd, &upstream_stage);
        let injected_refs;
        let attempt_headers: &[(&str, &str)] = match &injected_ctx {
            Some(injected) => {
                injected_refs = injected
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect::<Vec<_>>();
                &injected_refs
            }
            None => trace_headers,
        };

        let dial = run.begin_attempt(attempt, &entry.namespace, idx, &picked);
        let forwarded = match payload {
            ForwardPayload::Json { forward_body, .. } => {
                state
                    .provider_queues
                    .forward_json(
                        &snap.queue,
                        snap.generation,
                        provider,
                        path,
                        forward_body.clone(),
                        api_key,
                        upstream_model,
                        attempt_headers,
                        &entry.overrides,
                    )
                    .instrument(upstream_stage.clone())
                    .await
            }
            ForwardPayload::Multipart { body, content_type } => {
                state
                    .provider_queues
                    .forward_raw(
                        &snap.queue,
                        snap.generation,
                        provider,
                        path,
                        body.clone(),
                        content_type,
                        api_key,
                        attempt_headers,
                        &entry.overrides,
                    )
                    .instrument(upstream_stage.clone())
                    .await
            }
        };
        let next = match forwarded {
            Ok(response) => {
                record_attempt_status(&upstream_stage, response.status().as_u16());
                run.settle_response(&dial, response, guard, cancel).await
            }
            Err(err) => {
                let message = err.to_string();
                record_attempt_error(&upstream_stage, &message);
                run.settle_error(&dial, message, cancel).await
            }
        };
        if next == Next::Stop {
            break;
        }
    }
    run.out
}

/// The trace context an attempt hands its provider, injected from inside the
/// attempt's span so the provider call parents to the gateway (#805), on the
/// classic pool and the variant chain, for a JSON body and an upload alike
/// (#3535). `None` when no OTLP pipeline is installed: the caller's own trace
/// headers are forwarded then, and nothing is built.
fn attempt_trace_context(
    fwd: &Forward<'_>,
    stage: &tracing::Span,
) -> Option<Vec<(String, String)>> {
    if !rolter_core::telemetry::is_active() {
        return None;
    }
    let (headers, forwarded_names) = fwd.inbound;
    Some(stage.in_scope(|| crate::trace::outbound_headers(headers, forwarded_names)))
}

/// Record the status an attempt was answered with on its span. `error.type`
/// is conditionally required on failure, and must stay low-cardinality — the
/// status class, never the provider's free-text message (#808).
fn record_attempt_status(stage: &tracing::Span, status: u16) {
    stage.record("http.response.status_code", status);
    if status >= 400 {
        stage.record(
            crate::genai::ERROR_TYPE,
            crate::genai::error_type_for_status(status),
        );
    }
}

/// Record on an attempt's span that it got no response at all: `error.type`
/// is the transport failure's kind, `timeout` or `transport`, never the
/// message, which quotes addresses (#808, #3535).
fn record_attempt_error(stage: &tracing::Span, message: &str) {
    stage.record(
        crate::genai::ERROR_TYPE,
        crate::genai::error_type_for_transport(message),
    );
}

/// The provider key an attempt sends with: a weighted pick across the
/// provider's key pool, skipping keys parked on a cooldown (a single-key
/// provider yields its one key).
struct PickedKey {
    /// the provider has more than one key, so a key-level failure parks the
    /// key rather than the target
    multi_key: bool,
    /// the key pool's cooldown namespace ([`key_pool_key`])
    pool: String,
    picked: Option<(usize, String)>,
}

impl PickedKey {
    fn pick(
        state: &AppState,
        provider: &rolter_core::ProviderConfig,
        provider_id: &str,
        started: Instant,
    ) -> Self {
        let multi_key = provider.api_keys.len() > 1;
        let pool = key_pool_key(provider_id);
        let parked_keys = parked_keys(state, provider, &pool);
        let picked = provider.pick_api_key_indexed(jitter(started), |i| parked_keys.contains(i));
        Self {
            multi_key,
            pool,
            picked,
        }
    }

    fn api_key(&self) -> Option<&str> {
        self.picked.as_ref().map(|(_, k)| k.as_str())
    }

    /// The picked key's [`provider_key_fingerprint`], which a
    /// `/v1/responses` follow-up dials the same key by.
    fn fingerprint(&self) -> Option<String> {
        self.picked
            .as_ref()
            .map(|(_, k)| provider_key_fingerprint(k))
    }
}

/// Which pool a [`ForwardLoop`] walks, which decides how it asks whether a
/// target is left to fail over to.
#[derive(Clone, Copy)]
enum ForwardPool<'a> {
    /// the route's classic targets, indexed by target
    Classic,
    /// the variant chain's flattened `(variant, target)` candidates, indexed
    /// by candidate
    Variants(&'a [(usize, usize)]),
}

/// Whether a forward loop goes on to another attempt.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    /// the attempt was superseded: try another
    Retry,
    /// the loop is done, answered or out of options
    Stop,
}

/// One attempt a [`ForwardLoop`] dialled: the registry slot it holds and the
/// key it sent with.
struct Dial<'a> {
    attempt: u32,
    /// the registry key of the pool the target belongs to: the route's
    /// namespace, or the variant's ([`variant_key`])
    namespace: &'a str,
    /// the target's index within that pool
    index: usize,
    key: &'a PickedKey,
    /// per-attempt clock: `started` is the whole request, and a superseded
    /// attempt's own duration is what the health funnel records (#1646)
    started: Instant,
}

/// What a forward loop carries from one attempt to the next, shared by the
/// classic pool and the variant chain so a fix to the retry, cooldown,
/// breaker or failover bookkeeping is made once (#1041).
struct ForwardLoop<'f> {
    fwd: &'f Forward<'f>,
    pool: ForwardPool<'f>,
    /// the route's own budget replaces the deployment's (#2924)
    max_retries: u32,
    cd_enabled: bool,
    out: ForwardOutcome,
    /// the targets (classic) or candidates (variants) tried so far
    tried: Vec<usize>,
    /// the failure the last attempt ended with, which decides which targets
    /// may follow it and is what the row records as the fallback reason (#1584)
    last_failure: Option<Failure>,
}

impl<'f> ForwardLoop<'f> {
    fn new(fwd: &'f Forward<'f>, pool: ForwardPool<'f>) -> Self {
        let entry = fwd.entry;
        let capacity = match pool {
            ForwardPool::Classic => entry.route.targets.len(),
            ForwardPool::Variants(candidates) => candidates.len(),
        };
        Self {
            fwd,
            pool,
            max_retries: entry
                .route
                .advanced
                .limits
                .max_retries(fwd.snap.retry.max_retries),
            cd_enabled: fwd.snap.cooldown.enabled(),
            out: ForwardOutcome {
                outcome: None,
                last_provider: String::new(),
                last_target: fwd.model.to_string(),
                last_attempt_recorded: false,
                last_error: None,
                last_status: None,
                inflight_guard: None,
                variant: String::new(),
                first_variant: String::new(),
                sampled_variant: String::new(),
                provider_key_fingerprint: None,
                attempts: 0,
                fallback_reason: String::new(),
                genai_span: None,
            },
            tried: Vec::with_capacity(capacity),
            last_failure: None,
        }
    }

    /// Whether the next attempt has a target to try after `failure`.
    fn has_untried(&self, failure: Option<&Failure>) -> bool {
        match self.pool {
            ForwardPool::Classic => {
                has_untried_target(self.fwd.entry, &self.tried, self.fwd.key_meta, failure)
            }
            ForwardPool::Variants(candidates) => {
                has_untried_candidate(&self.fwd.entry.route, candidates, &self.tried, failure)
            }
        }
    }

    /// Count the attempt about to be sent and start its clock.
    fn begin_attempt<'a>(
        &mut self,
        attempt: u32,
        namespace: &'a str,
        index: usize,
        key: &'a PickedKey,
    ) -> Dial<'a> {
        self.out.last_attempt_recorded = false;
        // the variant loop names the variant before it dials, so the first
        // attempt's is the one dialled first (#3505); the classic pool
        // has none and copies nothing
        if self.out.attempts == 0 {
            self.out.first_variant.clone_from(&self.out.variant);
        }
        self.out.attempts = self.out.attempts.saturating_add(1);
        Dial {
            attempt,
            namespace,
            index,
            key,
            started: Instant::now(),
        }
    }

    /// The caller will never see this attempt's failure, so record it against
    /// the target that produced it before superseding the attempt (#1646).
    /// Through the guard, so a caller leaving during a backoff is not charged
    /// to the target a second time.
    fn record_superseded(
        &mut self,
        cancel: &mut crate::cancel::CancelGuard,
        dial: &Dial<'_>,
        status: u16,
    ) {
        let error = if status == 0 {
            self.out.last_error.as_deref().unwrap_or_default()
        } else {
            ""
        };
        cancel.record_failed_attempt(&crate::logging::FailedAttempt {
            provider: &self.out.last_provider,
            target: &self.out.last_target,
            status,
            latency_ms: dial.started.elapsed().as_millis() as u32,
            error,
        });
        self.out.last_attempt_recorded = true;
    }

    /// Settle an attempt the upstream answered. A refusal another target
    /// takes over, a key-level `429`/`401` and a retryable status supersede
    /// the attempt while a retry is left; anything else is the answer, and
    /// `guard` goes with it.
    async fn settle_response(
        &mut self,
        dial: &Dial<'_>,
        response: reqwest::Response,
        mut guard: crate::load::LoadGuard,
        cancel: &mut crate::cancel::CancelGuard,
    ) -> Next {
        let Forward { state, snap, .. } = *self.fwd;
        let status = response.status().as_u16();
        let can_retry = dial.attempt < self.max_retries;
        // a content-policy refusal is final unless the route has a target
        // that takes refusals over and one is left to try: then the body is
        // read to classify it, and an answer that is not taken over goes on
        // to the caller whole (#1584)
        let response = if can_retry
            && crate::failover::may_be_refusal(status)
            && self.fwd.entry.route.takes_over_refusals()
        {
            let peeked = crate::failover::peek_refusal(response).await;
            match peeked.refusal {
                Some(failure) if self.has_untried(Some(&failure)) => {
                    // a refusal says nothing about the target's health, so it
                    // neither parks it nor feeds its breaker
                    self.record_superseded(cancel, dial, status);
                    count_retry(&state.metrics, true);
                    self.out.last_status = Some(StatusFailure {
                        status,
                        reason: peeked.reason,
                        retry_after: None,
                    });
                    self.out.last_error = None;
                    self.last_failure = Some(failure);
                    state.breaker.release(dial.namespace, dial.index);
                    return Next::Retry;
                }
                _ => peeked.response,
            }
        } else {
            response
        };
        // a 429/401 on a multi-key provider is a key-level failure: park the
        // key, keep the target in rotation and retry — a sibling key usually
        // succeeds
        let key_failure = dial.key.multi_key && (status == 429 || status == 401);
        if key_failure {
            if let Some((ki, _)) = &dial.key.picked {
                let secs = snap.cooldown.duration_secs(retry_after_secs(&response));
                state.cooldowns.park(&dial.key.pool, *ki, secs.max(1));
                state
                    .metrics
                    .key_cooldowns_tripped_total
                    .fetch_add(1, Relaxed);
            }
            // let the same target be re-picked with a fresh key
            self.tried.pop();
            // a key-level failure says nothing about the target, so its probe
            // slot is given back rather than held for the lease
            state.breaker.release(dial.namespace, dial.index);
            self.last_failure = Failure::from_status(status);
            if can_retry {
                return self.supersede_status(dial, response, cancel).await;
            }
        } else if is_retryable_status(status) {
            self.last_failure = Failure::from_status(status);
            // park the failing target so siblings absorb the load
            if self.cd_enabled {
                let secs = snap.cooldown.duration_secs(retry_after_secs(&response));
                state.cooldowns.park(dial.namespace, dial.index, secs);
                state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
            }
            // feed the circuit breaker; a sustained run trips the target open
            if state.breaker.on_failure(dial.namespace, dial.index) {
                state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
            }
            if can_retry {
                return self.supersede_status(dial, response, cancel).await;
            }
        } else if state.breaker.on_success(dial.namespace, dial.index) {
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
            // successful attempt: fold its duration into the target's latency
            // EWMA once streaming finishes
            guard.mark_ok();
        }
        self.out.inflight_guard = Some(guard);
        self.out.outcome = Some((response, status, is_sse));
        Next::Stop
    }

    /// Supersede an attempt answered with a failing status: record it, read
    /// the answer's reason, and back off before the next attempt if there is
    /// one. The response is read for its reason and dropped here, so the
    /// delay is taken from it first; the reason is what the request ends with
    /// if this was the last target (#2807).
    async fn supersede_status(
        &mut self,
        dial: &Dial<'_>,
        response: reqwest::Response,
        cancel: &mut crate::cancel::CancelGuard,
    ) -> Next {
        let status = response.status().as_u16();
        self.record_superseded(cancel, dial, status);
        let retrying = count_retry(
            &self.fwd.state.metrics,
            self.has_untried(self.last_failure.as_ref()),
        );
        let delay = retry_delay_ms(
            &self.fwd.snap.retry,
            &response,
            dial.attempt + 1,
            self.fwd.started,
        );
        let retry_after = retry_after_secs(&response);
        self.out.last_status = Some(StatusFailure::read(response, retry_after).await);
        self.out.last_error = None;
        back_off(delay, retrying).await;
        Next::Retry
    }

    /// Settle an attempt that never got a response: park the target and
    /// feed its breaker, and back off before the next attempt if a retry is
    /// left. A provider queue that turned the request away ends the loop.
    async fn settle_error(
        &mut self,
        dial: &Dial<'_>,
        message: String,
        cancel: &mut crate::cancel::CancelGuard,
    ) -> Next {
        let Forward {
            state,
            snap,
            started,
            ..
        } = *self.fwd;
        self.last_failure = Some(Failure::from_transport_error(&message));
        if is_queue_admission_error(&message) {
            // the queue turned the request away before the target saw it
            state.breaker.release(dial.namespace, dial.index);
            self.out.last_error = Some(message);
            return Next::Stop;
        }
        self.out.last_error = Some(message);
        // a connection-level failure parks the target too
        if self.cd_enabled {
            state.cooldowns.park(
                dial.namespace,
                dial.index,
                snap.cooldown.duration_secs(None),
            );
            state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
        }
        // and counts against the circuit breaker
        if state.breaker.on_failure(dial.namespace, dial.index) {
            state.metrics.breaker_opened_total.fetch_add(1, Relaxed);
        }
        if dial.attempt >= self.max_retries {
            return Next::Stop;
        }
        self.record_superseded(cancel, dial, 0);
        let retrying = count_retry(&state.metrics, self.has_untried(self.last_failure.as_ref()));
        back_off(
            snap.retry.backoff_ms(dial.attempt + 1, jitter(started)),
            retrying,
        )
        .await;
        Next::Retry
    }
}

/// The `upstream.request` span of one attempt, following the OTel GenAI
/// conventions so a conformant backend's built-in GenAI views work. The trace
/// context handed to the provider is injected from *inside* this span, which
/// is what makes the provider call a child of the gateway rather than a
/// sibling parented to the caller (#805).
///
/// `otel.name` carries the `{operation} {model}` name the spec asks for:
/// `tracing` span names are compile-time constants, so the exported name has
/// to be set as a field (#808). `otel.kind=client` marks this as the outbound
/// leg.
fn genai_attempt_span(
    fwd: &Forward<'_>,
    target: &rolter_core::Target,
    attempt: u32,
) -> tracing::Span {
    let snap = fwd.snap;
    let genai_model = target.model.as_deref().unwrap_or(fwd.model);
    let upstream_stage = crate::trace::stage_span!(
        "upstream.request",
        attempt = attempt,
        otel.name = tracing::field::Empty,
        otel.kind = "client",
        gen_ai.operation.name = tracing::field::Empty,
        gen_ai.provider.name = %snap.provider_name(&target.provider),
        gen_ai.system = %snap.provider_name(&target.provider),
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
    // with no pipeline installed there is nothing to record onto, and the
    // span name below is a `format!` the untraced path must not pay for
    if upstream_stage.is_disabled() {
        return upstream_stage;
    }
    if let Some(operation) = crate::genai::operation_for_path(fwd.path) {
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
        if let (crate::genai::OP_EMBEDDINGS, ForwardPayload::Json { parsed, .. }) =
            (operation.name, fwd.payload)
        {
            if let Some(formats) = crate::genai::encoding_formats(parsed) {
                upstream_stage.record(crate::genai::REQUEST_ENCODING_FORMATS, formats.as_str());
            }
        }
    }
    upstream_stage
}

/// One classic target pool as an attempt picks from it: the route, what the
/// balancer is told about the request, the live load per target, the
/// registries that can rule a target out and who is asking.
///
/// Built once per forward loop from borrows, so a pick costs nothing to set
/// up, and the loop asks [`TargetPool::pick`] once per attempt.
pub(crate) struct TargetPool<'a> {
    pub(crate) entry: &'a crate::state::RouteEntry,
    pub(crate) ctx: &'a RouteContext<'a>,
    pub(crate) loads: &'a [u64],
    pub(crate) cooldowns: &'a crate::cooldowns::Cooldowns,
    pub(crate) health: &'a crate::health::Health,
    pub(crate) breaker: &'a crate::breaker::Breaker,
    /// the registry key of the pool, the route's runtime namespace
    pub(crate) namespace: &'a str,
    pub(crate) cd_enabled: bool,
    pub(crate) key_meta: Option<&'a KeyMeta>,
}

/// [`TargetPool::pick`] with every input spelled out, as the unit tests call
/// it.
#[cfg(test)]
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
    failure: Option<&Failure>,
) -> Option<usize> {
    TargetPool {
        entry,
        ctx,
        loads,
        cooldowns,
        health,
        breaker,
        namespace: model,
        cd_enabled,
        key_meta,
    }
    .pick(tried, failure)
}

impl TargetPool<'_> {
    /// Pick a target the request has not tried yet, preferring one that is
    /// not on a cooldown. Honours the balancer's choice when it is fresh and
    /// healthy; else falls over to the first untried, un-parked target. When
    /// every remaining target is parked it fails open to the first untried one
    /// so requests still flow rather than 503-ing on a transient wobble.
    /// `failure` is how the previous attempt ended (`None` for the first).
    pub(crate) fn pick(&self, tried: &[usize], failure: Option<&Failure>) -> Option<usize> {
        self.pick_with(tried, failure, true)
    }

    /// [`TargetPool::pick`] for a caller that picks a candidate before
    /// deciding to send to it, such as a realtime session walking its dial
    /// list: the breaker is only read (`Breaker::refuses`), so no half-open
    /// probe slot is taken for a target the caller may never dial. The caller
    /// asks `Breaker::allows` of the one it dials (#3248). `failure` is how
    /// the previous dial failed, as for [`TargetPool::pick`] (#3322).
    pub(crate) fn pick_read_only(
        &self,
        tried: &[usize],
        failure: Option<&Failure>,
    ) -> Option<usize> {
        self.pick_with(tried, failure, false)
    }

    fn pick_with(
        &self,
        tried: &[usize],
        failure: Option<&Failure>,
        take_probe: bool,
    ) -> Option<usize> {
        let TargetPool {
            entry,
            ctx,
            loads,
            cooldowns,
            health,
            breaker,
            namespace: model,
            cd_enabled,
            key_meta,
        } = *self;
        let n = entry.route.targets.len();
        // the breaker's admission: `allows` takes the half-open probe slot, so
        // only a caller about to send asks it
        let breaker_admits = |i: usize| {
            if take_probe {
                breaker.allows(model, i)
            } else {
                !breaker.refuses(model, i)
            }
        };
        // a target the key may use and, if it is a restricted fallback, one the
        // failure being walked away from lets it take over (#1584)
        let key_allows = |i: usize| admits(entry, i, key_meta, failure);
        // a target is skippable when parked on a cooldown, when its provider is
        // currently marked unhealthy by the active prober, or when its circuit
        // breaker is open. this is the check for one target: the pick the balancer
        // returns, which is all a strategy that ignores eligibility ever asks about
        let skip = |i: usize| {
            !key_allows(i)
                || (cd_enabled && cooldowns.is_parked(model, i))
                || !health.is_healthy(&entry.route.targets[i].provider)
                || !breaker_admits(i)
        };
        // the same rules for every target at once, read from each registry in one
        // lock acquisition and only if something asks. a strategy that weighs
        // targets against each other asks about all of them, and per-target
        // answers would take about three locks each (#1942)
        let snapshot = std::cell::OnceCell::new();
        let blocked = |i: usize| {
            snapshot
                .get_or_init(|| {
                    Blocked::read(
                        cooldowns,
                        health,
                        breaker,
                        model,
                        cd_enabled,
                        n,
                        entry.route.targets.iter().map(|t| t.provider.as_str()),
                    )
                })
                .contains(i)
        };
        // the balancer weighs only the targets this attempt can use, so a dead
        // replica's empty queue never reads as the least-loaded target (#1851)
        let usable = |i: usize| i < n && !tried.contains(&i) && key_allows(i) && !blocked(i);
        // a pick past the target list is treated as no pick rather than indexed:
        // a balancer shared across per-request pools (#1655) can be sized from a
        // different list than the one it is picking over, and `skip` indexes
        // `targets` directly (#1714)
        if let Some(i) = entry
            .balancer
            .pick_eligible(ctx, loads, &usable)
            .filter(|&i| i < n)
        {
            if !tried.contains(&i) && !skip(i) {
                return Some(i);
            }
        }
        // prefer an untried, non-skipped target; fail open to any untried one when
        // every remaining sibling is parked or unhealthy. the snapshot above does
        // not move an open breaker whose window has elapsed to half-open, so the
        // target chosen here asks `allows`, which does
        (0..n)
            .find(|&i| usable(i) && breaker_admits(i))
            .or_else(|| first_untried_allowed(entry, tried, key_meta, failure))
    }
}

/// Whether target `i` of the classic pool may be dialled by a request with
/// `key_meta` after an attempt that ended with `failure` (`None` for the first
/// dial): the key may use its provider, and its `takes_over` set, if it has
/// one, covers the failure (see [`rolter_core::failover::may_dial`]).
fn admits(
    entry: &crate::state::RouteEntry,
    i: usize,
    key_meta: Option<&KeyMeta>,
    failure: Option<&Failure>,
) -> bool {
    let target = &entry.route.targets[i];
    key_meta.is_none_or(|key| key.provider_allowed(&target.provider))
        && rolter_core::failover::may_dial(&target.takes_over, failure)
}

/// The first target this request has not tried that its key may use,
/// whatever its cooldown, health or breaker says.
///
/// This is where [`TargetPool::pick`] ends up when every sibling is skippable, and
/// the only way it returns `None`, so it answers "is there anything left to
/// fail over to" without consulting the balancer. Asking the balancer instead
/// would advance a round-robin cursor for a pick nobody uses.
fn first_untried_allowed(
    entry: &crate::state::RouteEntry,
    tried: &[usize],
    key_meta: Option<&KeyMeta>,
    failure: Option<&Failure>,
) -> Option<usize> {
    (0..entry.route.targets.len())
        .find(|&i| !tried.contains(&i) && admits(entry, i, key_meta, failure))
}

/// Whether the next attempt of a forward loop has a target to try.
///
/// Read before the loop backs off, so a request is not made to wait for an
/// attempt that cannot happen (#2835). A multi-key provider that answered `429`
/// or `401` takes `tried.pop()` first, which makes its own target untried
/// again, so that path still waits for the key's cooldown.
pub(crate) fn has_untried_target(
    entry: &crate::state::RouteEntry,
    tried: &[usize],
    key_meta: Option<&KeyMeta>,
    failure: Option<&Failure>,
) -> bool {
    first_untried_allowed(entry, tried, key_meta, failure).is_some()
}

/// Whether the variant loop has a candidate it has not tried that may follow
/// `failure`, the variant counterpart of [`has_untried_target`].
fn has_untried_candidate(
    route: &rolter_core::ModelRoute,
    candidates: &[(usize, usize)],
    tried: &[usize],
    failure: Option<&Failure>,
) -> bool {
    (0..candidates.len())
        .any(|ci| !tried.contains(&ci) && candidate_admits(route, candidates[ci], failure))
}

/// Whether the `(variant, target)` candidate may be dialled after `failure`,
/// the variant counterpart of [`admits`] (the key was checked when the
/// candidate list was built).
fn candidate_admits(
    route: &rolter_core::ModelRoute,
    (vi, ti): (usize, usize),
    failure: Option<&Failure>,
) -> bool {
    rolter_core::failover::may_dial(&route.variants[vi].targets[ti].takes_over, failure)
}

/// Count a superseded attempt as a retry, if the loop goes on to make another
/// one (#2866).
///
/// `rolter_retries_total` counts attempts retried after a transient failure. The
/// block that supersedes an attempt runs before the loop knows whether a target
/// is left, and on a single-target route, or on the last target of several, it
/// is not: the next iteration finds nothing to try and the request ends with
/// the answer this attempt got. Nothing was retried then, so the counter stays.
/// Returns `can_retry` so the caller hands the same answer to [`back_off`].
fn count_retry(metrics: &crate::metrics::Metrics, can_retry: bool) -> bool {
    if can_retry {
        metrics.retries_total.fetch_add(1, Relaxed);
    }
    can_retry
}

/// Wait out the backoff before the next attempt, unless there is no next
/// attempt to wait for.
///
/// A single-target route whose upstream answers `429` with `Retry-After: 30`
/// used to make the caller wait the full 30 s only to be told the same thing:
/// the loop woke, found nothing left to try and ended the request with that
/// answer. `can_retry` is whether the loop will find a target when it wakes.
async fn back_off(delay_ms: u64, can_retry: bool) {
    if can_retry {
        sleep(Duration::from_millis(delay_ms)).await;
    }
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

/// The upstream sent its status line and then failed to deliver the body this
/// gateway was buffering: a reset, a truncated body, a read timeout (#1775).
///
/// The request still happened, and the provider may have generated, and
/// billed, some or all of an answer nobody received. So it is logged like any
/// other upstream failure — one row, counted against its target by the passive
/// health funnel — with its usage marked unknown rather than zero, because
/// what the provider charged is what the lost body would have said.
fn body_read_failed(
    sink: &crate::logging::LogSink,
    mut log: RequestLog,
    started: Instant,
    err: &reqwest::Error,
) -> Response {
    // `reqwest` quotes the url it was reading from. the one masked text goes to
    // the caller and to the request-log row, which the dashboard's Logs screen
    // shows to people who never saw the provider's address (#2919)
    let message = rolter_core::redact::redact_urls_in_text(&format!(
        "upstream response body could not be read: {err}"
    ));
    log.status = StatusCode::BAD_GATEWAY.as_u16();
    log.latency_ms = started.elapsed().as_millis() as u32;
    log.error = message.clone();
    log.usage_unknown = 1;
    sink.metrics().upstream_errors_total.fetch_add(1, Relaxed);
    sink.log(log);
    error_json(StatusCode::BAD_GATEWAY, &message)
}

/// Convert an upstream response into a streaming axum response, teeing the body
/// through [`UsageLoggingStream`] so token usage and latency are logged once the
/// response has been fully forwarded.
///
/// `policy` is the post-response policy the request was admitted under; its
/// whole-body legs are set only for a request that did not ask for a stream.
async fn stream_response(
    response: reqwest::Response,
    is_sse: bool,
    translation: rolter_proxy::TranslationPlan,
    mut acct: Accounting,
    completion_observer: Option<crate::logging::CompletionObserver>,
    policy: DeliveryPolicy<'_, '_>,
    pii_stream: Option<crate::pii_stream::StreamRestore>,
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
    let decision = DecisionHeaders::from_log(&acct.log);
    // the prompt-cache hit a provider states in a response header (Fireworks,
    // #2882), which the body parser cannot see
    let header_cache_read = response
        .headers()
        .get(rolter_proxy::CACHED_PROMPT_TOKENS_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(rolter_proxy::cached_prompt_tokens_header);
    // a non-streamed body is buffered when something has to see all of it: a
    // response translation, or output guardrails (#591). the guard only exists
    // when a post-call rule applies to this route, so an unguarded route keeps
    // streaming straight through
    //
    // a stream is buffered only for a semantic guardrail policy that chose
    // `streaming = "buffer"` (ADR-0037): held in full, judged once, then
    // replayed unchanged or withheld. a stream that outgrows the cap is
    // withheld, never delivered unchecked
    // the leg judges what actually came back: a request that asked for a
    // stream and got a JSON body is judged as one
    let semantic = policy.semantic.map(|leg| SemanticLeg {
        stream: is_sse,
        ..leg
    });
    // a caller of this function hands over a whole-body leg only for a
    // request that did not ask for a stream, so an event stream here is one
    // the upstream sent unasked, which no such leg can read (#3471)
    if is_sse && status.is_success() && policy.has_whole_body_leg() {
        return withhold_unrequested_stream(response, &translation, acct).await;
    }
    let buffer_stream = is_sse && semantic.is_some();
    if buffer_stream
        || (!is_sse
            && (translation.rewrites_response()
                || policy.output.is_some()
                || semantic.is_some()
                || policy.plugins.is_some()
                || policy.pii.is_some()))
    {
        if buffer_stream {
            acct.sink
                .metrics()
                .guardrail_streams_buffered_total
                .fetch_add(1, Relaxed);
        }
        let cap = if buffer_stream {
            crate::semantic_guards::MAX_BUFFERED_STREAM_BYTES
        } else {
            usize::MAX
        };
        return match read_bounded(response, cap).await {
            Ok(None) => withheld_response(
                guardrail_output_blocked("stream_too_large"),
                guardrail_withheld_reason("stream_too_large"),
                crate::logging::Usage::default(),
                is_sse,
                acct,
            ),
            Ok(Some(bytes)) => {
                let bytes = translation.translate_response(bytes, is_sse);
                // what the provider charged, read before the guardrails,
                // plugins and sanitizer below can rewrite or withhold the body
                // (#1478)
                let mut billed =
                    crate::logging::parse_usage_with(is_sse, &bytes, translation.thinking_count());
                billed.fill_cache_read(
                    header_cache_read.map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
                );
                let policy = DeliveryPolicy {
                    semantic,
                    // a streamed transcript is judged from its events, not
                    // wrapped whole as plain text (#3432)
                    transcript: policy.transcript && !is_sse,
                    ..policy
                };
                // read before the policy too: a transcript's audio duration is
                // billed whether it is rewritten, withheld or delivered (#3331)
                acct.log.billed_units = acct.log.unit_basis.units(&bytes);
                let mut post_events = PolicyEvents::default();
                let applied = policy.apply(status, &mut post_events, bytes).await;
                post_events.record(&mut acct.log);
                let (bytes, pre_restore) = match applied {
                    Delivery::Send { body, pre_restore } => (body, pre_restore),
                    Delivery::Refuse { response, reason } => {
                        return withheld_response(response, reason, billed, is_sse, acct)
                    }
                };
                buffered_response(
                    Buffered {
                        bytes,
                        pre_restore,
                        billed,
                        status: status.as_u16(),
                        content_type,
                        is_sse,
                    },
                    acct,
                    completion_observer,
                    // a buffered stream is replayed through the restore like a
                    // live one, so the guard judged the placeholders and only
                    // the caller sees plaintext (ADR-0036, ADR-0037)
                    pii_stream,
                )
            }
            Err(err) => body_read_failed(&acct.sink, acct.log, acct.started, &err),
        };
    }
    let upstream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>,
    > = Box::pin(response.bytes_stream());
    let translated: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>,
    > = if translation.rewrites_response() && is_sse {
        Box::pin(rolter_proxy::TranslatedStream::new(upstream, translation))
    } else {
        upstream
    };
    let body = acct
        .into_stream(translated, is_sse)
        .with_completion_observer(completion_observer)
        .with_thinking_count(translation.thinking_count())
        .with_header_cache_read(header_cache_read);
    let body = restore_stream_body(body, is_sse, status, pii_stream);
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type);
    decision.apply(builder.headers_mut());
    builder.body(body).unwrap_or_else(|_| {
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
    /// payload capture is on for this request, so the pre-restore body is
    /// worth keeping (#3240)
    capture: bool,
}

/// Run the PII sanitizer's response leg over a buffered response body (#848).
///
/// Two independent steps, in this order: sanitize provider-generated PII, then
/// restore the placeholders this request's own sanitization introduced. Both
/// are best-effort — a failure returns the body unchanged, which is safe: the
/// worst case is a caller receiving placeholders instead of plaintext, never
/// the reverse.
///
/// Returns the body to deliver and, when a restore replaced placeholders and
/// payload capture is on, the placeholder-bearing body it replaced. That one is
/// what payload capture stores, so a captured response holds what the request
/// side holds — the sanitized text — rather than the plaintext the sanitizer
/// exists to keep out of rolter's stores (#3240).
async fn pii_response_leg_apply(leg: &PiiResponseLeg<'_>, bytes: Bytes) -> (Bytes, Option<Bytes>) {
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
        capture,
    } = leg;
    let (will_restore, ticket, capture) = (*will_restore, *ticket, *capture);
    let (state, config, scope) = (*state, *config, *scope);
    // a body that does not parse is not JSON we can walk; forwarding it
    // untouched is strictly better than failing a successful response
    let Ok(mut parsed) = serde_json::from_slice::<Value>(&bytes) else {
        return (bytes, None);
    };

    if config.sanitizes_response() {
        match crate::pii_sanitizer::sanitize(
            config,
            &state.metrics,
            &state.side_client,
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

    let mut pre_restore = None;
    if will_restore {
        if let Some(ticket) = ticket {
            if let Some(restored) = crate::pii_sanitizer::restore(
                config,
                &state.metrics,
                &state.side_client,
                ticket,
                scope,
                trace_id,
                tenant.clone(),
                &parsed,
            )
            .await
            {
                let before = std::mem::replace(&mut parsed, restored);
                pre_restore = capture.then_some(before);
            }
        }
    }

    let encode = |value: &Value| serde_json::to_vec(value).map(Bytes::from).ok();
    // the capture copy is only worth keeping when it differs from what is
    // delivered; an unchanged restore leaves nothing to tell apart
    let pre_restore = pre_restore
        .filter(|before| before != &parsed)
        .and_then(|before| encode(&before));
    (encode(&parsed).unwrap_or(bytes), pre_restore)
}

/// Replay a buffered (already fully received) response body through the same
/// [`UsageLoggingStream`] accounting the streamed path uses. Used by the
/// response cache's store path: the body was buffered to write it to Redis, so
/// it's handed on as a single-chunk stream rather than re-fetched. `is_sse`
/// carries the upstream content type through so a buffered SSE completion is
/// still parsed frame-by-frame for its final usage chunk.
fn buffered_response(
    buffered: Buffered,
    acct: Accounting,
    completion_observer: Option<crate::logging::CompletionObserver>,
    pii_stream: Option<crate::pii_stream::StreamRestore>,
) -> Response {
    let Buffered {
        bytes,
        pre_restore,
        billed,
        status,
        content_type,
        is_sse,
    } = buffered;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let log = &acct.log;
    let decision = DecisionHeaders::from_log(log);
    // redacted and cut to `payload_max_bytes` now, so the stream holds no more
    // than the row will store (#3240)
    let captured = pre_restore.filter(|_| log.capture_payloads).map(|body| {
        crate::logging::capture_payload(&body, log.payload_max_bytes, &log.payload_redactor)
    });
    let stream = futures_util::stream::iter(vec![Ok::<Bytes, reqwest::Error>(bytes)]);
    let body = acct
        .into_stream(Box::pin(stream), is_sse)
        .with_completion_observer(completion_observer)
        .with_billed_usage(billed)
        .with_captured_payload(captured);
    let body = restore_stream_body(body, is_sse, status, pii_stream);
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type);
    decision.apply(builder.headers_mut());
    builder.body(body).unwrap_or_else(|_| {
        error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build response",
        )
    })
}

/// The response body for a stream, restored through the sanitizer when
/// `pii_stream` is set (#911).
///
/// The restore wraps the accounting stream rather than sitting inside it, so
/// usage, billing, the disconnect row and payload capture all read the
/// upstream's own (placeholder-bearing) bytes, and only the caller sees the
/// restored ones. Only a successful SSE body is restored; an error body is
/// passed on as the upstream sent it, and a successful body that is not a
/// stream is counted as a restore that could not happen.
fn restore_stream_body<S>(
    body: S,
    is_sse: bool,
    status: StatusCode,
    pii_stream: Option<crate::pii_stream::StreamRestore>,
) -> Body
where
    S: futures_util::Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
{
    match pii_stream {
        // boxed only here, so a stream nothing restores costs no allocation
        Some(restore) if status.is_success() => {
            if is_sse {
                Body::from_stream(restore.wrap(Box::pin(body)))
            } else {
                restore.not_a_stream();
                Body::from_stream(body)
            }
        }
        _ => Body::from_stream(body),
    }
}

/// Refuse delivery of a response the upstream already produced — and billed —
/// while still accounting for it (#1478).
///
/// A post-call policy decides what the caller receives, not what the provider
/// charged. `billed` is the usage read from the upstream body before the policy
/// ran, so the budget and rate-limit counters advance and the log row carries
/// the real tokens and cost, marked `withheld` with `reason` as its error. The
/// rejected body is never handed to the accounting stream, so it cannot reach
/// payload capture. `refusal` is returned to the caller unchanged.
fn withheld_response(
    refusal: Response,
    reason: String,
    billed: crate::logging::Usage,
    is_sse: bool,
    acct: Accounting,
) -> Response {
    acct.into_stream(Box::pin(futures_util::stream::empty()), is_sse)
        .with_billed_usage(billed)
        .withhold(refusal.status().as_u16(), reason);
    refusal
}

/// What bills, logs and holds the in-flight slot of one forwarded response,
/// handed whole to the [`crate::logging::UsageLoggingStream`] that delivers
/// it. Every response tail ([`stream_response`], [`buffered_response`],
/// [`withheld_response`], [`withhold_unrequested_stream`]) takes it by value,
/// so it is moved down the tail and never cloned.
struct Accounting {
    started: Instant,
    sink: crate::logging::LogSink,
    price: Option<rolter_core::ModelPriceConfig>,
    log: RequestLog,
    recorder: SpendRecorder,
    token_recorder: TokenRecorder,
    inflight_guard: Option<crate::load::LoadGuard>,
    /// the successful attempt's span, which token usage is recorded on once
    /// the body has been consumed (#808)
    genai_span: Option<tracing::Span>,
}

impl Accounting {
    /// The accounting stream over `inner`, carrying the upstream span.
    fn into_stream(
        self,
        inner: std::pin::Pin<Box<dyn futures_util::Stream<Item = reqwest::Result<Bytes>> + Send>>,
        is_sse: bool,
    ) -> crate::logging::UsageLoggingStream {
        crate::logging::UsageLoggingStream::new(
            inner,
            is_sse,
            self.started,
            self.sink,
            self.price,
            self.log,
            Some(self.recorder),
            Some(self.token_recorder),
            self.inflight_guard,
        )
        .with_genai_span(self.genai_span)
    }
}

/// A fully received body on its way to [`buffered_response`], with what the
/// provider billed for it read before any policy rewrote it (#1478).
struct Buffered {
    bytes: Bytes,
    /// the placeholder-bearing body a restore replaced, kept for payload
    /// capture (#3240)
    pre_restore: Option<Bytes>,
    billed: crate::logging::Usage,
    status: u16,
    content_type: String,
    is_sse: bool,
}

/// Who asked for what, captured once a request is authenticated: enough to
/// write its request-log row on any path that answers without running the
/// forwarding pipeline that builds one for a provider request.
///
/// A response served from cache and the built-in `fake-llm` model both answer
/// that way, and start from the same row ([`RequestLogCtx::into_log`]) so the
/// attribution they stamp cannot drift apart. A path that refuses a request
/// before it reaches a provider can start from it too, and so does every row
/// of a forwarded request ([`RequestLogCtx::row`]).
#[derive(Clone)]
struct RequestLogCtx {
    request_id: String,
    trace_id: String,
    vk_id: String,
    org_id: String,
    team_id: String,
    project_id: String,
    business_unit_id: String,
    customer_id: String,
    model: String,
    /// the resolved route's runtime namespace, which labels the request's
    /// metric series (#3462); `None` before a route is resolved
    route_namespace: Option<std::sync::Arc<str>>,
    sample_rate: f64,
    started: Instant,
    /// what each guardrail did to the request (#2158)
    guardrail_events: Vec<String>,
    /// what each plugin did to the request (#2243)
    plugin_events: Vec<String>,
}

impl RequestLogCtx {
    /// Capture the identity of an authenticated request. `scope` is the key's
    /// attribution (empty for an anonymous request), `model` the name the
    /// caller asked for.
    fn new(
        headers: &HeaderMap,
        request_id: &str,
        scope: &ScopeIds,
        model: &str,
        sample_rate: f64,
        started: Instant,
    ) -> Self {
        Self {
            // the id the ensure_request_id middleware settled on, which is the
            // one the response echoes
            request_id: request_id.to_string(),
            // adopts the caller's distributed trace when one was propagated inbound
            trace_id: crate::trace::request_trace_id(headers),
            vk_id: scope.key.clone(),
            org_id: scope.org.clone(),
            team_id: scope.team.clone(),
            project_id: scope.project.clone(),
            business_unit_id: scope.business_unit.clone(),
            customer_id: scope.customer.clone(),
            model: model.to_string(),
            route_namespace: None,
            sample_rate,
            started,
            guardrail_events: Vec::new(),
            plugin_events: Vec::new(),
        }
    }

    /// The row this request starts from: its own start time and attribution,
    /// with the latency measured now and time to first token equal to it. The
    /// caller sets what only its path knows (status, provider, tokens).
    fn into_log(self) -> RequestLog {
        let latency_ms = self.started.elapsed().as_millis() as u32;
        RequestLog {
            latency_ms,
            ttft_ms: latency_ms,
            ..self.row()
        }
    }

    /// The row a forwarded request starts from: its start time and
    /// attribution, with the latency left to whoever finishes it.
    fn row(self) -> RequestLog {
        RequestLog {
            ts: crate::logging::started_at(self.started),
            seq: crate::logging::start_seq(self.started),
            request_id: self.request_id,
            trace_id: self.trace_id,
            virtual_key_id: self.vk_id,
            org_id: self.org_id,
            team_id: self.team_id,
            project_id: self.project_id,
            business_unit_id: self.business_unit_id,
            customer_id: self.customer_id,
            model: self.model,
            route_namespace: self.route_namespace,
            sample_rate: self.sample_rate,
            guardrail_events: self.guardrail_events,
            plugin_events: self.plugin_events,
            ..Default::default()
        }
    }
}

/// The longest model name a refusal row keeps. A refusal is written before any
/// route vouches for the name, so it is whatever string the caller sent.
const REFUSAL_MODEL_MAX_BYTES: usize = 256;

/// The longest `error` a refusal row keeps. Some refusals quote the caller's
/// own words back, such as `no route for model '…'`, and a body can be large.
const REFUSAL_ERROR_MAX_BYTES: usize = 512;

/// The message a refusal carries, bounded for a log row: the `error` the row
/// holds is what the caller was told.
fn refusal_error(response: &Response) -> String {
    let mut error = response
        .extensions()
        .get::<crate::error::ErrorMessage>()
        .map(|message| message.0.clone())
        .unwrap_or_default();
    error.truncate(error.floor_char_boundary(REFUSAL_ERROR_MAX_BYTES));
    error
}

/// Why the gateway refused with `response`, for the row's `outcome` (#3307):
/// from the error code it answered with, or its status when it has none.
fn refusal_outcome(response: &Response) -> rolter_core::request_outcome::RequestOutcome {
    let code = response
        .extensions()
        .get::<crate::error::ErrorCode>()
        .map(|code| code.0);
    rolter_core::request_outcome::RequestOutcome::for_refusal(response.status().as_u16(), code)
}

/// Writes the request-log row for a request the gateway refused before it
/// reached an upstream (#2807).
///
/// A spent budget, a rate limit, a guardrail, an unknown model and the rest
/// used to answer the caller and leave nothing behind, so an operator asked
/// "why did this call fail" had a `4xx` on the client side and an empty LLM
/// Logs on theirs. The rule now is that **every request from a caller the
/// gateway has identified leaves one row**, whatever happened to it.
///
/// What stays unlogged is what comes before identification: a body that is not
/// JSON, a missing `model`, a missing or unknown key, and the middleware
/// refusals ahead of the handler (required headers, body size). No tenant owns
/// those, and an unauthenticated caller could write rows without limit. They
/// are counted instead (`rolter_auth_failures_total`).
///
/// The row has no provider or target, which is how it reads as "never reached
/// an upstream", and is written without touching the per-model latency
/// histograms (see [`crate::logging::LogSink::log_refusal`]).
struct Refusals<'a> {
    state: &'a AppState,
    headers: &'a HeaderMap,
    request_id: &'a str,
    scope: &'a ScopeIds,
    model: &'a str,
    sample_rate: f64,
    started: Instant,
    /// what each plugin did before the refusal, so a request a later check
    /// turns away still shows the calls already made (#2243). A mutex, not a
    /// `Cell`: the handler future must stay `Sync` across awaits
    plugin_events: parking_lot::Mutex<Vec<String>>,
}

impl Refusals<'_> {
    /// Remember the calls a plugin stage made, for every row this request
    /// leaves from here on.
    fn note_plugin_events(&self, events: Vec<String>) {
        self.plugin_events.lock().extend(events);
    }

    /// The plugin calls noted so far.
    fn plugin_events(&self) -> Vec<String> {
        self.plugin_events.lock().clone()
    }

    /// Log `response` as this request's outcome and hand it back. The row's
    /// `error` is the message the response carries, so the log says what the
    /// caller was told.
    fn log(&self, response: Response) -> Response {
        self.log_with_events(response, Vec::new())
    }

    /// [`Refusals::log`] for a refusal a guardrail made, so the row names the
    /// rule that acted (#2158).
    fn log_with_events(&self, response: Response, guardrail_events: Vec<String>) -> Response {
        let error = refusal_error(&response);
        let model = &self.model[..self.model.floor_char_boundary(REFUSAL_MODEL_MAX_BYTES)];
        let ctx = RequestLogCtx::new(
            self.headers,
            self.request_id,
            self.scope,
            model,
            self.sample_rate,
            self.started,
        );
        self.state.log.log_refusal(RequestLog {
            status: response.status().as_u16(),
            outcome: Some(refusal_outcome(&response)),
            error,
            guardrail_events,
            plugin_events: self.plugin_events(),
            ..ctx.into_log()
        });
        response
    }
}

/// Write the request-log row for an answer the built-in `fake-llm` model gave
/// and hand the answer back.
///
/// The built-in answers before the pipeline that builds a row for every
/// provider request, so without this its traffic is missing from LLM Logs and
/// from every usage roll-up, and the first smoke test an operator runs looks
/// like logging is broken (#2802).
///
/// The row is the one a provider request would leave, labelled
/// [`fake_llm::PROVIDER_LABEL`] and carrying the tokens the fake body reports.
/// Cost stays zero and the row is not flagged `unpriced` or `usage_unknown`:
/// the model costs nothing by definition, so the zero is exact rather than the
/// absence of a price that those flags exist to expose. The row is written when
/// the answer is built, not when a stream is drained, which for a body that is
/// generated in memory and sent whole is the same moment to within a
/// millisecond.
fn log_builtin(
    sink: &crate::logging::LogSink,
    ctx: RequestLogCtx,
    reply: fake_llm::Reply,
) -> Response {
    let fake_llm::Reply {
        response,
        usage,
        stream,
    } = reply;
    sink.log_builtin(RequestLog {
        provider: fake_llm::PROVIDER_LABEL.to_string(),
        target: fake_llm::MODEL_NAME.to_string(),
        status: response.status().as_u16(),
        // the built-in has no upstream to fail: an error is the request's own
        outcome: (!response.status().is_success()).then(|| refusal_outcome(&response)),
        stream: u8::from(stream),
        prompt_tokens: usage.prompt,
        completion_tokens: usage.completion,
        total_tokens: usage.total,
        ..ctx.into_log()
    });
    response
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
pub(crate) fn unpriced_admission(
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
///
/// The pipelines read the same pair off the request's [`ScopeIds`], which
/// [`request_scope`] fills from the key. This reads it through
/// [`request_scope`] too, so the unit tests that pin what a key reports pin
/// the mapping the pipelines use.
#[cfg(test)]
fn key_attribution(vk: Option<&crate::state::KeyMeta>) -> (String, String) {
    let scope = request_scope(vk);
    (scope.business_unit, scope.customer)
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
/// The full post-response policy runs here too, exactly as on a live
/// response: output guardrails, `post_response` plugins, then the PII
/// sanitizer's response leg (#1477). The cache stores what the upstream
/// returned and the policy runs on every delivery instead of once at store
/// time, so a rule or plugin added after an entry was cached still applies to
/// it, an entry shared by two routes is judged by the route serving it, and a
/// PII restore uses the ticket of the request being answered — never one
/// persisted from the request that filled the entry.
async fn cached_response(
    hit: CachedResponse,
    sink: &crate::logging::LogSink,
    ctx: RequestLogCtx,
    policy: DeliveryPolicy<'_, '_>,
    pii_stream: Option<crate::pii_stream::StreamRestore>,
) -> Response {
    let status = StatusCode::from_u16(hit.status).unwrap_or(StatusCode::OK);
    let mut post_events = PolicyEvents::default();
    let applied = policy
        .apply(status, &mut post_events, Bytes::from(hit.body))
        .await;
    let (body, withheld) = match applied {
        // a hit captures no payload, so the pre-restore copy has no reader
        Delivery::Send { body, .. } => (body, None),
        Delivery::Refuse { response, reason } => (Bytes::new(), Some((response, reason))),
    };
    let mut log = RequestLog {
        status: hit.status,
        cache_hit: 1,
        // the answer was produced by a request translated this way, so a hit
        // says so as the miss did (#3326)
        param_adjustments: rolter_proxy::ParamAdjustments::from_log_entries(
            hit.param_adjustments.iter().map(String::as_str),
        ),
        ..ctx.into_log()
    };
    post_events.record(&mut log);
    // a withheld hit still gets its row, so a refusal is visible, but with zero
    // tokens and cost: nothing was generated upstream for this request (#1478)
    if let Some((refusal, reason)) = withheld {
        sink.metrics()
            .withheld_responses_total
            .fetch_add(1, Relaxed);
        // as `UsageLoggingStream::withhold` records a live refusal
        if let Some(rule) = reason.strip_prefix(crate::logging::GUARDRAIL_WITHHELD_PREFIX) {
            log.guardrail_events.push(format!("blocked:{rule}"));
        }
        sink.log(RequestLog {
            status: refusal.status().as_u16(),
            withheld: 1,
            error: reason,
            ..log
        });
        return refusal;
    }
    let decision = DecisionHeaders::from_log(&log);
    sink.log(log);
    // a replayed stream is restored with the token of the request being
    // answered, exactly like a live one (#911)
    let body = match pii_stream {
        Some(restore) if status.is_success() => {
            if hit.content_type.contains("text/event-stream") {
                let replay = futures_util::stream::iter([Ok::<_, reqwest::Error>(body)]);
                Body::from_stream(restore.wrap(Box::pin(replay)))
            } else {
                restore.not_a_stream();
                Body::from(body)
            }
        }
        _ => Body::from(body),
    };
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, hit.content_type);
    decision.apply(builder.headers_mut());
    builder.body(body).unwrap_or_else(|_| {
        error_json(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build response",
        )
    })
}

/// The post-response policy one request is delivered under: built-in output
/// guardrails, the `post_response` plugin chain, and the PII sanitizer's
/// response leg. Each is `None` when it does not apply to this request, which
/// includes every streamed request (they cannot be applied to a stream, and
/// the request path has already refused or opted the stream out).
///
/// It is the single place a fully buffered body is finalised, shared by the
/// live non-cached path, the cacheable miss path and both kinds of cache hit,
/// so enabling the response cache can never change which policy a caller's
/// completion passes through (#1477).
#[derive(Clone, Copy)]
struct DeliveryPolicy<'p, 'a> {
    output: Option<&'p crate::guardrails::OutputGuard<'a>>,
    semantic: Option<SemanticLeg<'p, 'a>>,
    plugins: Option<&'p crate::plugin_dispatch::PostResponsePlugins<'a>>,
    pii: Option<&'p PiiResponseLeg<'a>>,
    /// the body is an audio transcription or translation, which may come back
    /// as plain text or subtitles rather than JSON (#3331)
    transcript: bool,
}

impl DeliveryPolicy<'_, '_> {
    /// Whether a leg that must read the whole body is set: the built-in
    /// output rules, the `post_response` plugins or the sanitizer's response
    /// leg. None of them can read an event stream (#3471).
    fn has_whole_body_leg(&self) -> bool {
        self.output.is_some() || self.plugins.is_some() || self.pii.is_some()
    }

    /// Finalise a buffered body with `status`. Only a successful body is
    /// examined; an error body passes through untouched.
    async fn apply(&self, status: StatusCode, events: &mut PolicyEvents, bytes: Bytes) -> Delivery {
        if !status.is_success() {
            return Delivery::Send {
                body: bytes,
                pre_restore: None,
            };
        }
        // a transcript answered as `text`, `srt` or `vtt` is no JSON object, so
        // every leg below is shown it as `{"text": …}` and the body delivered is
        // its text again. one that is not even UTF-8 is nothing a leg could
        // judge, and is withheld rather than delivered unchecked (#3331)
        let (bytes, plain_text) = if self.transcript
            && serde_json::from_slice::<serde_json::Map<String, Value>>(&bytes).is_err()
        {
            match std::str::from_utf8(&bytes) {
                Ok(text) => (
                    Bytes::from(serde_json::json!({ "text": text }).to_string()),
                    true,
                ),
                Err(_) => {
                    return Delivery::Refuse {
                        response: guardrail_output_blocked("unreadable_transcript"),
                        reason: guardrail_withheld_reason("unreadable_transcript"),
                    }
                }
            }
        } else {
            (bytes, false)
        };
        match self.finalise(events, bytes).await {
            Delivery::Send { body, pre_restore } if plain_text => Delivery::Send {
                body: transcript_text(body),
                pre_restore: pre_restore.map(transcript_text),
            },
            delivery => delivery,
        }
    }

    /// Run every leg over a successful body, in order.
    async fn finalise(&self, events: &mut PolicyEvents, bytes: Bytes) -> Delivery {
        let bytes = match self.output {
            Some(guard) => match guard.apply(&bytes, &mut events.guardrail) {
                crate::guardrails::OutputDecision::Unchanged => bytes,
                crate::guardrails::OutputDecision::Masked(masked) => masked,
                crate::guardrails::OutputDecision::Blocked(rule) => {
                    return Delivery::Refuse {
                        response: guardrail_output_blocked(&rule),
                        reason: guardrail_withheld_reason(&rule),
                    }
                }
            },
            None => bytes,
        };
        // the semantic policy decides on what the provider produced, after
        // the built-in rules masked it and before a plugin or the sanitizer
        // rewrites it (#459)
        let bytes = match self.semantic {
            Some(leg) => match leg.apply(bytes, &mut events.guardrail).await {
                Ok(bytes) => bytes,
                Err(refusal) => return *refusal,
            },
            None => bytes,
        };
        let bytes = match self.plugins {
            Some(ctx) => {
                match crate::plugin_dispatch::apply_post_response(ctx, &mut events.plugin, &bytes)
                    .await
                {
                    crate::plugin_dispatch::PostResponseOutcome::Bytes(bytes) => Bytes::from(bytes),
                    crate::plugin_dispatch::PostResponseOutcome::Block(reason) => {
                        let reason =
                            reason.unwrap_or_else(|| "response withheld by plugin".to_string());
                        return Delivery::Refuse {
                            response: crate::error::ApiError::new(
                                StatusCode::FORBIDDEN,
                                reason.clone(),
                            )
                            .with_code("plugin_blocked")
                            .into_response(),
                            reason: format!("plugin_blocked: {reason}"),
                        };
                    }
                }
            }
            None => bytes,
        };
        // last, so the sanitizer sees the body the caller would have received
        // and a restore is never handed to a plugin
        match self.pii {
            Some(leg) => {
                let (body, pre_restore) = pii_response_leg_apply(leg, bytes).await;
                Delivery::Send { body, pre_restore }
            }
            None => Delivery::Send {
                body: bytes,
                pre_restore: None,
            },
        }
    }
}

/// The text of a plain-text transcript [`DeliveryPolicy::apply`] wrapped as
/// `{"text": …}`. A leg that replaced the object with one carrying no `text`
/// string leaves it as the JSON it made.
fn transcript_text(body: Bytes) -> Bytes {
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(mut object)) => match object.remove("text") {
            Some(Value::String(text)) => Bytes::from(text),
            _ => body,
        },
        _ => body,
    }
}

/// The `400` for a streamed request on a route whose built-in output rules
/// cannot see a stream, under `streaming_post_call = "reject"` (#591). Shared
/// by the chat and multipart paths (#3432).
fn output_rules_stream_refusal(state: &AppState) -> Response {
    state
        .metrics
        .guardrail_stream_rejections_total
        .fetch_add(1, Relaxed);
    crate::error::ApiError::new(
        StatusCode::BAD_REQUEST,
        "this model applies output guardrails, which cannot be enforced on a streamed \
         response; retry without \"stream\": true",
    )
    .with_code("guardrail_streaming_unsupported")
    .with_param("stream")
    .into_response()
}

/// The `400` for a streamed request whose semantic guardrail policy checks
/// responses under `streaming = "reject"` (ADR-0037).
fn policy_stream_refusal(
    state: &AppState,
    guard: &crate::semantic_guards::SemanticGuard<'_>,
) -> Response {
    state
        .metrics
        .guardrail_stream_rejections_total
        .fetch_add(1, Relaxed);
    crate::error::ApiError::new(
        StatusCode::BAD_REQUEST,
        format!(
            "the guardrail policy '{}' checks responses, which cannot be applied to a streamed \
             response; retry without \"stream\": true",
            guard.policy.label()
        ),
    )
    .with_code("guardrail_streaming_unsupported")
    .with_param("stream")
    .into_response()
}

/// The `400` for a streamed request on a deployment whose PII sanitizer
/// answers a stream with [`rolter_core::StreamingResponse::Reject`] (#911).
fn pii_stream_refusal(state: &AppState) -> Response {
    state
        .metrics
        .pii_stream_rejections_total
        .fetch_add(1, Relaxed);
    crate::error::ApiError::new(
        StatusCode::BAD_REQUEST,
        "this deployment sanitizes or restores response content, which cannot be applied to a \
         streamed response; retry without \"stream\": true",
    )
    .with_code("pii_streaming_unsupported")
    .with_param("stream")
    .into_response()
}

/// The `400` for a streamed request when a fail-closed `post_response` plugin
/// must approve every response, which it cannot do for a stream (#1776).
/// `None` when every plugin is fail-open: those already chose delivery without
/// their verdict, which is all a stream would get.
fn plugin_stream_refusal(
    state: &AppState,
    plugins: &[&rolter_core::PluginInstanceConfig],
) -> Option<Response> {
    let plugin = plugins
        .iter()
        .find(|plugin| plugin.failure_mode == rolter_core::FailureMode::FailClosed)?;
    state
        .metrics
        .plugin_stream_rejections_total
        .fetch_add(1, Relaxed);
    Some(
        crate::error::ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "the fail-closed post_response plugin '{}' must approve every response, which \
                 cannot be done for a streamed response; retry without \"stream\": true",
                plugin.slug
            ),
        )
        .with_code("plugin_streaming_unsupported")
        .with_param("stream")
        .into_response(),
    )
}

/// A semantic guardrail policy's `post_call` providers, and whether the body
/// they judge is a buffered stream (ADR-0037).
#[derive(Clone, Copy)]
struct SemanticLeg<'p, 'a> {
    guard: &'p crate::semantic_guards::SemanticGuard<'a>,
    stream: bool,
}

impl SemanticLeg<'_, '_> {
    /// Whether this leg refuses a streamed request before the upstream call:
    /// the policy chose `streaming = "reject"` (ADR-0037).
    fn rejects_stream(&self) -> bool {
        self.stream && self.guard.policy.streaming == rolter_core::StreamingStrategy::Reject
    }

    /// Judge a successful body. A stream is shown to the providers as its
    /// assembled text; any other body as its JSON, or as `{"text": …}` when it
    /// is UTF-8 but not JSON. A binary body (audio, an image) is no text a
    /// classifier can judge and passes. Every provider's outcome is appended to
    /// `events` for the log row, as on the request stage (#3330); a refusal is
    /// also recorded through its withheld reason. The refusal is boxed: it is
    /// the rare arm, and `Delivery` is large.
    async fn apply(&self, bytes: Bytes, events: &mut Vec<String>) -> Result<Bytes, Box<Delivery>> {
        let content = if self.stream {
            crate::semantic_guards::stream_content(&bytes)
        } else if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            value
        } else if let Ok(text) = std::str::from_utf8(&bytes) {
            serde_json::json!({ "text": text })
        } else {
            return Ok(bytes);
        };
        match self
            .guard
            .check(
                rolter_core::WebhookStage::PostCall,
                &content,
                self.stream,
                events,
            )
            .await
        {
            crate::semantic_guards::Verdict::Allow(None) => Ok(bytes),
            // a buffered stream never gets here with a rewrite: `check`
            // withholds it instead
            crate::semantic_guards::Verdict::Allow(Some(rewritten)) => {
                Ok(serde_json::to_vec(&rewritten)
                    .map(Bytes::from)
                    .unwrap_or(bytes))
            }
            crate::semantic_guards::Verdict::Block { who, reason } => {
                Err(Box::new(Delivery::Refuse {
                    response: crate::error::ApiError::new(StatusCode::FORBIDDEN, reason)
                        .with_code("guardrail_blocked")
                        .into_response(),
                    reason: guardrail_withheld_reason(&who),
                }))
            }
        }
    }
}

/// Read a whole upstream body, or `None` once it outgrows `cap` bytes.
async fn read_bounded(
    mut response: reqwest::Response,
    cap: usize,
) -> reqwest::Result<Option<Bytes>> {
    if cap == usize::MAX {
        return response.bytes().await.map(Some);
    }
    let mut buffer = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if buffer.len() + chunk.len() > cap {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(Some(Bytes::from(buffer)))
}

/// The outcome of [`DeliveryPolicy::apply`]: the finalised body, or the reply
/// to send instead of it plus the reason recorded on the withheld log row.
enum Delivery {
    /// `pre_restore` is the placeholder-bearing body a PII restore replaced,
    /// the one payload capture keeps (#3240); `None` when nothing was restored
    Send {
        body: Bytes,
        pre_restore: Option<Bytes>,
    },
    Refuse {
        response: Response,
        reason: String,
    },
}

/// What the post-response policy did to one body, kept apart by the column
/// each lands in: the output rules' and semantic providers' outcomes in
/// `guardrail_events`, the `post_response` plugins' in `plugin_events`
/// (#3330).
#[derive(Default)]
struct PolicyEvents {
    guardrail: Vec<String>,
    plugin: Vec<String>,
}

impl PolicyEvents {
    /// Append the outcomes to the row the request leaves, each once: the
    /// request stage may already have recorded the same entry (a policy that
    /// flags both the request and the response), and the rollups count a
    /// request once per entry, so a repeat would count it twice.
    fn record(self, log: &mut RequestLog) {
        append_new(&mut log.guardrail_events, self.guardrail);
        append_new(&mut log.plugin_events, self.plugin);
    }
}

/// Append each of `entries` that `into` does not hold yet.
fn append_new(into: &mut Vec<String>, entries: Vec<String>) {
    for entry in entries {
        if !into.contains(&entry) {
            into.push(entry);
        }
    }
}

/// The withheld reason of a non-streamed request whose upstream answered with
/// an event stream the response guardrails cannot read (#3471).
const UNREQUESTED_STREAM: &str = "unrequested_stream";

/// Withhold an event stream the request did not ask for, from an upstream
/// whose answer a whole-body leg (the built-in output rules, a
/// `post_response` plugin, the sanitizer's response leg) must see and cannot
/// read as a stream (#3471). The body is read up to the buffered-stream cap
/// for what the provider billed, so the withheld row is charged like any
/// other; a longer body is withheld with its usage unknown.
async fn withhold_unrequested_stream(
    response: reqwest::Response,
    translation: &rolter_proxy::TranslationPlan,
    mut acct: Accounting,
) -> Response {
    let cap = crate::semantic_guards::MAX_BUFFERED_STREAM_BYTES;
    let billed = match read_bounded(response, cap).await {
        Ok(Some(bytes)) => {
            let bytes = translation.translate_response(bytes, true);
            acct.log.billed_units = acct.log.unit_basis.units(&bytes);
            crate::logging::parse_usage_with(true, &bytes, translation.thinking_count())
        }
        Ok(None) | Err(_) => crate::logging::Usage::default(),
    };
    withheld_response(
        guardrail_output_blocked(UNREQUESTED_STREAM),
        guardrail_withheld_reason(UNREQUESTED_STREAM),
        billed,
        true,
        acct,
    )
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

/// The log-row `error` for a completion a post-call rule withheld. Names the
/// rule, never the text it matched (#1478).
fn guardrail_withheld_reason(rule: &str) -> String {
    format!("{}{rule}", crate::logging::GUARDRAIL_WITHHELD_PREFIX)
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
    /// the `x-rolter-param-adjustments` value, `None` (and never built) when
    /// translation forwarded every parameter as sent (#1586)
    param_adjustments: Option<String>,
}

impl DecisionHeaders {
    fn from_log(log: &RequestLog) -> Self {
        Self {
            // the row carries the runtime id until it is queued, and the org
            // that owns the provider beside it (ADR-0040)
            provider: rolter_core::namespace::display_name(&log.provider, &log.provider_org_id)
                .to_string(),
            target: log.target.clone(),
            model: log.model.clone(),
            variant: log.variant.clone(),
            cache_hit: log.cache_hit != 0,
            param_adjustments: log.param_adjustments.header_value(),
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
        if let Some(value) = self
            .param_adjustments
            .as_deref()
            .and_then(|value| HeaderValue::from_str(value).ok())
        {
            headers.insert(rolter_proxy::PARAM_ADJUSTMENTS_HEADER, value);
        }
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

    /// #3535 review: with no pipeline installed an attempt hands on no span,
    /// so the stream wrapper parses nothing to record onto a disabled one. A
    /// variant attempt and an upload on the classic pool, the two kinds of
    /// attempt #3535 started tracing.
    #[tokio::test]
    async fn an_untraced_attempt_hands_on_no_span() {
        assert!(!rolter_core::telemetry::is_active());
        let app = axum::Router::new()
            .route(
                "/v1/chat/completions",
                axum::routing::post(|| async {
                    Json(json!({"id": "x", "choices": [], "usage": {"prompt_tokens": 1}}))
                }),
            )
            .route(
                "/v1/audio/transcriptions",
                axum::routing::post(|| async { Json(json!({"text": "hello"})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let up = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let target = || Target {
            provider: "up".into(),
            model: Some("m".into()),
            ..Default::default()
        };
        let route = |model: &str, targets, variants| ModelRoute {
            model: model.into(),
            strategy: BalancingStrategy::RoundRobin,
            targets,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants,
            tenancy: None,
        };
        let mut config = GatewayConfig::default();
        config.providers.push(rolter_core::ProviderConfig {
            name: "up".into(),
            api_base: format!("http://{up}"),
            ..Default::default()
        });
        config.routes.push(route("classic", vec![target()], vec![]));
        config.routes.push(route(
            "variants",
            vec![],
            vec![rolter_core::Variant {
                name: "only".into(),
                weight: 1,
                targets: vec![target()],
                params: Default::default(),
            }],
        ));
        let state = AppState::with_logging(&config, None);
        let snap = state.snapshot.load();

        let parsed = json!({"model": "variants", "messages": []});
        let body = Bytes::from(parsed.to_string());
        let upload = Bytes::from_static(b"--b\r\n\r\n--b--\r\n");
        let headers = HeaderMap::new();
        let ctx = RouteContext::new();
        let cases = [
            (
                "variants",
                "/v1/chat/completions",
                ForwardPayload::Json {
                    parsed: &parsed,
                    body: &body,
                    forward_body: &body,
                },
            ),
            (
                "classic",
                "/v1/audio/transcriptions",
                ForwardPayload::Multipart {
                    body: &upload,
                    content_type: "multipart/form-data; boundary=b",
                },
            ),
        ];
        for (model, path, payload) in cases {
            let resolved = snap.resolve_for(model, None).expect("the route resolves");
            let fwd = Forward {
                state: &state,
                snap: &snap,
                entry: &resolved,
                model,
                ctx: &ctx,
                payload,
                path,
                started: Instant::now(),
                trace_headers: &[],
                inbound: (&headers, &[]),
                key_meta: None,
            };
            let mut cancel = crate::cancel::CancelGuard::new(
                state.log.clone(),
                Instant::now(),
                RequestLog::default(),
            );
            let out = forward(&fwd, &mut cancel).await;
            cancel.disarm();
            assert_eq!(out.attempts, 1, "{model}");
            assert!(out.outcome.is_some(), "{model}: the upstream answered");
            assert!(out.genai_span.is_none(), "{model}: no span is handed on");
        }
    }

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

    /// #2919: the transport error quotes the address it could not reach. The
    /// 502 keeps the rest of the sentence, which is what tells a caller a
    /// connection failed rather than timed out, and masks what is credential.
    #[tokio::test]
    async fn the_gateway_error_masks_credentials_in_the_quoted_url() {
        let secret = format!("pw-{}", uuid::Uuid::new_v4().simple());
        let message = format!(
            "error sending request for url (https://svc:{secret}@llm.internal:8443/v1/chat?api_key={secret}&n=2)"
        );
        let response = upstream_error_response(&message);
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let rendered = body["error"]["message"].as_str().unwrap();
        assert!(!rendered.contains(&secret), "{rendered}");
        assert!(
            rendered.starts_with("error sending request for url ("),
            "{rendered}"
        );
        assert!(rendered.contains("llm.internal:8443"), "{rendered}");
        assert!(rendered.contains("n=2"), "{rendered}");
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
                ..Default::default()
            }],
            tenancy: None,
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
                ..Default::default()
            }],
            tenancy: None,
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

    // #2357: the Security screen's "enforce virtual keys" switch only ever
    // reached managed gateways, which fail closed anyway, so it was removed.
    // an older control plane still sends the field; neither value may open
    // or close anything
    #[tokio::test]
    async fn a_retired_snapshot_field_never_opens_a_managed_gateway() {
        for value in [false, true] {
            let config: GatewayConfig = serde_json::from_value(serde_json::json!({
                "security": { "virtual_key_required": value },
            }))
            .expect("an older control plane's snapshot must still parse");
            let mut state = AppState::new(&config);
            state.managed_auth = true;
            let resp = list_models(State(state), HeaderMap::new()).await;
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{value}");

            let state = AppState::new(&config);
            let resp = list_models(State(state), HeaderMap::new()).await;
            assert_eq!(resp.status(), StatusCode::OK, "unmanaged, {value}");
        }
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

    /// A config whose single provider serves five models but whose one route
    /// names only one of them — the shape of #1647.
    fn config_with_a_wide_provider() -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.providers.push(rolter_core::ProviderConfig {
            name: "openai-edge".to_string(),
            slug: Some("openai-edge".to_string()),
            ..Default::default()
        });
        config.routes.push(ModelRoute {
            model: "chat".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![Target {
                provider: "openai-edge".to_string(),
                model: Some("gpt-4o".to_string()),
                ..Default::default()
            }],
            tenancy: None,
        });
        config
    }

    /// The bug: a model the provider serves is routable through
    /// `Snapshot::resolve_pinned` but was never listed, because the listing was
    /// built from route targets alone.
    #[tokio::test]
    async fn list_models_lists_the_whole_probed_catalogue_not_only_route_targets() {
        let state = AppState::new(&config_with_a_wide_provider());
        state.model_catalog.record(
            "openai-edge",
            vec![
                "gpt-4o".to_string(),
                "gpt-4.1".to_string(),
                "o3-mini".to_string(),
            ],
        );
        let ids =
            models_in_response(list_models(State(state.clone()), HeaderMap::new()).await).await;
        for id in [
            "openai-edge/gpt-4o",
            "openai-edge/gpt-4.1",
            "openai-edge/o3-mini",
        ] {
            assert!(ids.contains(&id.to_string()), "{id} missing from {ids:?}");
            // every listed address is one the gateway will actually route
            assert!(
                state.snapshot.load().resolve_pinned(id).is_some(),
                "{id} is listed but not routable"
            );
        }
    }

    /// Without a probe the listing is exactly what it always was, so a
    /// deployment that never enables health checks sees no change.
    #[tokio::test]
    async fn an_unprobed_provider_keeps_the_route_derived_listing() {
        let state = AppState::new(&config_with_a_wide_provider());
        let ids = models_in_response(list_models(State(state), HeaderMap::new()).await).await;
        assert!(ids.contains(&"openai-edge/gpt-4o".to_string()), "{ids:?}");
        assert!(!ids.iter().any(|id| id == "openai-edge/o3-mini"), "{ids:?}");
    }

    async fn listed_entries(state: AppState) -> Vec<Value> {
        let resp = list_models(State(state), HeaderMap::new()).await;
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        value["data"].as_array().unwrap().clone()
    }

    /// A route that sets catalog metadata serves the machine-meaningful part of
    /// it; the free-text description stays out (#2939).
    #[tokio::test]
    async fn a_route_serves_its_capabilities_model_type_and_context_length() {
        let mut config = config_with_a_wide_provider();
        config.routes[0].advanced = rolter_core::AdvancedModelConfig {
            model_type: Some("chat".to_string()),
            capabilities: vec!["vision".to_string(), "tools".to_string()],
            context_length: Some(128_000),
            description: Some("internal: fails over to the eu pool".to_string()),
            ..Default::default()
        };
        let entries = listed_entries(AppState::new(&config)).await;
        let chat = entries.iter().find(|e| e["id"] == "chat").unwrap();
        assert_eq!(chat["capabilities"], json!(["vision", "tools"]));
        assert_eq!(chat["model_type"], "chat");
        assert_eq!(chat["context_length"], 128_000);
        assert!(chat.get("description").is_none(), "{chat}");
        // an address has no route, so it carries no metadata
        let pinned = entries
            .iter()
            .find(|e| e["id"] == "openai-edge/gpt-4o")
            .unwrap();
        assert!(pinned.get("context_length").is_none(), "{pinned}");
    }

    /// A route that sets nothing gains none of the catalog fields.
    #[tokio::test]
    async fn a_route_without_metadata_lists_no_catalog_fields() {
        let entries = listed_entries(AppState::new(&config_with_a_wide_provider())).await;
        let chat = entries.iter().find(|e| e["id"] == "chat").unwrap();
        for key in ["capabilities", "model_type", "context_length"] {
            assert!(chat.get(key).is_none(), "{key} listed for {chat}");
        }
    }

    /// The catalogue widens what is discoverable, never what a key may reach.
    #[tokio::test]
    async fn a_key_allow_list_still_filters_catalogued_models() {
        let mut config = config_with_a_wide_provider();
        config.virtual_keys.push(VirtualKeyConfig {
            key: "sk-pinned".to_string(),
            name: None,
            models: vec!["openai-edge/gpt-4o".to_string()],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
        });
        let state = AppState::new(&config);
        state.model_catalog.record(
            "openai-edge",
            vec!["gpt-4o".to_string(), "o3-mini".to_string()],
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-pinned"),
        );
        let ids = models_in_response(list_models(State(state), headers).await).await;
        assert_eq!(ids, vec!["openai-edge/gpt-4o".to_string()]);
    }

    /// A group address fans out over its members' models, so a member's probed
    /// catalogue has to widen the group listing too.
    #[tokio::test]
    async fn a_group_listing_widens_with_its_members_catalogue() {
        let mut config = config_with_a_wide_provider();
        config
            .provider_groups
            .push(rolter_core::ProviderGroupConfig {
                name: "edge fleet".to_string(),
                slug: Some("edge".to_string()),
                strategy: BalancingStrategy::RoundRobin,
                members: vec![rolter_core::GroupMember {
                    provider: "openai-edge".to_string(),
                    model: None,
                    weight: 1,
                }],
                tenancy: None,
                ..Default::default()
            });
        let state = AppState::new(&config);
        state
            .model_catalog
            .record("openai-edge", vec!["o3-mini".to_string()]);
        let ids = models_in_response(list_models(State(state), HeaderMap::new()).await).await;
        assert!(ids.contains(&"edge/o3-mini".to_string()), "{ids:?}");
    }

    #[tokio::test]
    async fn a_group_listing_resolves_to_the_same_group_as_a_request() {
        let mut config = GatewayConfig::default();
        let pepper = config.server.resolve_key_pepper();
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&pepper, "sk-project-b"),
            org_id: "org-a".to_string(),
            project_id: "proj-a2".to_string(),
            ..Default::default()
        });
        for (name, tenancy) in [
            ("org-edge", owned_by("org-a", Some("proj-a1"))),
            ("global-edge", None),
        ] {
            config.providers.push(rolter_core::ProviderConfig {
                name: name.to_string(),
                slug: Some(name.to_string()),
                tenancy,
                ..Default::default()
            });
        }
        for (name, provider, model, tenancy) in [
            ("org pool", "org-edge", "org-only", owned_by("org-a", None)),
            ("global pool", "global-edge", "global-only", None),
        ] {
            config
                .provider_groups
                .push(rolter_core::ProviderGroupConfig {
                    name: name.to_string(),
                    slug: Some("pool".to_string()),
                    members: vec![rolter_core::GroupMember {
                        provider: provider.to_string(),
                        model: Some(model.to_string()),
                        weight: 1,
                    }],
                    tenancy,
                    ..Default::default()
                });
        }
        let state = AppState::new(&config);
        let key = key_in("org-a", "proj-a2");
        let snapshot = state.snapshot.load();
        let resolved = snapshot
            .resolve_for("pool/global-only", Some(&key))
            .expect("the global group answers the request");
        assert_eq!(resolved.route.targets[0].provider, "global-edge");

        let ids = models_in_response(list_models(State(state), bearer("sk-project-b")).await).await;
        assert!(ids.contains(&"pool/global-only".to_string()), "{ids:?}");
        assert!(!ids.contains(&"pool/org-only".to_string()), "{ids:?}");
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
        let resp =
            chat_completions(State(state), HeaderMap::new(), RequestId::default(), body).await;
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
        let resp = messages(State(state), HeaderMap::new(), RequestId::default(), body).await;
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
        let resp = responses(State(state), HeaderMap::new(), RequestId::default(), body).await;
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
            RequestId::default(),
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
        let resp = completions(State(state), HeaderMap::new(), RequestId::default(), body).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn fake_llm_respects_virtual_key_auth() {
        let state = AppState::new(&config_with_keys());
        let body = Bytes::from(r#"{"model": "fake-llm", "messages": []}"#);
        let resp =
            chat_completions(State(state), HeaderMap::new(), RequestId::default(), body).await;
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
            key_hash: rolter_auth::hash_key(&pepper, "sk-db-key"),
            id: "vk-1".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "proj-1".to_string(),
            user_id: "user-1".to_string(),
            ..Default::default()
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

    fn rotated_key(
        current: &str,
        previous: &str,
        valid_until: chrono::DateTime<Utc>,
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> GatewayConfig {
        let mut config = GatewayConfig::default();
        let pepper = config.server.resolve_key_pepper();
        config.db_virtual_keys.push(VirtualKeyRecord {
            previous: Some(rolter_core::PreviousKeySecret {
                key_hash: rolter_auth::hash_key(&pepper, previous),
                valid_until,
            }),
            key_hash: rolter_auth::hash_key(&pepper, current),
            id: "vk-rotated".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "proj-1".to_string(),
            expires_at,
            ..Default::default()
        });
        config
    }

    /// A rotation's overlap (#1837): both secrets authenticate as the one key,
    /// then the old one stops at the end of its window while the new one lives on.
    #[tokio::test]
    async fn a_rotated_keys_previous_secret_authenticates_until_its_overlap_ends() {
        let open = rotated_key(
            "sk-new",
            "sk-old",
            Utc::now() + chrono::Duration::hours(1),
            None,
        );
        let state = AppState::new(&open);
        for (which, secret) in [("new", "sk-new"), ("previous", "sk-old")] {
            let resp = list_models(State(state.clone()), bearer(secret)).await;
            assert_eq!(resp.status(), StatusCode::OK, "the {which} secret");
        }
        // one key, not two: same identity and same tenant isolation, so a rate
        // limit or budget is not doubled by the second secret
        let snap = state.snapshot.load();
        let new = &snap.keys[&rolter_auth::hash_key(&snap.pepper, "sk-new")];
        let old = &snap.keys[&rolter_auth::hash_key(&snap.pepper, "sk-old")];
        assert_eq!(old.id, new.id);
        assert_eq!(old.tenant_key, new.tenant_key);

        let ended = rotated_key(
            "sk-new",
            "sk-old",
            Utc::now() - chrono::Duration::seconds(1),
            None,
        );
        let state = AppState::new(&ended);
        let resp = list_models(State(state.clone()), bearer("sk-old")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let resp = list_models(State(state), bearer("sk-new")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// The overlap never outlives the key: an old secret cannot authenticate
    /// after the key it belonged to has expired.
    #[tokio::test]
    async fn a_previous_secret_never_outlives_the_key_that_expires_first() {
        let config = rotated_key(
            "sk-new",
            "sk-old",
            Utc::now() + chrono::Duration::hours(1),
            Some(Utc::now() - chrono::Duration::minutes(1)),
        );
        let state = AppState::new(&config);
        for (which, secret) in [("new", "sk-new"), ("previous", "sk-old")] {
            let resp = list_models(State(state.clone()), bearer(secret)).await;
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "the {which} secret"
            );
        }
    }

    /// The shape of a fresh managed deployment (#2300): no provider, no route,
    /// and the one key the control plane mints for the Playground of a
    /// routeless project, scoped to the builtin alone. That key gets an answer
    /// from `fake-llm` and reaches nothing else, and a keyless call is still
    /// refused.
    #[tokio::test]
    async fn managed_gateway_serves_fake_llm_to_a_routeless_playground_key() {
        let mut config = GatewayConfig::default();
        let pepper = config.server.resolve_key_pepper();
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&pepper, "sk-playground"),
            id: "vk-playground".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "proj-1".to_string(),
            user_id: "user-1".to_string(),
            models: vec![fake_llm::MODEL_NAME.to_string()],
            expires_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            ..Default::default()
        });
        let mut state = AppState::new(&config);
        state.managed_auth = true;
        let chat = || Bytes::from(r#"{"model": "fake-llm", "messages": []}"#);

        let resp = chat_completions(
            State(state.clone()),
            bearer("sk-playground"),
            RequestId::default(),
            chat(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = messages(
            State(state.clone()),
            bearer("sk-playground"),
            RequestId::default(),
            chat(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = list_models(State(state.clone()), bearer("sk-playground")).await;
        assert_eq!(
            models_in_response(resp).await,
            vec![fake_llm::MODEL_NAME.to_string()]
        );

        // the key reaches the builtin and nothing else
        let other = Bytes::from(r#"{"model": "gpt-4o", "messages": []}"#);
        let resp = chat_completions(
            State(state.clone()),
            bearer("sk-playground"),
            RequestId::default(),
            other,
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        // and a managed gateway still wants a key, even for the builtin
        let resp =
            chat_completions(State(state), HeaderMap::new(), RequestId::default(), chat()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn db_virtual_key_carries_cost_attribution_into_logs() {
        let mut config = GatewayConfig::default();
        let pepper = config.server.resolve_key_pepper();
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&pepper, "sk-attributed"),
            id: "vk-1".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "proj-1".to_string(),
            user_id: "user-1".to_string(),
            business_unit_id: "bu-1".to_string(),
            customer_id: "cust-1".to_string(),
            ..Default::default()
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
                        ..Default::default()
                    },
                    Target {
                        provider: "b".to_string(),
                        ..Default::default()
                    },
                ],
            }],
            tenancy: None,
        };
        let ctx = RouteContext::default();
        // the balancer's pick leads; declared order forms the fallback tail
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]).into(),
            variant_balancers: vec![Box::new(Fixed(1))],
            route: route.clone(),
        };
        assert_eq!(
            variant_target_order(&entry, &ctx, 0, 2, &[], &|_| true),
            vec![1, 0]
        );
        // an out-of-range pick degrades to plain declared order
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]).into(),
            variant_balancers: vec![Box::new(Fixed(9))],
            route: route.clone(),
        };
        assert_eq!(
            variant_target_order(&entry, &ctx, 0, 2, &[], &|_| true),
            vec![0, 1]
        );
        // no balancer built for the variant: declared order
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]).into(),
            variant_balancers: Vec::new(),
            route,
        };
        assert_eq!(
            variant_target_order(&entry, &ctx, 0, 2, &[], &|_| true),
            vec![0, 1]
        );
    }

    /// A dead variant target drains to an empty queue. The variant's
    /// balancer must not read that zero as the least-loaded target, or the
    /// warm target looks overloaded and the spill leads with the dead one.
    #[test]
    fn a_dead_variant_target_does_not_shape_the_variants_pick() {
        let variant = rolter_core::Variant {
            name: "v".to_string(),
            weight: 1,
            params: Default::default(),
            targets: ["a", "b", "c"]
                .into_iter()
                .map(|provider| Target {
                    provider: provider.to_string(),
                    ..Default::default()
                })
                .collect(),
        };
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            targets: Vec::new(),
            variants: vec![variant.clone()],
            tenancy: None,
        };
        let ctx = RouteContext::new()
            .with_prompt(Some("system: you are a careful assistant\nuser: hi\n"));
        let cache_aware = rolter_balancer::CacheAware::new(3, 0.5);
        rolter_balancer::LoadBalancer::observe(&cache_aware, 1, &ctx);
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[]).into(),
            variant_balancers: vec![Box::new(cache_aware)],
            route,
        };
        let state = AppState::new(&config_with_keys());
        state.health.set("c", false);
        // ten in flight on the first target, three on the warm one, and none on
        // the dead one, which drained when it stopped taking traffic
        let key = variant_key("m", "v");
        let _in_flight: Vec<_> = std::iter::repeat_n(0, 10)
            .chain(std::iter::repeat_n(1, 3))
            .map(|ti| state.loads.begin(&key, ti))
            .collect();
        assert_eq!(state.loads.snapshot(&key, 3), vec![10, 3, 0]);
        let keys = [key.clone()];
        let candidates = variant_candidates(&state, &entry, &keys, &ctx, 0, None, false);
        assert_eq!(
            candidates.first(),
            Some(&(0, 1)),
            "the warm live target should lead: {candidates:?}"
        );
        // the same pick blind to eligibility leads with the dead target
        assert_eq!(
            variant_target_order(&entry, &ctx, 0, 3, &[10, 3, 0], &|_| true)[0],
            2
        );
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
                        ..Default::default()
                    }],
                },
                rolter_core::Variant {
                    name: "canary".to_string(),
                    weight: 1,
                    params: Default::default(),
                    targets: vec![Target {
                        provider: "b".to_string(),
                        ..Default::default()
                    }],
                },
            ],
            tenancy: None,
        });
        let snap = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        assert_eq!(
            snap.routes.get("", "ab").unwrap().variant_balancers.len(),
            2
        );
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
                    ..Default::default()
                },
                Target {
                    provider: "b".to_string(),
                    ..Default::default()
                },
            ],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]).into(),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let first = pick_untried(
            &entry,
            &ctx,
            &[],
            &[],
            &cd,
            &hh,
            &bb,
            "m",
            false,
            None,
            None,
        )
        .unwrap();
        // with the first target excluded, the fallback must choose the other one
        let second = pick_untried(
            &entry,
            &ctx,
            &[first],
            &[],
            &cd,
            &hh,
            &bb,
            "m",
            false,
            None,
            None,
        )
        .unwrap();
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
                None,
                None
            ),
            None
        );
    }

    fn three_target_entry() -> crate::state::RouteEntry {
        let target = |provider: &str| Target {
            provider: provider.to_string(),
            ..Default::default()
        };
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![target("a"), target("b"), target("c")],
            tenancy: None,
        };
        crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1, 1]).into(),
            variant_balancers: Vec::new(),
            route,
        }
    }

    /// [`three_target_entry`] with target `c` restricted to `takes_over`.
    fn entry_with_restricted_c(takes_over: &[&str]) -> crate::state::RouteEntry {
        let mut entry = three_target_entry();
        entry.route.targets[2].takes_over = takes_over.iter().map(|t| t.parse().unwrap()).collect();
        entry
    }

    /// #1584: a restricted target is never the first dial, whatever the
    /// balancer and the registries say, and not even when the fail-open path
    /// runs because every ordinary target is parked.
    #[test]
    fn a_restricted_target_is_never_the_first_dial() {
        let entry = entry_with_restricted_c(&["throttled"]);
        let ctx = RouteContext::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let cd = crate::cooldowns::Cooldowns::default();
        for _ in 0..12 {
            let pick = pick_untried(
                &entry,
                &ctx,
                &[],
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                None,
            );
            assert!(matches!(pick, Some(0 | 1)), "{pick:?}");
        }
        // every ordinary target parked: the fail-open pick is still ordinary
        cd.park("m", 0, 60);
        cd.park("m", 1, 60);
        let pick = pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true, None, None);
        assert!(matches!(pick, Some(0 | 1)), "{pick:?}");
        // and with every ordinary target tried, nothing is left for a first dial
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &[0, 1],
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                None
            ),
            None
        );
    }

    /// #1584: a restricted target follows a failure it takes over and is
    /// passed over for one it does not.
    #[test]
    fn a_restricted_target_follows_only_a_matching_failure() {
        use rolter_core::failover::FailureKind;
        let entry = entry_with_restricted_c(&["throttled"]);
        let ctx = RouteContext::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let throttled = Failure::of(FailureKind::Throttled);
        let server = Failure::of(FailureKind::ServerError);
        let tried = [0, 1];
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &tried,
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                Some(&throttled)
            ),
            Some(2)
        );
        assert!(has_untried_target(&entry, &tried, None, Some(&throttled)));
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &tried,
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                Some(&server)
            ),
            None
        );
        assert!(!has_untried_target(&entry, &tried, None, Some(&server)));
    }

    /// #1584: a refusal is never retried onto an ordinary target, only onto
    /// one that takes refusals (or the refusal's category) over.
    #[test]
    fn a_refusal_moves_only_to_a_target_that_takes_it_over() {
        let refused = Failure::refusal(
            400,
            br#"{"error":{"code":"content_filter","innererror":{"content_filter_result":{"hate":{"filtered":true}}}}}"#,
        )
        .unwrap();
        let ctx = RouteContext::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let cd = crate::cooldowns::Cooldowns::default();
        for (takes_over, expected) in [
            (&["refusal"][..], Some(2)),
            (&["refusal:hate"][..], Some(2)),
            (&["refusal:sexual"][..], None),
            (&["throttled"][..], None),
        ] {
            let entry = entry_with_restricted_c(takes_over);
            assert_eq!(
                pick_untried(
                    &entry,
                    &ctx,
                    &[0],
                    &[],
                    &cd,
                    &hh,
                    &bb,
                    "m",
                    false,
                    None,
                    Some(&refused)
                ),
                expected,
                "{takes_over:?}"
            );
        }
    }

    /// #2835: the check the loops make before backing off must agree with the
    /// pick they make after it, for every set of tried targets, every key
    /// restriction and with each target parked on a cooldown. If it said "yes"
    /// where the pick finds nothing the request would wait for nothing, and if
    /// it said "no" where the pick finds a target the request would skip a wait
    /// it owes.
    #[test]
    fn has_untried_target_agrees_with_pick_untried() {
        let entry = three_target_entry();
        let ctx = RouteContext::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let keys = [
            None,
            Some(KeyMeta::default()),
            Some(KeyMeta {
                providers: vec!["b".to_string()],
                ..Default::default()
            }),
            Some(KeyMeta {
                providers: vec!["a".to_string(), "c".to_string()],
                ..Default::default()
            }),
            Some(KeyMeta {
                providers: vec!["missing".to_string()],
                ..Default::default()
            }),
        ];
        for tried_mask in 0u8..8 {
            let tried: Vec<usize> = (0..3).filter(|i| tried_mask & (1 << i) != 0).collect();
            for parked_mask in 0u8..8 {
                let cd = crate::cooldowns::Cooldowns::default();
                for i in (0..3).filter(|i| parked_mask & (1 << i) != 0) {
                    cd.park("m", i, 60);
                }
                for key in &keys {
                    let picked = pick_untried(
                        &entry,
                        &ctx,
                        &tried,
                        &[],
                        &cd,
                        &hh,
                        &bb,
                        "m",
                        true,
                        key.as_ref(),
                        None,
                    );
                    assert_eq!(
                        has_untried_target(&entry, &tried, key.as_ref(), None),
                        picked.is_some(),
                        "tried {tried:?}, parked {parked_mask:03b}, key {key:?}, picked {picked:?}"
                    );
                }
            }
        }
    }

    /// The multi-key path re-allows its own target with `tried.pop()`, and the
    /// check has to see that: it is the case the wait is kept for.
    #[test]
    fn a_target_popped_from_tried_is_untried_again() {
        let entry = three_target_entry();
        let mut tried = vec![0, 1, 2];
        assert!(!has_untried_target(&entry, &tried, None, None));
        tried.pop();
        assert!(has_untried_target(&entry, &tried, None, None));
    }

    /// One variant `v` with targets on providers `a` and `b`, `b` restricted to
    /// `takes_over` when that is not empty.
    fn variant_route(takes_over: &[&str]) -> rolter_core::ModelRoute {
        serde_json::from_value(json!({
            "model": "m",
            "variants": [{
                "name": "v",
                "targets": [
                    {"provider": "a"},
                    {"provider": "b", "takes_over": takes_over},
                ],
            }],
        }))
        .unwrap()
    }

    #[test]
    fn has_untried_candidate_counts_the_variant_candidates_left() {
        let route = variant_route(&[]);
        let candidates = [(0, 0), (0, 1)];
        assert!(!has_untried_candidate(&route, &[], &[], None));
        assert!(has_untried_candidate(&route, &candidates, &[], None));
        assert!(has_untried_candidate(&route, &candidates, &[1], None));
        assert!(!has_untried_candidate(&route, &candidates, &[1, 0], None));
        // a candidate popped for a sibling key is untried again
        assert!(has_untried_candidate(&route, &candidates[..1], &[], None));
    }

    /// #1584: a restricted variant target is a candidate only after a failure
    /// it takes over.
    #[test]
    fn a_restricted_variant_candidate_follows_only_a_matching_failure() {
        use rolter_core::failover::FailureKind;
        let route = variant_route(&["throttled"]);
        let candidates = [(0, 0), (0, 1)];
        let throttled = Failure::of(FailureKind::Throttled);
        let server = Failure::of(FailureKind::ServerError);
        assert!(!candidate_admits(&route, (0, 1), None));
        assert!(candidate_admits(&route, (0, 0), None));
        assert!(!has_untried_candidate(
            &route,
            &candidates,
            &[0],
            Some(&server)
        ));
        assert!(has_untried_candidate(
            &route,
            &candidates,
            &[0],
            Some(&throttled)
        ));
    }

    /// #2835: a retry that cannot happen is not waited for, and one that can
    /// is waited for in full. The clock is paused, so the elapsed time is the
    /// virtual time `back_off` asked the runtime to sleep through: exact, and
    /// out of reach of a descheduled or slow test process (#3416).
    #[tokio::test(start_paused = true)]
    async fn back_off_waits_only_for_an_attempt_that_can_happen() {
        let delay = 250;
        let start = tokio::time::Instant::now();
        back_off(delay, false).await;
        assert_eq!(start.elapsed(), Duration::ZERO, "a refused retry waited");
        back_off(delay, true).await;
        assert_eq!(start.elapsed(), Duration::from_millis(delay));
    }

    /// #1714: a balancer sized for more targets than the route carries must
    /// not panic the worker. Round robin over four slots walks past the one
    /// real target within a few calls; every call still has to land on it.
    #[test]
    fn pick_untried_survives_a_balancer_pick_past_the_target_list() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![Target {
                provider: "a".to_string(),
                ..Default::default()
            }],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1, 1, 1]).into(),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        for _ in 0..8 {
            assert_eq!(
                pick_untried(
                    &entry,
                    &ctx,
                    &[],
                    &[],
                    &cd,
                    &hh,
                    &bb,
                    "m",
                    false,
                    None,
                    None
                ),
                Some(0)
            );
        }
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
                    ..Default::default()
                },
                Target {
                    provider: "b".to_string(),
                    ..Default::default()
                },
            ],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]).into(),
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
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true, None, None),
            Some(1)
        );
        // park both: fail open to an untried target rather than returning None
        cd.park("m", 1, 60);
        assert!(
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true, None, None).is_some()
        );
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
                    ..Default::default()
                },
                Target {
                    provider: "b".to_string(),
                    ..Default::default()
                },
            ],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]).into(),
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
            pick_untried(
                &entry,
                &ctx,
                &[],
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                None
            ),
            Some(1)
        );
        // both providers unhealthy: fail open rather than returning None
        hh.set("b", false);
        assert!(pick_untried(
            &entry,
            &ctx,
            &[],
            &[],
            &cd,
            &hh,
            &bb,
            "m",
            false,
            None,
            None
        )
        .is_some());
    }

    /// #1851's load guard against a pool with a dead replica: its load stays
    /// at 0, and counting it made every warm replica with three requests in
    /// flight look overloaded, so the spill went to the dead replica and then
    /// on to target 0 by index, cache or no cache.
    #[test]
    fn cache_aware_balances_against_live_targets_only() {
        let target = |provider: &str| Target {
            provider: provider.to_string(),
            ..Default::default()
        };
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::CacheAware,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![target("a"), target("b"), target("c")],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1, 1]).into(),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::new()
            .with_prompt(Some("system: you are a careful assistant\nuser: hi\n"));
        entry.balancer.observe(1, &ctx);
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::new();
        let bb = crate::breaker::Breaker::default();
        hh.set("c", false);
        let loads = [10, 3, 0];
        // the warm replica is the least loaded of the live ones: it keeps the
        // request rather than losing it to busy target 0
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &[],
                &loads,
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                None
            ),
            Some(1)
        );
        // once it was tried, the spill goes to the other live replica
        assert_eq!(
            pick_untried(
                &entry,
                &ctx,
                &[1],
                &loads,
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                None
            ),
            Some(0)
        );
        // with every replica down the pick fails open, as before
        hh.set("a", false);
        hh.set("b", false);
        assert!(pick_untried(
            &entry,
            &ctx,
            &[],
            &loads,
            &cd,
            &hh,
            &bb,
            "m",
            false,
            None,
            None
        )
        .is_some());
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
                    ..Default::default()
                },
                Target {
                    provider: "b".to_string(),
                    ..Default::default()
                },
            ],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]).into(),
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
            pick_untried(
                &entry,
                &ctx,
                &[],
                &[],
                &cd,
                &hh,
                &bb,
                "m",
                false,
                None,
                None
            ),
            Some(1)
        );
        // both open: fail open to an untried target rather than returning None
        assert!(bb.on_failure("m", 1));
        assert!(pick_untried(
            &entry,
            &ctx,
            &[],
            &[],
            &cd,
            &hh,
            &bb,
            "m",
            false,
            None,
            None
        )
        .is_some());
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
                ..Default::default()
            }],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1]).into(),
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
                    ..Default::default()
                },
                Target {
                    provider: "b".to_string(),
                    ..Default::default()
                },
            ],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1, 1]).into(),
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
                None,
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
                None,
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
        let entry = snapshot.routes.get("", "gpt-4o").unwrap();
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

    fn key_in(org: &str, project: &str) -> KeyMeta {
        KeyMeta {
            org_id: org.to_string(),
            project_id: project.to_string(),
            ..Default::default()
        }
    }

    fn owned_by(org: &str, project: Option<&str>) -> Option<rolter_core::Tenancy> {
        Some(rolter_core::Tenancy {
            org_id: org.to_string(),
            project_id: project.map(str::to_string),
        })
    }

    #[test]
    fn a_route_is_served_to_its_whole_org_and_to_no_other() {
        let mut config = config_with_keys();
        config.routes[0].tenancy = owned_by("org-a", Some("proj-a1"));
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        let entry = snapshot.routes.get("", "gpt-4o").unwrap();

        // org-wide by default: every project of the owning org
        assert!(model_visible_to(Some(&key_in("org-a", "proj-a1")), entry));
        assert!(model_visible_to(Some(&key_in("org-a", "proj-a2")), entry));
        // another org never, and it is refused before any policy is consulted
        assert_eq!(
            authorize_route(Some(&key_in("org-b", "proj-b1")), entry),
            Err(AccessDenial::NotVisible)
        );
        // a key from the gateway's own config file has no org: the operator's
        assert!(model_visible_to(Some(&key_in("", "")), entry));
        // a route with no tenancy (config file) belongs to the deployment
        let global = snapshot.routes.get("", "claude").unwrap();
        assert!(model_visible_to(Some(&key_in("org-b", "proj-b1")), global));
    }

    #[test]
    fn a_project_only_route_is_served_to_its_own_project() {
        let mut config = config_with_keys();
        config.routes[0].tenancy = owned_by("org-a", Some("proj-a1"));
        config.routes[0].advanced.visibility.project_only = true;
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        let entry = snapshot.routes.get("", "gpt-4o").unwrap();

        assert!(model_visible_to(Some(&key_in("org-a", "proj-a1")), entry));
        assert!(!model_visible_to(Some(&key_in("org-a", "proj-a2")), entry));
        assert!(!model_visible_to(Some(&key_in("org-b", "proj-a1")), entry));
        assert!(model_visible_to(Some(&key_in("", "")), entry));
    }

    #[test]
    fn provider_and_group_addresses_carry_their_owners_org() {
        let mut config = config_with_keys();
        config.providers.push(rolter_core::ProviderConfig {
            name: "edge".to_string(),
            slug: Some("edge".to_string()),
            api_base: "http://127.0.0.1:9".to_string(),
            tenancy: owned_by("org-a", None),
            ..Default::default()
        });
        config
            .provider_groups
            .push(rolter_core::ProviderGroupConfig {
                name: "pool".to_string(),
                slug: Some("pool".to_string()),
                strategy: BalancingStrategy::RoundRobin,
                members: vec![rolter_core::GroupMember {
                    provider: "edge".to_string(),
                    model: None,
                    weight: 1,
                }],
                tenancy: owned_by("org-a", None),
                ..Default::default()
            });
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        for address in ["edge/gpt-4o", "pool/gpt-4o"] {
            let entry = snapshot.resolve_pinned(address).unwrap();
            assert!(
                authorize_route(Some(&key_in("org-a", "proj-a1")), &entry).is_ok(),
                "{address}"
            );
            // another org's key reaches neither the provider nor the group (#1844)
            assert_eq!(
                authorize_route(Some(&key_in("org-b", "proj-b1")), &entry),
                Err(AccessDenial::NotVisible),
                "{address}"
            );
        }
    }

    /// A route another org owns is, for this caller, a route that does not
    /// exist: it must not shadow this org's own provider or group address, nor
    /// the builtin, and the answer must be the unknown-model one.
    #[test]
    fn another_orgs_route_shadows_nothing_for_this_org() {
        let mut config = config_with_keys();
        let pinned_to = |provider: &str| {
            vec![Target {
                provider: provider.to_string(),
                ..Default::default()
            }]
        };
        for (name, org) in [("edge", "org-b"), ("squatter", "org-a")] {
            config.providers.push(rolter_core::ProviderConfig {
                name: name.to_string(),
                slug: Some(name.to_string()),
                api_base: "http://127.0.0.1:9".to_string(),
                tenancy: owned_by(org, None),
                ..Default::default()
            });
        }
        config
            .provider_groups
            .push(rolter_core::ProviderGroupConfig {
                name: "pool".to_string(),
                slug: Some("pool".to_string()),
                strategy: BalancingStrategy::RoundRobin,
                members: vec![rolter_core::GroupMember {
                    provider: "edge".to_string(),
                    model: None,
                    weight: 1,
                }],
                tenancy: owned_by("org-b", None),
                ..Default::default()
            });
        // org-a squats org-b's addresses and the builtin with named routes
        for model in ["edge/gpt-4o", "pool/gpt-4o", fake_llm::MODEL_NAME] {
            config.routes.push(ModelRoute {
                model: model.to_string(),
                strategy: BalancingStrategy::RoundRobin,
                params: Default::default(),
                param_policy: Default::default(),
                advanced: Default::default(),
                cache: None,
                variants: Default::default(),
                targets: pinned_to("squatter"),
                tenancy: owned_by("org-a", Some("proj-a1")),
            });
        }
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        let org_a = key_in("org-a", "proj-a1");
        let org_b = key_in("org-b", "proj-b1");
        let org_c = key_in("org-c", "proj-c1");

        for address in ["edge/gpt-4o", "pool/gpt-4o"] {
            // org-b reaches its own provider and group, not org-a's route
            let entry = snapshot.resolve_for(address, Some(&org_b)).unwrap();
            assert!(
                matches!(entry, crate::state::ResolvedRoute::Pinned(_)),
                "{address}"
            );
            assert!(
                entry.route.targets.iter().all(|t| t.provider == "edge"),
                "{address}"
            );
            assert!(authorize_route(Some(&org_b), &entry).is_ok(), "{address}");
            // org-a keeps the route it named
            let entry = snapshot.resolve_for(address, Some(&org_a)).unwrap();
            assert!(
                matches!(entry, crate::state::ResolvedRoute::Named(_)),
                "{address}"
            );
            // a third org gets the unknown-model answer for both, never a
            // refusal that would confirm either exists
            assert!(
                snapshot.resolve_for(address, Some(&org_c)).is_none(),
                "{address}"
            );
        }
        // org-a's `fake-llm` route shadows the builtin for org-a alone
        assert!(snapshot
            .named_route_for(fake_llm::MODEL_NAME, Some(&org_a))
            .is_some());
        assert!(snapshot
            .named_route_for(fake_llm::MODEL_NAME, Some(&org_b))
            .is_none());
        // the operator's own key (no org) sees every row, as it always did
        assert!(snapshot
            .named_route_for("edge/gpt-4o", Some(&key_in("", "")))
            .is_some());
    }

    /// A project-only route of another project is a miss too, so the org's own
    /// pinned address behind it still answers.
    #[test]
    fn a_project_only_route_shadows_nothing_for_another_project() {
        let mut config = config_with_keys();
        config.providers.push(rolter_core::ProviderConfig {
            name: "edge".to_string(),
            slug: Some("edge".to_string()),
            api_base: "http://127.0.0.1:9".to_string(),
            tenancy: owned_by("org-a", None),
            ..Default::default()
        });
        config.routes[0].model = "edge/gpt-4o".to_string();
        config.routes[0].tenancy = owned_by("org-a", Some("proj-a1"));
        config.routes[0].advanced.visibility.project_only = true;
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        assert!(matches!(
            snapshot.resolve_for("edge/gpt-4o", Some(&key_in("org-a", "proj-a1"))),
            Some(crate::state::ResolvedRoute::Named(_))
        ));
        assert!(matches!(
            snapshot.resolve_for("edge/gpt-4o", Some(&key_in("org-a", "proj-a2"))),
            Some(crate::state::ResolvedRoute::Pinned(_))
        ));
    }

    #[test]
    fn route_gate_reports_the_first_gate_that_refused() {
        let mut config = config_with_keys();
        config.routes[0]
            .advanced
            .visibility
            .allowed_key_ids
            .push("allowed-key".to_string());
        let snapshot = crate::state::Snapshot::build(&config, &crate::load::LoadTracker::default());
        let entry = snapshot.routes.get("", "gpt-4o").unwrap();
        let deny_route = rolter_core::ModelPolicy {
            denied_routes: vec!["gpt-4o".to_string()],
            ..Default::default()
        };

        // visibility is checked first, so a hidden route never reveals whether
        // the caller's policy or provider list would also have refused it
        let hidden = KeyMeta {
            id: "other-key".to_string(),
            access_policy: Some(deny_route.clone()),
            providers: vec!["missing".to_string()],
            ..Default::default()
        };
        assert_eq!(
            authorize_route(Some(&hidden), entry),
            Err(AccessDenial::NotVisible)
        );
        assert_eq!(authorize_route(None, entry), Err(AccessDenial::NotVisible));

        let policy_denied = KeyMeta {
            id: "allowed-key".to_string(),
            access_policy: Some(deny_route),
            providers: vec!["missing".to_string()],
            ..Default::default()
        };
        assert_eq!(
            authorize_route(Some(&policy_denied), entry),
            Err(AccessDenial::RoutePolicy)
        );

        let provider_denied = KeyMeta {
            id: "allowed-key".to_string(),
            providers: vec!["missing".to_string()],
            ..Default::default()
        };
        assert_eq!(
            authorize_route(Some(&provider_denied), entry),
            Err(AccessDenial::NoProvider)
        );

        let allowed = KeyMeta {
            id: "allowed-key".to_string(),
            ..Default::default()
        };
        assert_eq!(authorize_route(Some(&allowed), entry), Ok(()));
        assert_eq!(
            authorize_model(
                Some(&KeyMeta {
                    models: vec!["x".to_string()],
                    ..Default::default()
                }),
                "gpt-4o"
            ),
            Err(AccessDenial::ModelNotAllowed)
        );
        assert_eq!(authorize_model(None, "gpt-4o"), Ok(()));
    }

    /// #1942: the load-aware strategies weigh only the targets the attempt can
    /// use. Target 0 reads as the emptiest in every case here, so a strategy
    /// that ignores eligibility hands it back and `pick_untried` falls over to
    /// the lowest usable index (target 1, the busiest) instead of target 2.
    #[test]
    fn pick_untried_weighs_load_only_among_usable_targets() {
        enum Dead {
            Parked,
            Unhealthy,
            BreakerOpen,
        }
        for strategy in [BalancingStrategy::PowerOfTwo, BalancingStrategy::Pipeline] {
            for dead in [Dead::Parked, Dead::Unhealthy, Dead::BreakerOpen] {
                let route = ModelRoute {
                    model: "m".to_string(),
                    strategy,
                    params: Default::default(),
                    param_policy: Default::default(),
                    advanced: Default::default(),
                    cache: None,
                    variants: Default::default(),
                    targets: ["a", "b", "c"]
                        .map(|provider| Target {
                            provider: provider.to_string(),
                            ..Default::default()
                        })
                        .to_vec(),
                    tenancy: None,
                };
                let entry = crate::state::RouteEntry {
                    namespace: route.model.clone().into(),
                    project_scope: None,
                    overrides: Default::default(),
                    throughput: None,
                    guardrails: Default::default(),
                    balancer: rolter_balancer::build(route.strategy, &[1, 1, 1]).into(),
                    variant_balancers: Vec::new(),
                    route,
                };
                let ctx = RouteContext::default();
                let cd = crate::cooldowns::Cooldowns::new();
                let hh = crate::health::Health::new();
                let bb = crate::breaker::Breaker::new(true, 1, 3600);
                match dead {
                    Dead::Parked => cd.park("m", 0, 60),
                    Dead::Unhealthy => hh.set("a", false),
                    Dead::BreakerOpen => {
                        bb.on_failure("m", 0);
                    }
                }
                let loads = [0, 9, 1];
                for _ in 0..100 {
                    assert_eq!(
                        pick_untried(
                            &entry,
                            &ctx,
                            &[],
                            &loads,
                            &cd,
                            &hh,
                            &bb,
                            "m",
                            true,
                            None,
                            None
                        ),
                        Some(2),
                        "{strategy:?}"
                    );
                }
            }
        }
    }

    /// An open breaker whose window has elapsed still admits its probe: the
    /// mask does not count it as blocked, and the pick moves it to half-open.
    #[test]
    fn pick_untried_admits_a_probe_after_the_open_window() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![Target {
                provider: "a".to_string(),
                ..Default::default()
            }],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1]).into(),
            variant_balancers: Vec::new(),
            route,
        };
        let bb = crate::breaker::Breaker::new(true, 1, 0);
        bb.on_failure("m", 0);
        assert!(bb.open_mask("m", 1).is_empty());
        let picked = pick_untried(
            &entry,
            &RouteContext::default(),
            &[],
            &[],
            &crate::cooldowns::Cooldowns::new(),
            &crate::health::Health::default(),
            &bb,
            "m",
            true,
            None,
            None,
        );
        assert_eq!(picked, Some(0));
    }

    /// Fail-open wins over the half-open probe limit (#3389): on a route whose
    /// only target is probing, a second request is still forwarded to it
    /// rather than refused, so a single-target route gets no stampede
    /// protection from the one-probe rule.
    #[test]
    fn pick_untried_fails_open_past_a_probe_in_flight() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
            targets: vec![Target {
                provider: "a".to_string(),
                ..Default::default()
            }],
            tenancy: None,
        };
        let entry = crate::state::RouteEntry {
            namespace: route.model.clone().into(),
            project_scope: None,
            overrides: Default::default(),
            throughput: None,
            guardrails: Default::default(),
            balancer: rolter_balancer::build(route.strategy, &[1]).into(),
            variant_balancers: Vec::new(),
            route,
        };
        let cooldowns = crate::cooldowns::Cooldowns::new();
        let health = crate::health::Health::default();
        let bb = crate::breaker::Breaker::new(true, 1, 0);
        bb.on_failure("m", 0);
        let pick = || {
            pick_untried(
                &entry,
                &RouteContext::default(),
                &[],
                &[],
                &cooldowns,
                &health,
                &bb,
                "m",
                true,
                None,
                None,
            )
        };
        // the first request takes the probe slot
        assert_eq!(pick(), Some(0));
        // the breaker refuses everyone else while the probe is in flight
        assert!(bb.refuses("m", 0));
        assert!(!bb.allows("m", 0));
        // and the route still forwards: fail-open, not a 503
        assert_eq!(pick(), Some(0));
        assert_eq!(pick(), Some(0));
    }
}
