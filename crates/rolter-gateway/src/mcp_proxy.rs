//! Authenticated MCP HTTP proxying.
//!
//! spec-ref: mcp-transport
//!
//! A virtual key authenticates the workload, while its database owner selects
//! the user's OAuth session. Server, owner and scope checks all happen before
//! any downstream connection is attempted.

use axum::body::Body;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;

use rolter_core::{McpAuthKind, McpOrgSettingsConfig, McpServerConfig};
use rolter_proxy::{McpTransportOverrides, McpUpstreamAuth};

use crate::mcp_log::{CallContext, PendingCall};
use crate::mcp_policy::Verdict;
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
    // a `tools/call` is recorded in the MCP tool-call log (#2395). `None` for
    // every other request, and without parsing anything when no ClickHouse is
    // configured, so the common path pays one flag check
    let call = if method == Method::POST {
        PendingCall::parse(
            &state.mcp_events,
            &body,
            &CallContext {
                server: &server.slug,
                transport: &server.transport,
                org_id: &key.org_id,
                team_id: &key.team_id,
                project_id: &key.project_id,
                virtual_key_id: &key.id,
                // the session is looked up by this id below, so on a per-user
                // OAuth server it is the session owner
                user_id: &key.user_id,
                request_id: request_id.as_str(),
                capture: &snapshot.logging.payload_capture,
            },
        )
    } else {
        None
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
                if let Some(call) = call {
                    call.fail("auth_denied");
                }
                return error(
                    StatusCode::FORBIDDEN,
                    "no live MCP OAuth session authorizes this user and server",
                    "mcp_session_unauthorized",
                );
            };
            if chrono::Utc::now() >= session.expires_at {
                if let Some(call) = call {
                    call.fail("auth_denied");
                }
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
                if let Some(call) = call {
                    call.fail("auth_denied");
                }
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
    if method == Method::POST {
        match crate::mcp_policy::enforce(&snapshot, server, &body) {
            Verdict::Allow => {}
            Verdict::Denied(refused) => {
                let context = CallContext {
                    server: &server.slug,
                    transport: &server.transport,
                    org_id: &key.org_id,
                    team_id: &key.team_id,
                    project_id: &key.project_id,
                    virtual_key_id: &key.id,
                    user_id: &key.user_id,
                    request_id: request_id.as_str(),
                    capture: &snapshot.logging.payload_capture,
                };
                let message = refused
                    .first()
                    .map(|call| call.message(&server.slug))
                    .unwrap_or_default();
                for call in refused {
                    if let Some(call) =
                        PendingCall::from_request(&state.mcp_events, call.request, &context)
                    {
                        call.fail("auth_denied");
                    }
                }
                return error(StatusCode::FORBIDDEN, message, "mcp_tool_denied");
            }
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
        }
    }

    let Some(url) = downstream_url(&server.url, tail.as_deref(), query) else {
        return error(
            StatusCode::BAD_GATEWAY,
            "MCP server URL is invalid",
            "mcp_server_invalid",
        );
    };
    match state
        .forwarder
        .forward_mcp(
            method,
            &url,
            end_to_end_headers(headers),
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
                if let Some(step_up) = insufficient_scope(&response, &server.slug) {
                    if let Some(call) = call {
                        call.fail("auth_denied");
                    }
                    return step_up;
                }
            }
            upstream_response(response, call)
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
            if let Some(call) = call {
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
/// they do, every such call fails the same way and is logged as `auth_denied`.
fn insufficient_scope(response: &reqwest::Response, slug: &str) -> Option<Response> {
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
    Some(answer)
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

// spec-ref: http-semantics
fn end_to_end_headers(mut headers: HeaderMap) -> HeaderMap {
    for name in [
        "authorization",
        "api-key",
        "connection",
        "content-length",
        "cookie",
        "host",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "x-api-key",
    ] {
        headers.remove(name);
    }
    headers
}

fn upstream_response(response: reqwest::Response, call: Option<PendingCall>) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = end_to_end_headers(response.headers().clone());
    let is_sse = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    let mut builder = Response::builder().status(status);
    if let Some(response_headers) = builder.headers_mut() {
        *response_headers = headers;
    }
    let body = match call {
        // recorded when the body has been read, without touching a byte of it
        Some(call) => {
            Body::from_stream(call.observe(status.as_u16(), is_sse, response.bytes_stream()))
        }
        None => Body::from_stream(response.bytes_stream()),
    };
    builder.body(body).unwrap_or_else(|_| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build MCP response",
            "mcp_response_error",
        )
    })
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
            access_policy: None,
            key_hash: rolter_auth::hash_key(&config.server.resolve_key_pepper(), "sk-mcp-user"),
            id: "key-1".to_string(),
            org_id: "org-1".to_string(),
            team_id: "team-1".to_string(),
            project_id: "project-1".to_string(),
            user_id: "user-1".to_string(),
            models: Vec::new(),
            providers: Vec::new(),
            disabled: false,
            expires_at: None,
            cache: None,
            business_unit_id: String::new(),
            customer_id: String::new(),
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
            });
        config
    }

    fn call_body(tool: &str) -> Bytes {
        Bytes::from(format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
        ))
    }

    async fn post(state: AppState, body: Bytes) -> Response {
        proxy(
            state,
            "github".to_string(),
            None,
            None,
            Method::POST,
            bearer(),
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
    async fn an_undeclared_tool_is_refused_and_fail_open_forwards_an_unreadable_body() {
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

        // fail closed (the default) refuses what it cannot read; fail open forwards
        let (status, code) =
            code_of(post(AppState::new(&config), Bytes::from_static(b"{ nope")).await).await;
        assert_eq!(
            (status, code.as_str()),
            (StatusCode::BAD_REQUEST, "mcp_request_unreadable")
        );
        config.mcp_settings = vec![strict(true)];
        let forwarded = post(AppState::new(&config), Bytes::from_static(b"{ nope")).await;
        assert_eq!(forwarded.status(), StatusCode::OK);
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
