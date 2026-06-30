use std::sync::atomic::Ordering::Relaxed;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use serde_json::{json, Value};

use rolter_balancer::RouteContext;

use crate::state::AppState;

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

/// OpenAI-compatible model listing built from configured routes.
pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let snap = state.snapshot.load();
    let data: Vec<Value> = snap
        .routes
        .keys()
        .map(|m| json!({"id": m, "object": "model", "owned_by": "rolter"}))
        .collect();
    Json(json!({"object": "list", "data": data}))
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

    // virtual-key auth is enforced only when keys are configured
    if !snap.keys.is_empty() {
        match extract_key(&headers) {
            Some(key) => match snap.keys.get(&key) {
                Some(vk) if rolter_auth::model_allowed(&vk.models, &model) => {}
                Some(_) => {
                    return error_json(StatusCode::FORBIDDEN, "model not allowed for this key")
                }
                None => {
                    state.metrics.auth_failures_total.fetch_add(1, Relaxed);
                    return error_json(StatusCode::UNAUTHORIZED, "invalid api key");
                }
            },
            None => {
                state.metrics.auth_failures_total.fetch_add(1, Relaxed);
                return error_json(StatusCode::UNAUTHORIZED, "missing api key");
            }
        }
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
