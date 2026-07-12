//! `/admin` — LiteLLM-style management surface on the gateway port.
//!
//! The gateway itself stays a thin data plane: these routes reverse-proxy
//! `/admin/*` to the control plane's `/api/v1/*` management API, so a
//! deployment can point every client — inference and administration — at one
//! host and still get Postgres-backed, reload-free provider/model CRUD.
//! Authentication is enforced by the control plane (`ROLTER_ADMIN_TOKEN`);
//! the `Authorization` header passes through untouched.

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use serde_json::json;

#[derive(Clone)]
struct AdminProxy {
    client: reqwest::Client,
    /// control-plane base url, e.g. `http://127.0.0.1:4001` (no trailing slash)
    base_url: String,
}

/// Build the `/admin` router forwarding to the control plane at `base_url`.
pub fn router(base_url: &str) -> Router {
    let state = AdminProxy {
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new()),
        base_url: base_url.trim_end_matches('/').to_string(),
    };
    Router::new()
        .route("/admin/{*path}", any(forward))
        .with_state(state)
}

/// Forward one request to the control plane, preserving method, path, query,
/// body and the `Authorization`/`Content-Type` headers.
async fn forward(
    State(proxy): State<AdminProxy>,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{}/api/v1/{path}{query}", proxy.base_url);

    let method = match reqwest::Method::from_bytes(method.as_str().as_bytes()) {
        Ok(m) => m,
        Err(_) => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };
    let mut request = proxy.client.request(method, &url).body(body);
    for name in [header::AUTHORIZATION, header::CONTENT_TYPE] {
        if let Some(value) = headers.get(&name) {
            if let Ok(value) = value.to_str() {
                request = request.header(name.as_str(), value);
            }
        }
    }

    match request.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let content_type = resp
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            match resp.bytes().await {
                Ok(bytes) => Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(bytes))
                    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response()),
                Err(err) => bad_gateway(&err),
            }
        }
        Err(err) => bad_gateway(&err),
    }
}

fn bad_gateway(err: &dyn std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error": {"message": format!("control plane unreachable: {err}")}})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::post;

    async fn serve(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn forwards_method_path_query_body_and_auth() {
        // control-plane stub that echoes what it saw
        async fn echo(
            method: Method,
            uri: Uri,
            headers: HeaderMap,
            body: String,
        ) -> Json<serde_json::Value> {
            Json(json!({
                "method": method.as_str(),
                "path": uri.path(),
                "query": uri.query(),
                "auth": headers.get("authorization").and_then(|v| v.to_str().ok()),
                "body": body,
            }))
        }
        let control = Router::new().route("/api/v1/{*rest}", post(echo).get(echo));
        let control_addr = serve(control).await;

        let gateway = router(&format!("http://{control_addr}"));
        let gateway_addr = serve(gateway).await;

        let client = reqwest::Client::new();
        let resp: serde_json::Value = client
            .post(format!(
                "http://{gateway_addr}/admin/orgs/abc/providers?x=1"
            ))
            .header("authorization", "Bearer sekrit")
            .header("content-type", "application/json")
            .body(r#"{"name":"openai"}"#)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(resp["method"], "POST");
        assert_eq!(resp["path"], "/api/v1/orgs/abc/providers");
        assert_eq!(resp["query"], "x=1");
        assert_eq!(resp["auth"], "Bearer sekrit");
        assert_eq!(resp["body"], r#"{"name":"openai"}"#);
    }

    #[tokio::test]
    async fn unreachable_control_plane_maps_to_bad_gateway() {
        let gateway = router("http://127.0.0.1:1");
        let addr = serve(gateway).await;
        let resp = reqwest::Client::new()
            .get(format!("http://{addr}/admin/orgs"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 502);
    }
}
