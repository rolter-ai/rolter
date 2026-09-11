//! Authenticated MCP HTTP proxying.
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

use rolter_core::{McpAuthKind, McpServerConfig};
use rolter_proxy::{McpTransportOverrides, McpUpstreamAuth};

use crate::state::AppState;

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
    body: Bytes,
) -> Response {
    proxy(state, path.server, None, uri.query(), method, headers, body).await
}

pub(crate) async fn proxy_tail(
    State(state): State<AppState>,
    Path(path): Path<McpTailPath>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    proxy(
        state,
        path.server,
        Some(path.path),
        uri.query(),
        method,
        headers,
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
                return error(
                    StatusCode::FORBIDDEN,
                    "no live MCP OAuth session authorizes this user and server",
                    "mcp_session_unauthorized",
                );
            };
            if chrono::Utc::now() >= session.expires_at {
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
            transport_overrides(server),
        )
        .await
    {
        Ok(response) => upstream_response(response),
        Err(upstream) => error(
            StatusCode::BAD_GATEWAY,
            format!("MCP upstream request failed: {upstream}"),
            "mcp_upstream_error",
        ),
    }
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

/// The server's transport overrides, in the form the forwarder takes. A `None`
/// column inherits the deployment default rather than becoming a zero.
fn transport_overrides(server: &McpServerConfig) -> McpTransportOverrides {
    McpTransportOverrides {
        connect_timeout: server
            .connect_timeout_ms
            .map(std::time::Duration::from_millis),
        request_timeout: server
            .request_timeout_ms
            .map(std::time::Duration::from_millis),
        max_retries: server.max_retries.unwrap_or(0),
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

fn upstream_response(response: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = end_to_end_headers(response.headers().clone());
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
    builder
        .body(Body::from_stream(response.bytes_stream()))
        .unwrap_or_else(|_| {
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

    #[test]
    fn downstream_url_preserves_tail_and_query() {
        assert_eq!(
            downstream_url("https://mcp.example/rpc/", Some("messages/1"), Some("v=2")),
            Some("https://mcp.example/rpc/messages/1?v=2".to_string())
        );
    }
}
