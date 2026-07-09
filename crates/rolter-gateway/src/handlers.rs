use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};

use tokio::time::sleep;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use chrono::Utc;
use serde_json::{json, Value};

use rolter_balancer::RouteContext;

use crate::budgets::{ScopeIds, SpendRecorder};
use crate::fake_llm;
use crate::logging::RequestLog;
use crate::rate_limits::TokenRecorder;
use crate::state::{AppState, KeyMeta, Snapshot};

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
) -> Result<Option<KeyMeta>, Response> {
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
    let started = Instant::now();

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

    // scope identity drives both budget enforcement and log attribution
    let scope = vk
        .as_ref()
        .map(|v| ScopeIds {
            org: v.org_id.clone(),
            team: v.team_id.clone(),
            project: v.project_id.clone(),
            key: v.id.clone(),
        })
        .unwrap_or_default();

    // block before spending upstream tokens when any applicable budget is spent
    if let Some(exceeded) = state.budgets.exceeded(&snap.budgets, &scope).await {
        state.metrics.budget_blocks_total.fetch_add(1, Relaxed);
        return error_json(
            StatusCode::PAYMENT_REQUIRED,
            &format!(
                "budget exceeded for {:?} '{}' (limit ${:.2})",
                exceeded.scope, exceeded.id, exceeded.limit_usd
            ),
        );
    }

    // throughput cap: reject before forwarding when a matching request/token
    // window is already at capacity (admission also counts the request)
    if let Some(hit) = state.rate_limiter.check(&snap.rate_limits, &scope).await {
        state.metrics.rate_limit_blocks_total.fetch_add(1, Relaxed);
        let mut resp = error_json(
            StatusCode::TOO_MANY_REQUESTS,
            &format!(
                "{} rate limit exceeded for {:?} '{}' (limit {}/min)",
                hit.kind, hit.scope, hit.id, hit.limit
            ),
        );
        resp.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(hit.retry_after));
        return resp;
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

    // capture log fields independent of the chosen target
    let stream = parsed
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // scope identity for log attribution (empty for config-defined keys)
    let (vk_id, org_id, team_id, project_id) = (
        scope.key.clone(),
        scope.org.clone(),
        scope.team.clone(),
        scope.project.clone(),
    );
    // records this request's tokens against its rate limits once usage is known
    let token_recorder = TokenRecorder::new(
        state.rate_limiter.clone(),
        snap.rate_limits.clone(),
        scope.clone(),
    );
    // records this request's cost against its budgets once cost_usd is known
    let recorder = SpendRecorder::new(state.budgets.clone(), snap.budgets.clone(), scope);
    let price = snap.prices.get(&model).cloned();

    let session_key = headers.get("x-session-id").and_then(|v| v.to_str().ok());
    let prompt = std::str::from_utf8(&body).ok();
    let ctx = RouteContext {
        session_key,
        prompt,
    };

    // pick a target and forward, retrying transient failures on a fresh target
    // (exponential backoff + jitter). retries happen before any body bytes reach
    // the client, so a partial response is never duplicated.
    let retry = &snap.retry;
    let cooldown = &snap.cooldown;
    let cd_enabled = cooldown.enabled();
    let mut tried: Vec<usize> = Vec::new();
    let mut last_provider = String::new();
    let mut last_target = model.clone();
    let mut last_error: Option<String> = None;
    let mut outcome: Option<(reqwest::Response, u16, bool)> = None;

    for attempt in 0..=retry.max_retries {
        let idx = match pick_untried(entry, &ctx, &tried, &state.cooldowns, &model, cd_enabled) {
            Some(i) => i,
            None => break, // no untried target left to fail over to
        };
        entry.balancer.observe(idx, &ctx);
        tried.push(idx);

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
        let api_key = provider.resolve_api_key();
        let upstream_model = target.model.as_deref();

        match state
            .forwarder
            .forward_json(
                provider,
                path,
                body.clone(),
                api_key.as_deref(),
                upstream_model,
            )
            .await
        {
            Ok(response) => {
                let status = response.status().as_u16();
                if is_retryable_status(status) {
                    // park the failing target so siblings absorb the load
                    if cd_enabled {
                        let secs = cooldown.duration_secs(retry_after_secs(&response));
                        state.cooldowns.park(&model, idx, secs);
                        state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
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
                }
                let is_sse = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|ct| ct.contains("event-stream"))
                    .unwrap_or(false);
                outcome = Some((response, status, is_sse));
                break;
            }
            Err(err) => {
                last_error = Some(err.to_string());
                // a connection-level failure parks the target too
                if cd_enabled {
                    state
                        .cooldowns
                        .park(&model, idx, cooldown.duration_secs(None));
                    state.metrics.cooldowns_tripped_total.fetch_add(1, Relaxed);
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

    match outcome {
        // token counts, latency and ttft are filled by the stream wrapper once
        // the body has been fully forwarded to the client
        Some((response, status, is_sse)) => {
            let log = RequestLog {
                request_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                model,
                provider: last_provider,
                target: last_target,
                status,
                stream: stream as u8,
                ..Default::default()
            };
            stream_response(
                response,
                is_sse,
                started,
                state.log.clone(),
                price,
                log,
                recorder,
                token_recorder,
            )
        }
        None => {
            // no attempt ever reached an upstream: the balancer had no target
            if last_error.is_none() {
                return error_json(StatusCode::SERVICE_UNAVAILABLE, "no target selected");
            }
            state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
            let message = last_error.unwrap_or_default();
            state.log.log(RequestLog {
                request_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                model,
                provider: last_provider,
                target: last_target,
                status: StatusCode::BAD_GATEWAY.as_u16(),
                stream: stream as u8,
                latency_ms: started.elapsed().as_millis() as u32,
                error: message.clone(),
                ..Default::default()
            });
            error_json(StatusCode::BAD_GATEWAY, &message)
        }
    }
}

/// Pick a target the request has not tried yet, preferring one that is not on a
/// cooldown. Honours the balancer's choice when it is fresh and healthy; else
/// falls over to the first untried, un-parked target. When every remaining
/// target is parked it fails open to the first untried one so requests still
/// flow rather than 503-ing on a transient wobble.
fn pick_untried(
    entry: &crate::state::RouteEntry,
    ctx: &RouteContext,
    tried: &[usize],
    cooldowns: &crate::cooldowns::Cooldowns,
    model: &str,
    cd_enabled: bool,
) -> Option<usize> {
    let parked = |i: usize| cd_enabled && cooldowns.is_parked(model, i);
    if let Some(i) = entry.balancer.pick(ctx, &[]) {
        if !tried.contains(&i) && !parked(i) {
            return Some(i);
        }
    }
    let n = entry.route.targets.len();
    (0..n)
        .find(|i| !tried.contains(i) && !parked(*i))
        .or_else(|| (0..n).find(|i| !tried.contains(i)))
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
fn stream_response(
    response: reqwest::Response,
    is_sse: bool,
    started: Instant,
    sink: crate::logging::LogSink,
    price: Option<rolter_core::ModelPriceConfig>,
    log: RequestLog,
    recorder: SpendRecorder,
    token_recorder: TokenRecorder,
) -> Response {
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let body = crate::logging::UsageLoggingStream::new(
        Box::pin(response.bytes_stream()),
        is_sse,
        started,
        sink,
        price,
        log,
        Some(recorder),
        Some(token_recorder),
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from_stream(body))
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

    use rolter_core::{
        BalancingStrategy, GatewayConfig, ModelRoute, Target, VirtualKeyConfig, VirtualKeyRecord,
    };

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
            models: vec![],
            disabled: false,
            expires_at: None,
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
    fn pick_untried_fails_over_to_sibling() {
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
        };
        let entry = crate::state::RouteEntry {
            balancer: rolter_balancer::build(route.strategy, route.targets.len()),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let first = pick_untried(&entry, &ctx, &[], &cd, "m", false).unwrap();
        // with the first target excluded, the fallback must choose the other one
        let second = pick_untried(&entry, &ctx, &[first], &cd, "m", false).unwrap();
        assert_ne!(first, second);
        // both tried: nothing left to fail over to
        assert_eq!(
            pick_untried(&entry, &ctx, &[first, second], &cd, "m", false),
            None
        );
    }

    #[test]
    fn pick_untried_skips_parked_target() {
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
        };
        let entry = crate::state::RouteEntry {
            balancer: rolter_balancer::build(route.strategy, route.targets.len()),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::new();
        // park target 0: selection must avoid it and pick 1
        cd.park("m", 0, 60);
        assert_eq!(pick_untried(&entry, &ctx, &[], &cd, "m", true), Some(1));
        // park both: fail open to an untried target rather than returning None
        cd.park("m", 1, 60);
        assert!(pick_untried(&entry, &ctx, &[], &cd, "m", true).is_some());
    }
}
