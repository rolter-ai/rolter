//! Authenticated MCP HTTP proxying.
//!
//! A virtual key authenticates the workload, while its database owner selects
//! the user's OAuth session. Server, owner and scope checks all happen before
//! any downstream connection is attempted.

// spec-ref: mcp-transport
use axum::body::Body;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;

use rolter_core::{McpAuthKind, McpOrgSettingsConfig, McpServerConfig};
use rolter_proxy::{McpTransportOverrides, McpUpstreamAuth};

use std::sync::Arc;

use crate::mcp_list_filter::{FilteredSse, ListFilter, MAX_LIST_REPLY};
use crate::mcp_log::{CallContext, PendingCall};
use crate::mcp_policy::{ambiguous_message, Verdict};
use crate::state::AppState;
use crate::trace::RequestId;

/// What a caller is told when the MCP server could not be reached or did not
/// answer: the same sentence for every transport failure, with the cause in the
/// gateway log.
const MCP_UPSTREAM_FAILED: &str = "MCP upstream request failed";

#[derive(Deserialize)]
pub(crate) struct McpRootPath {
    server: String,
}

#[derive(Deserialize)]
pub(crate) struct McpTailPath {
    server: String,
    path: String,
}

pub(crate) async fn proxy_root(
    State(state): State<AppState>,
    Path(path): Path<McpRootPath>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(
        state,
        path.server,
        None,
        uri.query(),
        method,
        headers,
        request_id,
        body,
    )
    .await
}

pub(crate) async fn proxy_tail(
    State(state): State<AppState>,
    Path(path): Path<McpTailPath>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    proxy(
        state,
        path.server,
        Some(path.path),
        uri.query(),
        method,
        headers,
        request_id,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn proxy(
    state: AppState,
    server_slug: String,
    tail: Option<String>,
    query: Option<&str>,
    method: Method,
    headers: HeaderMap,
    request_id: RequestId,
    body: Bytes,
) -> Response {
    let snapshot = state.snapshot.load();
    // "/mcp", not the real request path: auth bypass routes are validated to
    // start with "/v1/", so the MCP surface can never be named by one, and
    // handing the real path in would only invite that to change quietly
    let key = match crate::handlers::authenticate(&state, &snapshot, &headers, "/mcp") {
        Ok(Some(key)) => key,
        Ok(None) => {
            return error(
                StatusCode::UNAUTHORIZED,
                "MCP proxy requires a virtual key",
                "mcp_key_required",
            )
        }
        Err(response) => return response,
    };
    if key.org_id.is_empty() || key.user_id.is_empty() {
        return error(
            StatusCode::FORBIDDEN,
            "MCP proxy requires a user-owned database virtual key",
            "mcp_owner_required",
        );
    }
    let Some(server) = snapshot.mcp_servers.get(&(key.org_id.clone(), server_slug)) else {
        return error(
            StatusCode::NOT_FOUND,
            "MCP server was not found for this organization",
            "mcp_server_not_found",
        );
    };
    if !matches!(server.transport.as_str(), "streamable_http" | "sse") {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "MCP server '{}' uses transport '{}', which is not available on the HTTP proxy path",
                server.slug, server.transport
            ),
            "mcp_transport_unsupported",
        );
    }
    // the body is read once, for the tool policy and the MCP tool-call log
    // alike (#3399): a `tools/call` the policy lets through is recorded under
    // the same case-folded keys it was judged by. Nothing is parsed when no rule
    // applies and no ClickHouse is configured, so the common path pays two flag
    // checks
    let caller = crate::mcp_policy::Caller {
        org_id: &key.org_id,
        key_id: &key.id,
        project_id: &key.project_id,
        team_id: &key.team_id,
    };
    let rules = crate::mcp_policy::Rules::of(&snapshot, server, &caller);
    let is_post = method == Method::POST;
    // a body the relay cannot decode cannot be judged, and the upstream may
    // decode it (#3475). refused before any policy, as is any body that does
    // not parse (#3527): the relay forwards only a body it has judged
    if is_post && rules.any() && is_encoded(&headers) {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!(
                "the request to MCP server '{}' has a Content-Encoding other than identity, so \
                 the organization's tool groups cannot be applied to it; send the body uncompressed",
                server.slug
            ),
            "mcp_request_unreadable",
        );
    }
    // a governed body is judged without a leading UTF-8 byte-order mark, and
    // forwarded without it, so the upstream receives exactly what was judged;
    // anything else that does not parse is refused below (#3527)
    let body = if is_post && rules.any() {
        crate::mcp_policy::without_bom(body)
    } else {
        body
    };
    // an encoded body to an ungoverned server is forwarded as it came (#3475),
    // so it is not parsed: its bytes are not the JSON-RPC the server reads
    let encoded = is_post && is_encoded(&headers);
    let read = if is_post && !encoded {
        crate::mcp_policy::Read::of(&body, rules.any(), state.mcp_events.enabled())
    } else {
        crate::mcp_policy::Read::Skipped
    };
    // a body forwarded to an ungoverned server without being read: its calls
    // cannot be itemised, so it is counted, and recorded as one `unread` row
    // (#3528). a body is read for an ungoverned server only when the MCP log
    // wants its calls, so only then can one be found unparseable
    let unread = if !is_post || rules.any() || body.is_empty() {
        None
    } else if encoded {
        Some("encoded")
    } else if matches!(read, crate::mcp_policy::Read::Unreadable) {
        Some("unparseable")
    } else {
        None
    };
    // the calls the body holds are recorded in the MCP tool-call log (#2395):
    // by the refusal that stops the request, or by the reply once it is
    // forwarded. they are taken out of the parsed body at that point, so a
    // request that is refused for something else never pays for them
    let capture = &snapshot.logging.payload_capture;
    let keep_arguments = capture.enabled && capture.max_bytes > 0;
    let context = CallContext {
        server: &server.slug,
        transport: &server.transport,
        org_id: &key.org_id,
        team_id: &key.team_id,
        project_id: &key.project_id,
        virtual_key_id: &key.id,
        // the session is looked up by this id below, so on a per-user OAuth
        // server it is the session owner
        user_id: &key.user_id,
        request_id: request_id.as_str(),
        capture,
        redactor: &snapshot.payload_redactor,
    };
    // the credential is chosen by what the server row says it is. only `oauth`
    // consults the per-user session; the static kinds are a deployment-wide
    // credential and deliberately do not require one, which is the whole point
    // of #952 — before it, every kind fell through to the session check and a
    // configured bearer token did nothing
    let auth = match server.auth_kind {
        McpAuthKind::Oauth => {
            let Some(session) = snapshot
                .mcp_oauth_sessions
                .get(&(server.id.clone(), key.user_id.clone()))
            else {
                fail_all(
                    pending_calls(&state, read, keep_arguments, &context),
                    "auth_denied",
                );
                return error(
                    StatusCode::FORBIDDEN,
                    "no live MCP OAuth session authorizes this user and server",
                    "mcp_session_unauthorized",
                );
            };
            if chrono::Utc::now() >= session.expires_at {
                fail_all(
                    pending_calls(&state, read, keep_arguments, &context),
                    "auth_denied",
                );
                return error(
                    StatusCode::FORBIDDEN,
                    "MCP OAuth session has expired",
                    "mcp_session_expired",
                );
            }
            if server
                .required_scopes
                .iter()
                .any(|scope| !session.scopes.contains(scope))
            {
                fail_all(
                    pending_calls(&state, read, keep_arguments, &context),
                    "auth_denied",
                );
                return error(
                    StatusCode::FORBIDDEN,
                    "MCP OAuth session does not cover the server's required scopes",
                    "mcp_scope_denied",
                );
            }
            McpUpstreamAuth::Bearer(&session.access_token)
        }
        McpAuthKind::Bearer => {
            let Some(credential) = server.credential.as_deref() else {
                return missing_credential(&server.slug);
            };
            McpUpstreamAuth::Bearer(credential)
        }
        McpAuthKind::Header => {
            let Some(credential) = server.credential.as_deref() else {
                return missing_credential(&server.slug);
            };
            // the shape constraint in migration 0069 makes the header name
            // non-null for this kind; a row written around it is a
            // misconfiguration rather than an open server
            let Some(name) = server.auth_header_name.as_deref() else {
                return error(
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "MCP server '{}' authenticates with a header but names none",
                        server.slug
                    ),
                    "mcp_credential_unavailable",
                );
            };
            McpUpstreamAuth::Header {
                name,
                value: credential,
            }
        }
        McpAuthKind::None => McpUpstreamAuth::None,
        // a kind this build does not know. presenting nothing would quietly
        // downgrade a server the control plane believes is authenticated
        McpAuthKind::Unknown => {
            return error(
                StatusCode::BAD_GATEWAY,
                format!(
                    "MCP server '{}' uses an authentication kind this gateway does not support; \
                     upgrade the gateway to match the control plane",
                    server.slug
                ),
                "mcp_auth_kind_unsupported",
            )
        }
    };

    // the tool groups an organization enabled are an access boundary: a call
    // naming a tool outside them is refused before anything is forwarded, and
    // recorded in the MCP log like any other denial (#1403)
    let mut list_filter = None;
    if method == Method::GET {
        // a stream the caller opens can carry the reply to a `tools/list`
        // posted elsewhere: a legacy `sse` server answers every POST with 202
        // and delivers the reply here, and a streamable-HTTP stream can be
        // replayed (#3297)
        list_filter = crate::mcp_policy::stream_filter(&rules).map(Arc::new);
    }
    if method == Method::POST {
        let decision = crate::mcp_policy::evaluate_read(&rules, &read);
        list_filter = decision.list_filter.map(Arc::new);
        state.mcp_events.count_dropped(decision.unlogged);
        let (refused, message) = match decision.verdict {
            Verdict::Allow => (None, String::new()),
            Verdict::Denied(refused) => {
                let message = refused
                    .first()
                    .map(|call| call.message(&server.slug))
                    .unwrap_or_default();
                (Some(refused), message)
            }
            Verdict::Ambiguous(refused) => (Some(refused), ambiguous_message(&server.slug)),
            Verdict::Unreadable => {
                return error(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "the request to MCP server '{}' could not be read as JSON-RPC, so the \
                         organization's tool groups cannot be applied to it",
                        server.slug
                    ),
                    "mcp_request_unreadable",
                );
            }
        };
        if let Some(refused) = refused {
            for call in refused {
                if let Some(call) =
                    PendingCall::from_request(&state.mcp_events, call.request, &context)
                {
                    call.fail("auth_denied");
                }
            }
            return error(StatusCode::FORBIDDEN, message, "mcp_tool_denied");
        }
    }

    // the parsed body can be many times the size of the bytes, so what the
    // log needs is taken out of it and the rest is released before the wait on
    // the upstream
    let mut calls = pending_calls(&state, read, keep_arguments, &context);

    let Some(url) = downstream_url(&server.url, tail.as_deref(), query) else {
        return error(
            StatusCode::BAD_GATEWAY,
            "MCP server URL is invalid",
            "mcp_server_invalid",
        );
    };
    if let Some(reason) = unread {
        state.metrics.observe_mcp_unread(reason);
        calls.extend(PendingCall::unread(&state.mcp_events, &context));
    }
    let mut headers = end_to_end_headers(headers);
    if list_filter.is_some() {
        // a compressed reply cannot be narrowed
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("identity"),
        );
    }
    match state
        .forwarder
        .forward_mcp(
            method,
            &url,
            headers,
            body,
            auth,
            transport_overrides(server, snapshot.mcp_org_settings.get(&server.org_id)),
        )
        .await
    {
        Ok(response) => {
            // an upstream that says the user's token was valid but too narrow
            // gets the step-up answer, not a bare 403 (#1417)
            if matches!(server.auth_kind, McpAuthKind::Oauth) {
                if let Some((step_up, scopes)) = insufficient_scope(&response, &server.slug) {
                    fail_all_insufficient_scope(calls, scopes);
                    return step_up;
                }
            }
            upstream_response(response, calls, list_filter, is_post).await
        }
        Err(upstream) => {
            let detail = upstream.to_string();
            // the caller gets a fixed sentence: the forwarder's text quotes the
            // server url, which is where an operator puts the token for a server
            // that wants it in the query string, and masking userinfo and
            // credential-named parameters cannot catch a token in the path or a
            // parameter with an ordinary name (#2919). the detail is for the
            // operator, so it goes to the log, masked, beside the request id
            // that ties it to the answer
            tracing::warn!(
                server = %server.slug,
                request_id = request_id.as_str(),
                error = %rolter_core::redact::redact_urls_in_text(&detail),
                "mcp upstream request failed"
            );
            for call in calls {
                call.fail_upstream(&detail);
            }
            error(
                StatusCode::BAD_GATEWAY,
                MCP_UPSTREAM_FAILED,
                "mcp_upstream_error",
            )
        }
    }
}

/// Whether the request body is encoded: any `Content-Encoding` that is not
/// `identity`, in any of the header's lines or its comma-separated values.
fn is_encoded(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .any(|value| {
            value.to_str().map_or(true, |value| {
                value
                    .split(',')
                    .any(|coding| !coding.trim().eq_ignore_ascii_case("identity"))
            })
        })
}

/// The step-up answer for an upstream `403` whose challenge says
/// `insufficient_scope` (#1417), or `None` for any other response.
///
/// The gateway cannot ask the user for consent: that is an interactive flow the
/// control plane owns. What it can do is stop the failure being opaque and keep
/// it from widening anything on its own. The caller gets `403` with the code
/// `mcp_insufficient_scope`, the scopes the upstream named in the body, and a
/// standard `WWW-Authenticate` challenge, rebuilt rather than relayed because
/// the upstream's would point `resource_metadata` at the upstream itself. The
/// key's owner then re-consents with those scopes, through the dashboard or
/// `POST /api/v1/mcp-servers/{id}/oauth/authorize` with `step_up_scopes`; until
/// they do, every such call fails the same way and is logged as `auth_denied`,
/// with the error `insufficient scope` and the scopes the challenge named
/// (#3375), which are returned beside the answer.
fn insufficient_scope(response: &reqwest::Response, slug: &str) -> Option<(Response, Vec<String>)> {
    use rolter_core::mcp_challenge::{challenge_scopes, is_insufficient_scope};

    if response.status() != reqwest::StatusCode::FORBIDDEN {
        return None;
    }
    let challenge = response
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    if !is_insufficient_scope(challenge) {
        return None;
    }
    // validated, deduplicated and bounded: the upstream's words never reach the
    // caller unfiltered
    let scopes = challenge_scopes(challenge);
    let mut body = crate::error::ApiError::new(
        StatusCode::FORBIDDEN,
        format!(
            "MCP server '{slug}' refused the call for want of an OAuth scope the user's session \
             does not hold; the key's owner must authorize it again for the scopes in \
             required_scopes"
        ),
    )
    .with_code("mcp_insufficient_scope")
    .body();
    body["error"]["required_scopes"] = serde_json::json!(scopes);
    let mut rebuilt = String::from("Bearer error=\"insufficient_scope\"");
    if !scopes.is_empty() {
        rebuilt.push_str(&format!(", scope=\"{}\"", scopes.join(" ")));
    }
    let mut answer = (StatusCode::FORBIDDEN, axum::Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(&rebuilt) {
        answer.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    }
    Some((answer, scopes))
}

/// A server configured to present a credential that did not reach the snapshot.
///
/// The usual cause is `ROLTER_KEK` being unset or rotated on the control plane,
/// so the sealed value could not be unsealed. Refusing with a message that says
/// so beats presenting nothing and letting the upstream return its own 401,
/// which would look like the operator's token being wrong.
fn missing_credential(slug: &str) -> Response {
    error(
        StatusCode::BAD_GATEWAY,
        format!(
            "MCP server '{slug}' is configured with a stored credential that is not available to \
             the gateway; check that ROLTER_KEK is set on the control plane"
        ),
        "mcp_credential_unavailable",
    )
}

/// The transport values for one call, in the form the forwarder takes.
///
/// Highest precedence first: the server's own override, the organization's saved
/// MCP settings (#1404), then the deployment-level default, which is what a
/// `None` field means to the forwarder. An organization that never saved its
/// settings contributes nothing, so it keeps the deployment default.
fn transport_overrides(
    server: &McpServerConfig,
    org: Option<&McpOrgSettingsConfig>,
) -> McpTransportOverrides {
    let millis = std::time::Duration::from_millis;
    McpTransportOverrides {
        connect_timeout: server
            .connect_timeout_ms
            .or(org.map(|org| org.connect_timeout_ms))
            .map(millis),
        request_timeout: server
            .request_timeout_ms
            .or(org.map(|org| org.request_timeout_ms))
            .map(millis),
        max_retries: server
            .max_retries
            .or(org.map(|org| org.max_retries))
            .unwrap_or(0),
    }
}

fn downstream_url(base: &str, tail: Option<&str>, query: Option<&str>) -> Option<String> {
    let mut url = reqwest::Url::parse(base).ok()?;
    if let Some(path) = tail.filter(|path| !path.is_empty()) {
        let joined = format!(
            "{}/{}",
            url.path().trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        url.set_path(&joined);
    }
    if let Some(query) = query.filter(|query| !query.is_empty()) {
        let merged = url.query().map_or_else(
            || query.to_string(),
            |existing| format!("{existing}&{query}"),
        );
        url.set_query(Some(&merged));
    }
    Some(url.into())
}

/// A log row for every `tools/call` the body holds, taking them out of `read`.
/// Empty when the log is off or the body held none. A call sent as a
/// notification is recorded too: it has no reply, so it ends with the HTTP
/// layer's answer (#3466).
fn pending_calls(
    state: &AppState,
    read: crate::mcp_policy::Read,
    keep_arguments: bool,
    context: &CallContext<'_>,
) -> Vec<PendingCall> {
    if !state.mcp_events.enabled() {
        return Vec::new();
    }
    let calls = read.into_calls(keep_arguments, crate::mcp_log::MAX_CALLS_PER_REQUEST);
    state.mcp_events.count_dropped(calls.dropped);
    calls
        .kept
        .into_iter()
        .filter_map(|request| PendingCall::from_request(&state.mcp_events, request, context))
        .collect()
}

fn fail_all(calls: Vec<PendingCall>, status: &'static str) {
    for call in calls {
        call.fail(status);
    }
}

fn fail_all_insufficient_scope(calls: Vec<PendingCall>, scopes: Vec<String>) {
    for call in calls {
        call.fail_insufficient_scope(scopes.clone());
    }
}

// spec-ref: http-semantics
fn end_to_end_headers(mut headers: HeaderMap) -> HeaderMap {
    rolter_proxy::hop_by_hop::strip(&mut headers);
    for name in [
        "authorization",
        "api-key",
        "content-length",
        "cookie",
        "host",
        "x-api-key",
    ] {
        headers.remove(name);
    }
    headers
}

async fn upstream_response(
    response: reqwest::Response,
    calls: Vec<PendingCall>,
    list_filter: Option<Arc<ListFilter>>,
    method_is_post: bool,
) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = end_to_end_headers(response.headers().clone());
    let is_sse = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            // a media type is case-insensitive
            value
                .trim_start()
                .get(.."text/event-stream".len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("text/event-stream"))
        });
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    let encoded = headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| value != "identity");
    let mut builder = Response::builder().status(status);
    if let Some(response_headers) = builder.headers_mut() {
        *response_headers = headers;
    }
    // a reply that lists tools is narrowed to what the caller may call
    // (#3297). `202`, `204` and an empty body carry nothing to narrow: a legacy
    // `sse` server answers the POST that way and sends the list on its stream
    let carries_body = status.is_success() && !matches!(status.as_u16(), 202 | 204);
    if let Some(filter) = list_filter.filter(|_| carries_body) {
        // a reply that is rewritten cannot be read for the calls it answers (a
        // batch can hold a `tools/list` beside `tools/call`s)
        for call in calls {
            call.finish_unobserved(status.as_u16());
        }
        if encoded {
            return unfilterable_list();
        }
        if is_sse {
            let stream = FilteredSse::new(Box::pin(response.bytes_stream()), filter);
            return builder
                .body(Body::from_stream(stream))
                .unwrap_or_else(|_| unfilterable_list());
        }
        // a stream the caller opened that is not an event stream cannot be shown
        // to hold no tool list, and only a POST reply is buffered and narrowed
        if !method_is_post {
            return unfilterable_list();
        }
        {
            let Some(bytes) = read_limited(response, MAX_LIST_REPLY).await else {
                return unfilterable_list();
            };
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return builder
                    .body(Body::from(bytes))
                    .unwrap_or_else(|_| unfilterable_list());
            }
            let Some(filtered) = filter.filter_json(&bytes) else {
                return unfilterable_list();
            };
            return builder
                .body(Body::from(filtered))
                .unwrap_or_else(|_| unfilterable_list());
        }
    }
    let body = if calls.is_empty() {
        Body::from_stream(response.bytes_stream())
    } else {
        // recorded when the body has been read, without touching a byte of it
        Body::from_stream(crate::mcp_log::observe(
            calls,
            status.as_u16(),
            is_sse,
            response.bytes_stream(),
        ))
    };
    builder.body(body).unwrap_or_else(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build MCP response",
            "mcp_response_error",
        )
    })
}

/// The reply to a `tools/list` that could not be narrowed. Forwarding it as it
/// came would show the caller tools the policy refuses, so it is not forwarded.
fn unfilterable_list() -> Response {
    error(
        StatusCode::BAD_GATEWAY,
        "the MCP server's tool list could not be filtered to the tools this key may call",
        "mcp_upstream_error",
    )
}

/// The body of `response`, or `None` once it is longer than `limit` or breaks off.
async fn read_limited(mut response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > limit {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

fn error(status: StatusCode, message: impl Into<String>, code: &'static str) -> Response {
    crate::error::ApiError::new(status, message)
        .with_code(code)
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{GatewayConfig, McpOAuthSessionConfig, McpServerConfig, VirtualKeyRecord};

    fn config(session_user: &str, session_scopes: &[&str]) -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.db_virtual_keys.push(VirtualKeyRecord {
            key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), "sk-mcp-user"),
            id: "key-1".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "project-1".to_string(),
            user_id: "user-1".to_string(),
            ..Default::default()
        });
        config.mcp_servers.push(McpServerConfig {
            id: "server-1".to_string(),
            org_id: "org-1".to_string(),
            slug: "github".to_string(),
            url: "http://127.0.0.1:9".to_string(),
            transport: "streamable_http".to_string(),
            required_scopes: vec!["tools:execute".to_string()],
            auth_kind: McpAuthKind::Oauth,
            ..Default::default()
        });
        config.mcp_oauth_sessions.push(McpOAuthSessionConfig {
            id: "session-1".to_string(),
            server_id: "server-1".to_string(),
            user_id: session_user.to_string(),
            scopes: session_scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            access_token: "downstream-secret".to_string(),
        });
        config
    }

    fn bearer() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer sk-mcp-user"),
        );
        headers
    }

    #[tokio::test]
    async fn proxy_denies_session_owned_by_another_user() {
        let response = proxy(
            AppState::new(&config("user-2", &["tools:execute"])),
            "github".to_string(),
            None,
            None,
            Method::POST,
            bearer(),
            RequestId::default(),
            Bytes::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn proxy_denies_missing_required_scope_before_connecting() {
        let response = proxy(
            AppState::new(&config("user-1", &["tools:read"])),
            "github".to_string(),
            None,
            None,
            Method::POST,
            bearer(),
            RequestId::default(),
            Bytes::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn proxy_denies_session_that_expired_after_snapshot_load() {
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_oauth_sessions[0].expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        let response = proxy(
            AppState::new(&config),
            "github".to_string(),
            None,
            None,
            Method::POST,
            bearer(),
            RequestId::default(),
            Bytes::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn the_relay_drops_nominated_and_trailer_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("x-internal"));
        headers.insert("x-internal", HeaderValue::from_static("secret"));
        headers.insert(header::TRAILER, HeaderValue::from_static("x-checksum"));
        headers.insert("x-kept", HeaderValue::from_static("kept"));
        let headers = end_to_end_headers(headers);
        assert!(headers.get(header::CONNECTION).is_none());
        assert!(headers.get("x-internal").is_none());
        assert!(headers.get(header::TRAILER).is_none());
        assert_eq!(headers.get("x-kept").unwrap(), "kept");
    }

    #[tokio::test]
    async fn proxy_replaces_client_auth_and_preserves_path_query_and_body() {
        use axum::routing::any;
        use axum::{Json, Router};

        async fn capture(
            OriginalUri(uri): OriginalUri,
            headers: HeaderMap,
            body: Bytes,
        ) -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "authorization": headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                "uri": uri.to_string(),
                "body": String::from_utf8_lossy(&body),
            }))
        }

        let app = Router::new().route("/{*path}", any(capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        let response = proxy(
            AppState::new(&config),
            "github".to_string(),
            Some("messages/1".to_string()),
            Some("v=2"),
            Method::POST,
            bearer(),
            RequestId::default(),
            Bytes::from_static(br#"{"jsonrpc":"2.0"}"#),
        )
        .await;
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let captured: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            captured,
            serde_json::json!({
                "authorization": "Bearer downstream-secret",
                "uri": "/rpc/messages/1?v=2",
                "body": "{\"jsonrpc\":\"2.0\"}",
            })
        );
    }

    /// #2919: the forwarder's error quotes the server url, and an operator puts
    /// the token for a server in that url. The caller gets one fixed sentence,
    /// whichever part of the address carried the secret.
    #[tokio::test]
    async fn an_unreachable_server_never_has_its_url_echoed_to_the_caller() {
        let secret = format!("tok-{}", uuid::Uuid::new_v4().simple());
        let mut config = config("user-1", &["tools:execute"]);
        // a port nothing listens on, so the connection is refused and the error
        // names the whole address
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        config.mcp_servers[0].auth_kind = McpAuthKind::None;
        config.mcp_servers[0].url = format!(
            "http://svc:{secret}@{closed}/{secret}/mcp?access_token={secret}&team={secret}"
        );
        let response = proxy(
            AppState::new(&config),
            "github".to_string(),
            None,
            None,
            Method::POST,
            bearer(),
            RequestId::default(),
            Bytes::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains(&secret),
            "the server url reached the caller: {text}"
        );
        assert!(
            !text.contains(&closed.to_string()),
            "the host reached the caller: {text}"
        );
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "mcp_upstream_error");
        assert_eq!(body["error"]["message"], MCP_UPSTREAM_FAILED);
    }

    fn with_group(tools: &[&str]) -> GatewayConfig {
        let mut config = config("user-1", &["tools:execute"]);
        config
            .mcp_tool_groups
            .push(rolter_core::McpToolGroupConfig {
                id: "group-1".to_string(),
                org_id: "org-1".to_string(),
                slug: "reviewers".to_string(),
                tools: tools
                    .iter()
                    .map(|tool| rolter_core::McpToolRefConfig {
                        server_id: "server-1".to_string(),
                        tool: (*tool).to_string(),
                    })
                    .collect(),
                ..Default::default()
            });
        config
    }

    fn call_body(tool: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
        ))
    }

    async fn post(state: AppState, body: Bytes) -> Response {
        post_with(state, bearer(), body).await
    }

    async fn post_with(state: AppState, headers: HeaderMap, body: Bytes) -> Response {
        proxy(
            state,
            "github".to_string(),
            None,
            None,
            Method::POST,
            headers,
            RequestId::default(),
            body,
        )
        .await
    }

    async fn code_of(response: Response) -> (StatusCode, String) {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        (
            status,
            body["error"]["code"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        )
    }

    /// #3399: a call a group governs, the policy allows and a decoder reads
    /// through differently cased keys is forwarded and recorded, from the one
    /// parse the policy made.
    #[tokio::test]
    async fn a_governed_allowed_call_with_other_key_cases_is_recorded() {
        let mut config = with_group(&["get_issue"]);
        config.mcp_servers[0].url = serving(200, None).await;
        let mut state = AppState::new(&config);
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
        state.mcp_events = sink;
        let body = Bytes::from(
            r#"{"jsonrpc":"2.0","ID":1,"Method":"tools/call","Params":{"Name":"get_issue","Arguments":{}}}"#,
        );
        let response = post(state, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        // the row is written when the reply has been read
        let _ = axum::body::to_bytes(response.into_body(), 4096).await;
        let event = events.try_recv().expect("the allowed call is recorded");
        assert_eq!(event.tool, "get_issue");
        assert_ne!(event.status, "auth_denied");
    }

    /// an upstream that answers every request with `body`
    async fn replying(status: u16, body: &'static str) -> String {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route(
            "/{*path}",
            any(move || async move {
                axum::http::Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/rpc")
    }

    /// the rows recorded for `body` sent to an upstream answering with
    /// `status` and `reply`, in the order they were written
    async fn recorded(
        tools: &[&str],
        status: u16,
        reply: &'static str,
        body: &str,
    ) -> (StatusCode, Vec<crate::mcp_log::McpEvent>) {
        recorded_on("streamable_http", tools, status, reply, body).await
    }

    /// [`recorded`] for a server on `transport`
    async fn recorded_on(
        transport: &str,
        tools: &[&str],
        status: u16,
        reply: &'static str,
        body: &str,
    ) -> (StatusCode, Vec<crate::mcp_log::McpEvent>) {
        let mut config = with_group(tools);
        config.mcp_servers[0].transport = transport.to_string();
        config.mcp_servers[0].url = replying(status, reply).await;
        let mut state = AppState::new(&config);
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
        state.mcp_events = sink;
        let response = post(state, Bytes::from(body.to_string())).await;
        let status = response.status();
        // the rows are written when the reply has been read
        let _ = axum::body::to_bytes(response.into_body(), 4096).await;
        let mut rows = Vec::new();
        while let Ok(event) = events.try_recv() {
            rows.push(event);
        }
        (status, rows)
    }

    /// #3466: every allowed call in a batch has a row of its own, matched to its
    /// reply by `id` whatever order the replies come in.
    #[tokio::test]
    async fn each_allowed_call_in_a_batch_is_recorded_with_its_own_outcome() {
        let batch = format!(
            "[{},{}]",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue"}}"#,
            r#"{"jsonrpc":"2.0","id":"b","method":"tools/call","params":{"name":"list_issues"}}"#,
        );
        let reply = r#"[{"jsonrpc":"2.0","id":"b","result":{"content":[]}},{"jsonrpc":"2.0","id":1,"result":{"isError":true}}]"#;
        let (status, rows) = recorded(&["get_issue", "list_issues"], 200, reply, &batch).await;
        assert_eq!(status, StatusCode::OK);
        let seen: Vec<_> = rows
            .iter()
            .map(|row| (row.tool.as_str(), row.status, row.notification))
            .collect();
        assert_eq!(
            seen,
            [
                ("get_issue", "error", false),
                ("list_issues", "success", false)
            ]
        );
        assert_ne!(rows[0].event_id, rows[1].event_id);
    }

    /// the batch of `n` allowed calls, ids 1..=n
    fn batch_of(n: usize, tool: &str) -> String {
        let calls: Vec<String> = (1..=n)
            .map(|id| {
                format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}"}}}}"#
                )
            })
            .collect();
        format!("[{}]", calls.join(","))
    }

    /// a request is bounded only by the body limit, so the rows made from it
    /// are bounded by `MAX_CALLS_PER_REQUEST`: the calls past it are forwarded
    /// and counted as dropped, never held as pending rows through the wait
    #[tokio::test]
    async fn a_batch_past_the_cap_records_the_cap_and_counts_the_rest() {
        let cap = crate::mcp_log::MAX_CALLS_PER_REQUEST;
        let extra = 10;
        let reply = r#"[{"jsonrpc":"2.0","id":1,"result":{}}]"#;
        let mut config = with_group(&["get_issue"]);
        config.mcp_servers[0].url = replying(200, reply).await;
        let mut state = AppState::new(&config);
        let metrics = std::sync::Arc::new(crate::metrics::Metrics::default());
        // room for every row, so a dropped count can only be the cap's
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing_with(metrics.clone(), cap * 2);
        state.mcp_events = sink;
        let response = post(state, Bytes::from(batch_of(cap + extra, "get_issue"))).await;
        assert_eq!(response.status(), StatusCode::OK);
        let _ = axum::body::to_bytes(response.into_body(), 4096).await;
        let mut rows = 0;
        while events.try_recv().is_ok() {
            rows += 1;
        }
        assert_eq!(rows, cap);
        assert_eq!(
            metrics
                .mcp_events_dropped_total
                .load(std::sync::atomic::Ordering::Relaxed),
            extra as u64
        );
    }

    /// the same bound holds for the refusals a batch of forbidden calls logs
    #[tokio::test]
    async fn a_refused_batch_past_the_cap_logs_the_cap_and_counts_the_rest() {
        let cap = crate::mcp_log::MAX_CALLS_PER_REQUEST;
        let mut state = AppState::new(&with_group(&["get_issue"]));
        let metrics = std::sync::Arc::new(crate::metrics::Metrics::default());
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing_with(metrics.clone(), cap * 2);
        state.mcp_events = sink;
        let (status, _) = code_of(post(state, Bytes::from(batch_of(cap + 5, "drop"))).await).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let mut rows = 0;
        while events.try_recv().is_ok() {
            rows += 1;
        }
        assert_eq!(rows, cap);
        assert_eq!(
            metrics
                .mcp_events_dropped_total
                .load(std::sync::atomic::Ordering::Relaxed),
            5
        );
    }

    /// a batch holding a `tools/list` has its reply rewritten for the caller, so
    /// its calls cannot be read from the reply: they are rows with the HTTP
    /// status and no result
    #[tokio::test]
    async fn the_calls_of_a_batch_whose_reply_is_narrowed_are_recorded_by_status() {
        let batch = format!(
            "[{},{}]",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_issue"}}"#,
        );
        let reply = r#"[{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"get_issue"}]}},{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"x"}]}}]"#;
        let (status, rows) = recorded(&["get_issue"], 200, reply, &batch).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].tool.as_str(), rows[0].status),
            ("get_issue", "success")
        );
        assert_eq!(rows[0].result, "");
    }

    /// `Content-Encoding` is read from every header line, and a value that is
    /// empty or not text is not `identity`
    #[test]
    fn only_identity_codings_are_unencoded() {
        let with = |values: &[&[u8]]| {
            let mut headers = HeaderMap::new();
            for value in values {
                headers.append(
                    header::CONTENT_ENCODING,
                    HeaderValue::from_bytes(value).unwrap(),
                );
            }
            is_encoded(&headers)
        };
        assert!(!is_encoded(&HeaderMap::new()));
        assert!(!with(&[b"identity"]));
        assert!(!with(&[b" Identity , identity"]));
        assert!(with(&[b"identity", b"gzip"]), "a second line");
        assert!(with(&[b""]), "an empty value");
        assert!(with(&[b"\xff\xfe"]), "bytes that are not text");
        assert!(with(&[b"identity,"]), "an empty coding in the list");
    }

    /// a server no group governs forwards a compressed body as it came: nothing
    /// is judged, so nothing is refused
    #[tokio::test]
    async fn an_ungoverned_server_passes_a_compressed_body_through() {
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = replying(200, "{}").await;
        let mut headers = bearer();
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        let response = post_with(
            AppState::new(&config),
            headers,
            Bytes::from_static(b"\x1f\x8b\x08\x00 compressed"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// #3528: a body forwarded to an ungoverned server without being read is
    /// counted by why, and with the MCP log on it is one `unread` row with no
    /// tool; a failure the HTTP layer reports stays that failure
    #[tokio::test]
    async fn a_body_forwarded_unread_is_counted_and_recorded_as_unread() {
        async fn send(
            upstream: u16,
            encoding: Option<&'static str>,
            body: &'static [u8],
            logging: bool,
        ) -> (String, Vec<crate::mcp_log::McpEvent>) {
            let mut config = config("user-1", &["tools:execute"]);
            config.mcp_servers[0].url = replying(upstream, "{}").await;
            let mut state = AppState::new(&config);
            let metrics = state.metrics.clone();
            let (sink, mut events) =
                crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
            if logging {
                state.mcp_events = sink;
            }
            let mut headers = bearer();
            if let Some(encoding) = encoding {
                headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
            }
            let response = post_with(state, headers, Bytes::from_static(body)).await;
            assert_eq!(response.status().as_u16(), upstream);
            let _ = axum::body::to_bytes(response.into_body(), 4096).await;
            let mut rows = Vec::new();
            while let Ok(event) = events.try_recv() {
                rows.push(event);
            }
            (metrics.render(), rows)
        }
        let counted = |text: &str, reason: &str| {
            text.lines()
                .find(|line| {
                    line.starts_with(&format!(
                        "rolter_mcp_unread_bodies_total{{reason=\"{reason}\"}} "
                    ))
                })
                .and_then(|line| line.rsplit(' ').next())
                .map(str::to_string)
        };
        let gzip: &[u8] = b"\x1f\x8b\x08\x00 compressed tools/call";
        let (text, rows) = send(200, Some("gzip"), gzip, true).await;
        assert_eq!(counted(&text, "encoded").as_deref(), Some("1"), "{text}");
        assert_eq!(
            counted(&text, "unparseable").as_deref(),
            Some("0"),
            "{text}"
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].tool.as_str(), rows[0].status, rows[0].outcome),
            ("", "success", "unread")
        );
        assert!(!rows[0].notification);

        let (text, rows) = send(200, None, b"{ nope tools/call", true).await;
        assert_eq!(
            counted(&text, "unparseable").as_deref(),
            Some("1"),
            "{text}"
        );
        assert_eq!((rows[0].status, rows[0].outcome), ("success", "unread"));

        // with the log off an encoded body is still counted, and nothing reads
        // one that is not JSON
        let (text, rows) = send(200, Some("gzip"), gzip, false).await;
        assert_eq!(counted(&text, "encoded").as_deref(), Some("1"), "{text}");
        assert!(rows.is_empty());
        let (text, _) = send(200, None, b"{ nope tools/call", false).await;
        assert_eq!(
            counted(&text, "unparseable").as_deref(),
            Some("0"),
            "{text}"
        );

        let (_, rows) = send(502, Some("gzip"), gzip, true).await;
        assert_eq!((rows[0].status, rows[0].outcome), ("transport_error", ""));
        // a body that is read and holds no call is neither counted nor a row
        let (text, rows) = send(
            200,
            None,
            br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
            true,
        )
        .await;
        assert_eq!(
            counted(&text, "unparseable").as_deref(),
            Some("0"),
            "{text}"
        );
        assert!(rows.is_empty());
    }

    /// #3466: one refused call refuses the batch whole. Only the refusal is a
    /// row: the allowed call beside it never reached the server, so there is no
    /// call to record.
    #[tokio::test]
    async fn a_batch_with_one_allowed_and_one_refused_call_records_the_refusal() {
        let batch = format!(
            "[{},{}]",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"drop"}}"#,
        );
        let reply = r#"[{"jsonrpc":"2.0","id":1,"result":{}}]"#;
        let (status, rows) = recorded(&["get_issue"], 200, reply, &batch).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let seen: Vec<_> = rows
            .iter()
            .map(|row| (row.tool.as_str(), row.status))
            .collect();
        assert_eq!(seen, [("drop", "auth_denied")]);
    }

    /// #3466: an allowed notification has no id and so no reply. It is recorded
    /// with the status the HTTP layer gave and the flag that says no reply was
    /// awaited, in a batch as well as alone.
    #[tokio::test]
    async fn an_allowed_notification_is_recorded_as_one_that_awaited_no_reply() {
        let note = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"get_issue"}}"#;
        let (status, rows) = recorded(&["get_issue"], 202, "", note).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].tool.as_str(), rows[0].status, rows[0].notification),
            ("get_issue", "success", true)
        );
        assert_eq!(serde_json::to_value(&rows[0]).unwrap()["notification"], 1);

        // beside a call that has one: each ends as its own kind of call
        let batch = format!(
            "[{note},{}]",
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_issue"}}"#
        );
        let reply = r#"[{"jsonrpc":"2.0","id":7,"result":{}}]"#;
        let (_, rows) = recorded(&["get_issue"], 200, reply, &batch).await;
        let seen: Vec<_> = rows
            .iter()
            .map(|row| (row.status, row.notification))
            .collect();
        assert_eq!(seen, [("success", true), ("success", false)]);
        // and a refused notification is a refusal, not a notification
        let (status, rows) = recorded(
            &["get_issue"],
            202,
            "",
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"drop"}}"#,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            (rows[0].status, rows[0].notification),
            ("auth_denied", false)
        );
    }

    /// #3529: a legacy `sse` server answers the `POST` `202` and sends the reply
    /// on its stream, so the call is `accepted`, not a transport error. A `202`
    /// whose body does carry the reply is judged by it, and a streamable-HTTP
    /// server owes the reply in its answer, so there a bare `202` stays a
    /// failure.
    #[tokio::test]
    async fn a_legacy_sse_call_answered_202_is_accepted_with_its_reply_on_the_stream() {
        let call =
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_issue"}}"#;
        let (status, rows) = recorded_on("sse", &["get_issue"], 202, "Accepted", call).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(
            (rows[0].status, rows[0].outcome, rows[0].notification),
            ("success", "accepted", false)
        );
        let row = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(
            (&row["outcome"], &row["error"]),
            (&serde_json::json!("accepted"), &serde_json::json!(""))
        );

        let rpc_error = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"bad"}}"#;
        let (_, rows) = recorded_on("sse", &["get_issue"], 202, rpc_error, call).await;
        assert_eq!((rows[0].status, rows[0].outcome), ("error", ""));

        let (_, rows) = recorded_on("streamable_http", &["get_issue"], 202, "", call).await;
        assert_eq!((rows[0].status, rows[0].outcome), ("transport_error", ""));
        // every other row leaves the field out, for a table without `051`
        let row = serde_json::to_value(&rows[0]).unwrap();
        assert!(row.get("outcome").is_none(), "{row}");
    }

    /// #1403: a tool outside the organization's groups is refused before
    /// anything is forwarded, and the refusal is a row in the MCP log.
    #[tokio::test]
    async fn a_tool_outside_the_tool_groups_is_refused_and_logged() {
        let mut state = AppState::new(&with_group(&["get_issue"]));
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
        state.mcp_events = sink;
        // the default server url is a closed port: a 403 proves nothing was dialled
        let (status, code) = code_of(post(state, call_body("delete_repo")).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "mcp_tool_denied")
        );
        let event = events.try_recv().expect("the refusal is recorded");
        assert_eq!(
            (event.tool.as_str(), event.status, event.server.as_str()),
            ("delete_repo", "auth_denied", "github")
        );
        assert_eq!(
            (event.org_id.as_str(), event.user_id.as_str()),
            ("org-1", "user-1")
        );
        assert_eq!(event.error, "authentication denied");
    }

    #[tokio::test]
    async fn a_batch_holding_one_refused_call_is_refused_whole() {
        let mut state = AppState::new(&with_group(&["get_issue"]));
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
        state.mcp_events = sink;
        let body = Bytes::from(format!(
            "[{},{}]",
            String::from_utf8_lossy(&call_body("get_issue")),
            String::from_utf8_lossy(&call_body("drop")),
        ));
        let (status, code) = code_of(post(state, body).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "mcp_tool_denied")
        );
        assert_eq!(events.try_recv().unwrap().tool, "drop");
        assert!(
            events.try_recv().is_err(),
            "the allowed call was not refused"
        );
    }

    #[tokio::test]
    async fn an_unreadable_body_is_refused_while_a_group_governs_the_server() {
        let state = AppState::new(&with_group(&["get_issue"]));
        let (status, code) =
            code_of(post(state, Bytes::from_static(b"{\"method\": tools/call")).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::BAD_REQUEST, "mcp_request_unreadable")
        );
    }

    /// #3298: a repeated key is refused like a forbidden tool, and nothing is
    /// forwarded.
    #[tokio::test]
    async fn a_body_repeating_the_tool_name_is_refused_like_a_forbidden_tool() {
        let mut state = AppState::new(&with_group(&["get_issue"]));
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
        state.mcp_events = sink;
        let body = Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"drop","name":"get_issue"}}"#,
        );
        let (status, code) = code_of(post(state, body).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "mcp_tool_denied")
        );
        // logged like any other refusal (#3381)
        let event = events.try_recv().expect("the refusal is recorded");
        assert_eq!(
            (event.status, event.tool.as_str()),
            ("auth_denied", "get_issue")
        );
    }

    /// #3297 with a legacy `sse` server: the POST is answered `202`, which is
    /// forwarded as it is, and the reply on the GET stream is the one narrowed
    #[tokio::test]
    async fn a_legacy_sse_servers_stream_is_narrowed_and_its_202_passes() {
        use axum::routing::get;
        use axum::Router;

        let app = Router::new().route(
            "/rpc",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    format!("event: message\ndata: {LIST_REPLY}\n\n"),
                )
            })
            .post(|| async { (StatusCode::ACCEPTED, "Accepted") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = with_group(&["get_issue"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        config.mcp_servers[0].transport = "sse".to_string();

        let list = Bytes::from_static(br#"{"jsonrpc":"2.0","id":1.0,"method":"tools/list"}"#);
        let posted = post(AppState::new(&config), list).await;
        assert_eq!(posted.status(), StatusCode::ACCEPTED);
        let bytes = axum::body::to_bytes(posted.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"Accepted");

        let streamed = proxy(
            AppState::new(&config),
            "github".to_string(),
            None,
            None,
            Method::GET,
            bearer(),
            RequestId::default(),
            Bytes::new(),
        )
        .await;
        assert_eq!(streamed.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(streamed.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            text.contains("get_issue") && !text.contains("drop"),
            "{text}"
        );
    }

    async fn get_with(content_type: &'static str) -> (StatusCode, String) {
        use axum::routing::get;
        use axum::Router;

        let app = Router::new().route(
            "/rpc",
            get(move || async move {
                (
                    [(header::CONTENT_TYPE, content_type)],
                    format!("event: message\ndata: {LIST_REPLY}\n\n"),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = with_group(&["get_issue"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        let response = proxy(
            AppState::new(&config),
            "github".to_string(),
            None,
            None,
            Method::GET,
            bearer(),
            RequestId::default(),
            Bytes::new(),
        )
        .await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// a media type is case-insensitive, and a stream that is not an event
    /// stream cannot be shown to hold no tool list
    #[tokio::test]
    async fn a_stream_is_filtered_whatever_the_case_and_refused_when_it_is_not_sse() {
        let (status, text) = get_with("Text/Event-Stream; charset=utf-8").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            text.contains("get_issue") && !text.contains("drop"),
            "{text}"
        );
        let (status, text) = get_with("application/json").await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{text}");
        assert!(!text.contains("drop"));
    }

    const LIST_REPLY: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"get_issue"},{"name":"drop"}],"nextCursor":"p2"}}"#;

    async fn list_through(config: &mut GatewayConfig, sse: bool) -> (StatusCode, String) {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route(
            "/{*path}",
            any(move || async move {
                if sse {
                    (
                        [(header::CONTENT_TYPE, "text/event-stream")],
                        format!("event: message\ndata: {LIST_REPLY}\n\n"),
                    )
                } else {
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        LIST_REPLY.to_string(),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        let body = Bytes::from_static(br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let response = post(AppState::new(config), body).await;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    /// #3297: a client is shown only the tools it may call, in a JSON reply and
    /// in an SSE one.
    #[tokio::test]
    async fn a_tools_list_reply_is_narrowed_to_the_callable_tools() {
        for sse in [false, true] {
            let mut config = with_group(&["get_issue"]);
            let (status, body) = list_through(&mut config, sse).await;
            assert_eq!(status, StatusCode::OK);
            assert!(body.contains("get_issue"), "{body}");
            assert!(!body.contains("drop"), "{body}");
            assert!(body.contains("nextCursor"), "{body}");
        }
    }

    #[tokio::test]
    async fn a_tools_list_reply_from_an_ungoverned_server_is_forwarded_as_is() {
        let mut config = config("user-1", &["tools:execute"]);
        let (_, body) = list_through(&mut config, false).await;
        assert_eq!(body, LIST_REPLY);
    }

    /// #3295: a group assigned to the key's project narrows that key below the
    /// organization-wide groups.
    #[tokio::test]
    async fn a_group_assigned_to_the_keys_project_narrows_the_key() {
        let mut config = with_group(&["get_issue", "list_issues"]);
        config
            .mcp_tool_groups
            .push(rolter_core::McpToolGroupConfig {
                id: "group-2".to_string(),
                org_id: "org-1".to_string(),
                slug: "readers".to_string(),
                tools: vec![rolter_core::McpToolRefConfig {
                    server_id: "server-1".to_string(),
                    tool: "get_issue".to_string(),
                }],
                assignments: vec![rolter_core::McpToolGroupSubjectConfig {
                    kind: "project".to_string(),
                    id: "project-1".to_string(),
                }],
                ..Default::default()
            });
        let (status, code) =
            code_of(post(AppState::new(&config), call_body("list_issues")).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "mcp_tool_denied")
        );
        // the same tool, with the assignment pointed at someone else
        config.mcp_tool_groups[1].assignments[0].id = "project-2".to_string();
        let (status, _) =
            code_of(post(AppState::new(&config), call_body("list_issues")).await).await;
        assert_ne!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_tool_in_a_group_is_forwarded() {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route("/{*path}", any(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = with_group(&["get_issue"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        let response = post(AppState::new(&config), call_body("get_issue")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_server_no_group_names_still_serves_every_tool() {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route("/{*path}", any(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        let response = post(AppState::new(&config), call_body("anything")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn downstream_url_preserves_tail_and_query() {
        assert_eq!(
            downstream_url("https://mcp.example/rpc/", Some("messages/1"), Some("v=2")),
            Some("https://mcp.example/rpc/messages/1?v=2".to_string())
        );
    }

    #[test]
    fn transport_values_resolve_server_then_organization_then_deployment() {
        let org = McpOrgSettingsConfig {
            org_id: "org-1".into(),
            connect_timeout_ms: 1000,
            request_timeout_ms: 2000,
            max_retries: 3,
            ..Default::default()
        };
        let none = McpServerConfig::default();
        let overrides = transport_overrides(&none, Some(&org));
        assert_eq!(
            overrides.connect_timeout,
            Some(std::time::Duration::from_millis(1000))
        );
        assert_eq!(
            overrides.request_timeout,
            Some(std::time::Duration::from_millis(2000))
        );
        assert_eq!(overrides.max_retries, 3);

        let server = McpServerConfig {
            connect_timeout_ms: Some(10),
            request_timeout_ms: Some(20),
            max_retries: Some(0),
            ..Default::default()
        };
        let overrides = transport_overrides(&server, Some(&org));
        assert_eq!(
            overrides.connect_timeout,
            Some(std::time::Duration::from_millis(10))
        );
        assert_eq!(
            overrides.request_timeout,
            Some(std::time::Duration::from_millis(20))
        );
        assert_eq!(
            overrides.max_retries, 0,
            "an explicit 0 beats the organization"
        );

        // nothing saved anywhere: the forwarder falls back to the deployment
        let overrides = transport_overrides(&none, None);
        assert_eq!(
            (overrides.connect_timeout, overrides.request_timeout),
            (None, None)
        );
        assert_eq!(overrides.max_retries, 0);
    }

    #[tokio::test]
    async fn the_organization_request_timeout_applies_to_a_server_without_its_own() {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route(
            "/{*path}",
            any(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                "late"
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        config.mcp_settings.push(McpOrgSettingsConfig {
            org_id: "org-1".into(),
            connect_timeout_ms: 1000,
            request_timeout_ms: 150,
            max_retries: 0,
            ..Default::default()
        });
        let started = std::time::Instant::now();
        let response = post(AppState::new(&config), call_body("anything")).await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
    }

    #[tokio::test]
    async fn an_undeclared_tool_is_refused_and_so_is_an_unreadable_body_failing_open() {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route("/{*path}", any(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let strict = |fail_open| McpOrgSettingsConfig {
            org_id: "org-1".into(),
            connect_timeout_ms: 5000,
            request_timeout_ms: 30000,
            max_retries: 0,
            fail_open,
            allow_unlisted_tools: false,
        };
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = format!("http://{addr}/rpc");
        config.mcp_servers[0].tools = vec!["get_issue".into()];
        config.mcp_settings = vec![strict(false)];

        let (status, code) = code_of(post(AppState::new(&config), call_body("drop")).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "mcp_tool_denied")
        );
        let allowed = post(AppState::new(&config), call_body("get_issue")).await;
        assert_eq!(allowed.status(), StatusCode::OK);

        // what the relay cannot read it does not forward, and the deprecated
        // `fail_open` no longer changes that (#3527)
        for fail_open in [false, true] {
            config.mcp_settings = vec![strict(fail_open)];
            let (status, code) =
                code_of(post(AppState::new(&config), Bytes::from_static(b"{ nope")).await).await;
            assert_eq!(
                (status, code.as_str()),
                (StatusCode::BAD_REQUEST, "mcp_request_unreadable"),
                "{fail_open}"
            );
        }
    }

    /// An upstream that answers `ok` and keeps every body it receives.
    async fn recording() -> (String, Arc<parking_lot::Mutex<Vec<Bytes>>>) {
        use axum::routing::any;
        use axum::Router;

        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let kept = seen.clone();
        let app = Router::new().route(
            "/{*path}",
            any(move |body: Bytes| {
                let kept = kept.clone();
                async move {
                    kept.lock().push(body);
                    "ok"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/rpc"), seen)
    }

    fn failing_open(config: &mut GatewayConfig) {
        config.mcp_settings = vec![McpOrgSettingsConfig {
            org_id: "org-1".into(),
            connect_timeout_ms: 5000,
            request_timeout_ms: 30000,
            max_retries: 0,
            fail_open: true,
            allow_unlisted_tools: true,
        }];
    }

    /// #3527: a UTF-8 byte-order mark in front of a `tools/call` is stripped
    /// and the call judged, in an organization that saved `fail_open`: a
    /// refused tool is refused, and an allowed one reaches the upstream without
    /// the mark, so the upstream receives exactly the bytes that were judged.
    #[tokio::test]
    async fn a_bom_prefixed_call_is_judged_and_forwarded_without_the_mark() {
        let mut config = with_group(&["get_issue"]);
        let (url, seen) = recording().await;
        config.mcp_servers[0].url = url;
        failing_open(&mut config);
        let with_bom = |tool: &str| {
            let mut body = b"\xef\xbb\xbf".to_vec();
            body.extend_from_slice(&call_body(tool));
            Bytes::from(body)
        };

        let (status, code) = code_of(post(AppState::new(&config), with_bom("drop")).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::FORBIDDEN, "mcp_tool_denied")
        );
        assert!(seen.lock().is_empty());

        let allowed = post(AppState::new(&config), with_bom("get_issue")).await;
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(*seen.lock(), vec![call_body("get_issue")]);
    }

    /// #3527: a server no tool group or manifest governs has nothing to judge,
    /// so even with the MCP log on a body that does not parse, and one with a
    /// byte-order mark, are forwarded byte for byte.
    #[tokio::test]
    async fn an_ungoverned_server_gets_every_body_verbatim_with_logging_on() {
        let mut config = config("user-1", &["tools:execute"]);
        let (url, seen) = recording().await;
        config.mcp_servers[0].url = url;
        let mut with_bom = b"\xef\xbb\xbf".to_vec();
        with_bom.extend_from_slice(&call_body("drop"));
        let bodies = [Bytes::from_static(b"{ nope"), Bytes::from(with_bom)];
        for body in &bodies {
            let mut state = AppState::new(&config);
            let (sink, _events) =
                crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
            state.mcp_events = sink;
            assert!(state.mcp_events.enabled());
            let response = post(state, body.clone()).await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        assert_eq!(*seen.lock(), bodies.to_vec());
    }

    /// #3527: a body `serde_json` does not read but a lenient upstream might
    /// (UTF-16, `NaN`) is refused in a `fail_open` organization, and nothing
    /// reaches the upstream.
    #[tokio::test]
    async fn a_body_only_a_lenient_parser_reads_is_refused_failing_open() {
        let mut config = with_group(&["get_issue"]);
        let (url, seen) = recording().await;
        config.mcp_servers[0].url = url;
        failing_open(&mut config);
        let text = String::from_utf8(call_body("drop").to_vec()).unwrap();
        let utf16: Vec<u8> = "\u{feff}"
            .chars()
            .chain(text.chars())
            .collect::<String>()
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let nan = text.replace(r#""arguments":{}"#, r#""arguments":{"x":NaN}"#);
        assert_ne!(nan, text);
        for body in [Bytes::from(utf16), Bytes::from(nan)] {
            let (status, code) = code_of(post(AppState::new(&config), body).await).await;
            assert_eq!(
                (status, code.as_str()),
                (StatusCode::BAD_REQUEST, "mcp_request_unreadable")
            );
        }
        assert!(seen.lock().is_empty());
    }

    /// #3475: a compressed body cannot be judged, and an upstream that honours
    /// `Content-Encoding` would run the tool it names. It is refused whatever
    /// `fail_open` says, and an `identity` coding is no coding at all.
    #[tokio::test]
    async fn a_compressed_call_to_a_governed_server_is_refused_even_when_failing_open() {
        let mut config = with_group(&["get_issue"]);
        config.mcp_servers[0].url = serving(200, None).await;
        config.mcp_settings = vec![McpOrgSettingsConfig {
            org_id: "org-1".into(),
            connect_timeout_ms: 5000,
            request_timeout_ms: 30000,
            max_retries: 0,
            fail_open: true,
            allow_unlisted_tools: false,
        }];
        let encoded = |coding: &'static str| {
            let mut headers = bearer();
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(coding));
            headers
        };
        // stands in for gzip bytes: they are not JSON, which is exactly what
        // fail_open would have forwarded
        let gzip = Bytes::from_static(b"\x1f\x8b\x08\x00 compressed");
        for coding in ["gzip", "br", "deflate", "identity, gzip", "GZIP"] {
            let (status, code) =
                code_of(post_with(AppState::new(&config), encoded(coding), gzip.clone()).await)
                    .await;
            assert_eq!(
                (status, code.as_str()),
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "mcp_request_unreadable"),
                "{coding}"
            );
        }
        // a readable body is refused just the same: the coding is the problem
        let (status, code) =
            code_of(post_with(AppState::new(&config), encoded("gzip"), call_body("drop")).await)
                .await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::UNSUPPORTED_MEDIA_TYPE, "mcp_request_unreadable")
        );
        for coding in ["identity", "Identity"] {
            let allowed = post_with(
                AppState::new(&config),
                encoded(coding),
                call_body("get_issue"),
            )
            .await;
            assert_eq!(allowed.status(), StatusCode::OK, "{coding}");
        }
    }

    async fn serving(status: u16, challenge: Option<&'static str>) -> String {
        use axum::routing::any;
        use axum::Router;

        let app = Router::new().route(
            "/{*path}",
            any(move || async move {
                let mut response = axum::http::Response::builder().status(status);
                if let Some(challenge) = challenge {
                    response = response.header(header::WWW_AUTHENTICATE, challenge);
                }
                response.body(Body::from("upstream said no")).unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/rpc")
    }

    /// #1417: an upstream `insufficient_scope` is the defined step-up answer
    /// for an OAuth server, and a log row, not a bare relayed 403.
    #[tokio::test]
    async fn insufficient_scope_from_an_oauth_server_is_the_step_up_answer() {
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = serving(
            403,
            Some(
                r#"Bearer error="insufficient_scope", scope="repo read:org", resource_metadata="https://upstream.example/prm""#,
            ),
        )
        .await;
        let mut state = AppState::new(&config);
        let (sink, mut events) =
            crate::mcp_log::McpEventSink::capturing(std::sync::Arc::new(Default::default()));
        state.mcp_events = sink;
        let response = post(state, call_body("anything")).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let challenge = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        // rebuilt: the upstream's resource_metadata is not passed on
        assert_eq!(
            challenge,
            r#"Bearer error="insufficient_scope", scope="repo read:org""#
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "mcp_insufficient_scope");
        assert_eq!(
            body["error"]["required_scopes"],
            serde_json::json!(["repo", "read:org"])
        );
        assert!(!body.to_string().contains("upstream said no"));
        let event = events.try_recv().expect("the refusal is logged");
        assert_eq!(
            (event.status, event.tool.as_str()),
            ("auth_denied", "anything")
        );
        // the row says why, and which scopes the upstream asked for (#3375)
        assert_eq!(event.error, "insufficient scope");
        assert_eq!(event.error_scopes, ["repo", "read:org"]);
    }

    #[tokio::test]
    async fn hostile_scope_text_in_the_challenge_never_reaches_the_caller() {
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = serving(
            403,
            Some(r#"Bearer error="insufficient_scope", scope="ok <script> back\slash""#),
        )
        .await;
        let response = post(AppState::new(&config), call_body("anything")).await;
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body["error"]["required_scopes"],
            serde_json::json!(["ok", "<script>"])
        );
    }

    #[tokio::test]
    async fn other_refusals_and_other_auth_kinds_are_relayed_untouched() {
        // a 403 that is not insufficient_scope
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].url = serving(403, Some(r#"Bearer error="invalid_token""#)).await;
        let response = post(AppState::new(&config), call_body("x")).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(&body[..], b"upstream said no");

        // insufficient_scope from a server with a static credential: there is no
        // consent to repeat, so it is the upstream's own answer
        let mut config = config_static_bearer().await;
        config.mcp_servers[0].url = serving(
            403,
            Some(r#"Bearer error="insufficient_scope", scope="repo""#),
        )
        .await;
        let response = post(AppState::new(&config), call_body("x")).await;
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(&body[..], b"upstream said no");
    }

    async fn config_static_bearer() -> GatewayConfig {
        let mut config = config("user-1", &["tools:execute"]);
        config.mcp_servers[0].auth_kind = McpAuthKind::Bearer;
        config.mcp_servers[0].credential = Some("static-token".to_string());
        config
    }
}
