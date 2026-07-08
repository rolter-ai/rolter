use std::sync::atomic::Ordering::Relaxed;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use chrono::Utc;
use serde_json::{json, Value};

use rolter_balancer::RouteContext;
use rolter_core::VirtualKeyConfig;

use crate::fake_llm;
use crate::state::{AppState, Snapshot};

/// Liveness probe.
pub async fn healthz() -> &'static str {
    "ok"
}

/// Prometheus metrics endpoint.
pub async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}

/// OpenAI-compatible model listing built from configured routes, filtered to
/// the models the caller's virtual key is allowed to see.
pub async fn list_models(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let snap = state.snapshot.load();
    let vk = match authenticate(&state, &snap, &headers) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    let builtin = std::iter::once(fake_llm::MODEL_NAME)
        .filter(|m| !snap.routes.contains_key(*m))
        .map(str::to_string);
    let data: Vec<Value> = snap
        .routes
        .keys()
        .cloned()
        .chain(builtin)
        .filter(|m| {
            vk.as_ref()
                .is_none_or(|vk| rolter_auth::model_allowed(&vk.models, m))
        })
        .map(|m| json!({"id": m, "object": "model", "owned_by": "rolter"}))
        .collect();
    Json(json!({"object": "list", "data": data})).into_response()
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

pub async fn messages(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    proxy(state, headers, body, "/v1/messages").await
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

fn error_json(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"message": message, "type": "rolter_error"}})),
    )
        .into_response()
}

/// Shared virtual-key auth check for every `/v1/*` handler. Returns the
/// matched key (or `None` when no keys are configured, i.e. auth disabled).
#[allow(clippy::result_large_err)]
fn authenticate(
    state: &AppState,
    snap: &Snapshot,
    headers: &HeaderMap,
) -> Result<Option<VirtualKeyConfig>, Response> {
    if snap.keys.is_empty() {
        return Ok(None);
    }
    match extract_key(headers) {
        Some(key) => {
            // look up by peppered digest so the plaintext key is never used as a
            // map key and never lingers in memory beyond this call
            let digest = rolter_auth::hash_key(&snap.pepper, &key);
            match snap.keys.get(&digest) {
                Some(vk) if vk.is_active(Utc::now()) => Ok(Some(vk.clone())),
                // matched but revoked or expired: do not distinguish from a
                // missing key in the response
                Some(_) => {
                    state.metrics.auth_failures_total.fetch_add(1, Relaxed);
                    Err(error_json(StatusCode::UNAUTHORIZED, "invalid api key"))
                }
                None => {
                    state.metrics.auth_failures_total.fetch_add(1, Relaxed);
                    Err(error_json(StatusCode::UNAUTHORIZED, "invalid api key"))
                }
            }
        }
        None => {
            state.metrics.auth_failures_total.fetch_add(1, Relaxed);
            Err(error_json(StatusCode::UNAUTHORIZED, "missing api key"))
        }
    }
}

/// Shared proxy pipeline: parse, authenticate, balance, forward, stream back.
async fn proxy(state: AppState, headers: HeaderMap, body: Bytes, path: &str) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);

    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return error_json(StatusCode::BAD_REQUEST, "invalid json body"),
    };
    let model = match parsed.get("model").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => return error_json(StatusCode::BAD_REQUEST, "missing model field"),
    };

    let snap = state.snapshot.load();

    let vk = match authenticate(&state, &snap, &headers) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    if let Some(vk) = &vk {
        if !rolter_auth::model_allowed(&vk.models, &model) {
            return error_json(StatusCode::FORBIDDEN, "model not allowed for this key");
        }
    }

    // built-in fake-llm answers locally unless a configured route shadows it
    if model == fake_llm::MODEL_NAME && !snap.routes.contains_key(&model) {
        return match path {
            "/v1/chat/completions" => fake_llm::chat_completions(&parsed),
            "/v1/messages" => fake_llm::messages(&parsed),
            _ => error_json(
                StatusCode::NOT_FOUND,
                &format!("'{model}' is not served on {path}"),
            ),
        };
    }

    let entry = match snap.routes.get(&model) {
        Some(entry) => entry,
        None => {
            return error_json(
                StatusCode::NOT_FOUND,
                &format!("no route for model '{model}'"),
            )
        }
    };
    if entry.route.targets.is_empty() {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "route has no targets");
    }

    // scope the body borrow so the bytes can be moved into the forwarder after
    let picked = {
        let session_key = headers.get("x-session-id").and_then(|v| v.to_str().ok());
        let prompt = std::str::from_utf8(&body).ok();
        let ctx = RouteContext {
            session_key,
            prompt,
        };
        let idx = entry.balancer.pick(&ctx, &[]);
        if let Some(i) = idx {
            entry.balancer.observe(i, &ctx);
        }
        idx
    };
    let idx = match picked {
        Some(i) => i,
        None => return error_json(StatusCode::SERVICE_UNAVAILABLE, "no target selected"),
    };

    let target = &entry.route.targets[idx];
    let provider = match snap.providers.get(&target.provider) {
        Some(provider) => provider,
        None => {
            return error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "configured target provider not found",
            )
        }
    };

    let api_key = provider.resolve_api_key();
    let upstream_model = target.model.as_deref();

    match state
        .forwarder
        .forward_json(provider, path, body, api_key.as_deref(), upstream_model)
        .await
    {
        Ok(response) => stream_response(response),
        Err(err) => {
            state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
            error_json(StatusCode::BAD_GATEWAY, &err.to_string())
        }
    }
}

/// Convert an upstream response into a streaming axum response.
fn stream_response(response: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(response.bytes_stream()))
        .unwrap_or_else(|_| {
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build response",
            )
        })
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::extract::State;
    use axum::http::HeaderValue;

    use rolter_core::{BalancingStrategy, GatewayConfig, ModelRoute, Target, VirtualKeyConfig};

    use super::*;

    fn config_with_keys() -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.routes.push(ModelRoute {
            model: "gpt-4o".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            targets: vec![Target {
                provider: "openai".to_string(),
                model: None,
                weight: 1,
            }],
        });
        config.routes.push(ModelRoute {
            model: "claude".to_string(),
            strategy: BalancingStrategy::RoundRobin,
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
            disabled: false,
            expires_at: None,
        });
        config
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
    async fn expired_key_is_rejected() {
        let vk = VirtualKeyConfig {
            key: "sk-expired".to_string(),
            name: None,
            models: vec![],
            disabled: false,
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
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
            disabled: true,
            expires_at: None,
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
            disabled: false,
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
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
}
