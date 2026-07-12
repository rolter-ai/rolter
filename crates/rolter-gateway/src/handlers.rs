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

    let vk = match authenticate(&state, &snap, &headers) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    if let Some(vk) = &vk {
        if !rolter_auth::model_allowed(&vk.models, &model) {
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

    let entry = match snap.routes.get(&model) {
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
    if entry.route.targets.is_empty() && !entry.route.has_variants() {
        return error_json(StatusCode::SERVICE_UNAVAILABLE, "route has no targets");
    }

    // inject the admin's per-model param defaults (temperature, max_tokens, ...)
    // before forwarding; caller values survive only where the override policy
    // permits. falls back to the untouched body if re-serialization fails
    let forward_body = if entry.route.params.is_empty() {
        body.clone()
    } else {
        let mut injected = parsed.clone();
        entry.route.apply_params(&mut injected);
        serde_json::to_vec(&injected)
            .map(Bytes::from)
            .unwrap_or_else(|_| body.clone())
    };

    // capture log fields independent of the chosen target
    let stream = parsed
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    // the ensure_request_id middleware guarantees this header is present
    let request_id = headers
        .get(crate::trace::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    // adopt the caller's distributed trace when one was propagated inbound
    let trace_id = crate::trace::inbound_trace_id(&headers);
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
    // the caller's inbound trace context, propagated verbatim to whichever
    // upstream is chosen so it continues the same distributed trace (ROL-61)
    let trace_ctx = crate::trace::outbound_trace_headers(&headers);
    let trace_headers: Vec<(&str, &str)> =
        trace_ctx.iter().map(|(k, v)| (*k, v.as_str())).collect();

    // pick a target and forward, retrying transient failures on a fresh target
    // (exponential backoff + jitter). retries happen before any body bytes reach
    // the client, so a partial response is never duplicated. a route with
    // variants routes through the weighted-variant fallback chain instead of the
    // classic single-pool balancer.
    let (outcome, last_provider, last_target, last_error, inflight_guard, chosen_variant) =
        if entry.route.has_variants() {
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
            )
            .await;
            (
                fwd.outcome,
                fwd.last_provider,
                fwd.last_target,
                fwd.last_error,
                fwd.inflight_guard,
                fwd.variant,
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
            let mut tried: Vec<usize> = Vec::new();
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
                ) {
                    Some(i) => i,
                    None => break, // no untried target left to fail over to
                };
                entry.balancer.observe(idx, &ctx);
                tried.push(idx);
                // count this attempt as in-flight; the guard falls out of scope (and
                // decrements) on retry, or is moved into the stream wrapper on success
                let mut guard = state.loads.begin(&model, idx);

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
                let upstream_model = target.model.as_deref();

                match state
                    .forwarder
                    .forward_json(
                        provider,
                        path,
                        forward_body.clone(),
                        api_key,
                        upstream_model,
                        &trace_headers,
                    )
                    .await
                {
                    Ok(response) => {
                        let status = response.status().as_u16();
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
                        last_error = Some(err.to_string());
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
            )
        };

    match outcome {
        // token counts, latency and ttft are filled by the stream wrapper once
        // the body has been fully forwarded to the client
        Some((response, status, is_sse)) => {
            let log = RequestLog {
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: chosen_variant,
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
                inflight_guard,
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
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: chosen_variant,
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

    let vk = match authenticate(&state, &snap, &headers) {
        Ok(vk) => vk,
        Err(resp) => return resp,
    };
    if let Some(vk) = &vk {
        if !rolter_auth::model_allowed(&vk.models, &model) {
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

    let entry = match snap.routes.get(&model) {
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
    let trace_id = crate::trace::inbound_trace_id(&headers);
    let (vk_id, org_id, team_id, project_id) = (
        scope.key.clone(),
        scope.org.clone(),
        scope.team.clone(),
        scope.project.clone(),
    );
    let token_recorder = TokenRecorder::new(
        state.rate_limiter.clone(),
        snap.rate_limits.clone(),
        scope.clone(),
    );
    let recorder = SpendRecorder::new(state.budgets.clone(), snap.budgets.clone(), scope);
    let price = snap.prices.get(&model).cloned();

    let ctx = RouteContext {
        session_key: headers.get("x-session-id").and_then(|v| v.to_str().ok()),
        prompt: None,
    };
    let trace_ctx = crate::trace::outbound_trace_headers(&headers);
    let trace_headers: Vec<(&str, &str)> =
        trace_ctx.iter().map(|(k, v)| (*k, v.as_str())).collect();

    let retry = &snap.retry;
    let cooldown = &snap.cooldown;
    let cd_enabled = cooldown.enabled();
    let mut loads = state.loads.snapshot(&model, entry.route.targets.len());
    for (i, target) in entry.route.targets.iter().enumerate() {
        if let Some(l) = loads.get_mut(i) {
            *l = l.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
        }
    }
    let mut tried: Vec<usize> = Vec::new();
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
        ) {
            Some(i) => i,
            None => break,
        };
        entry.balancer.observe(idx, &ctx);
        tried.push(idx);
        let mut guard = state.loads.begin(&model, idx);

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
            .forwarder
            .forward_raw(
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
                last_error = Some(err.to_string());
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

    match outcome {
        Some((response, status, is_sse)) => {
            let log = RequestLog {
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: String::new(),
                status,
                stream: 0,
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
                inflight_guard,
            )
        }
        None => {
            if last_error.is_none() {
                return error_json(StatusCode::SERVICE_UNAVAILABLE, "no target selected");
            }
            state.metrics.upstream_errors_total.fetch_add(1, Relaxed);
            let message = last_error.unwrap_or_default();
            state.log.log(RequestLog {
                request_id,
                trace_id,
                virtual_key_id: vk_id,
                org_id,
                team_id,
                project_id,
                model,
                provider: last_provider,
                target: last_target,
                variant: String::new(),
                status: StatusCode::BAD_GATEWAY.as_u16(),
                stream: 0,
                latency_ms: started.elapsed().as_millis() as u32,
                error: message.clone(),
                ..Default::default()
            });
            error_json(StatusCode::BAD_GATEWAY, &message)
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
}

/// Namespaces the per-target reliability registries (cooldown/breaker/load) by
/// variant so a target's health under one variant never leaks into another.
pub(crate) fn variant_key(model: &str, variant: &str) -> String {
    format!("{model}::{variant}")
}

/// Namespaces the per-key cooldown registry: keys are parked per provider,
/// shared across every route and variant that uses that provider.
fn key_pool_key(provider: &str) -> String {
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
) -> ForwardOutcome {
    let route = &entry.route;
    let retry = &snap.retry;
    let cooldown = &snap.cooldown;
    let cd_enabled = cooldown.enabled();

    // primary by weight, then the rest in declared order; flatten each variant's
    // targets into one ordered candidate list, letting the variant's balancer
    // choose which of its targets leads
    let primary = route.sample_variant(jitter(started)).unwrap_or(0);
    let mut candidates: Vec<(usize, usize)> = Vec::new();
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
                candidates.push((vi, ti));
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
    };
    let mut tried: Vec<usize> = Vec::new();

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
        // weighted pick across the provider's key pool, skipping keys parked
        // on a cooldown — same policy as the classic path
        let multi_key = provider.api_keys.len() > 1;
        let key_ns = key_pool_key(&target.provider);
        let picked_key = provider.pick_api_key_indexed(jitter(started), |i| {
            multi_key && state.cooldowns.is_parked(&key_ns, i)
        });
        let api_key = picked_key.as_ref().map(|(_, k)| k.as_str());
        let upstream_model = target.model.as_deref();

        match state
            .forwarder
            .forward_json(
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
                out.last_error = Some(err.to_string());
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
fn pick_untried(
    entry: &crate::state::RouteEntry,
    ctx: &RouteContext,
    tried: &[usize],
    loads: &[u64],
    cooldowns: &crate::cooldowns::Cooldowns,
    health: &crate::health::Health,
    breaker: &crate::breaker::Breaker,
    model: &str,
    cd_enabled: bool,
) -> Option<usize> {
    // a target is skippable when parked on a cooldown, when its provider is
    // currently marked unhealthy by the active prober, or when its circuit
    // breaker is open
    let skip = |i: usize| {
        (cd_enabled && cooldowns.is_parked(model, i))
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
    inflight_guard: Option<crate::load::LoadGuard>,
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
    // whether it was a cache hit (ROL-58). cache is always a miss until the
    // response cache lands (ROL-56); the header flips automatically once
    // `log.cache_hit` is set upstream
    let decision = DecisionHeaders::from_log(&log);
    let body = crate::logging::UsageLoggingStream::new(
        Box::pin(response.bytes_stream()),
        is_sse,
        started,
        sink,
        price,
        log,
        Some(recorder),
        Some(token_recorder),
        inflight_guard,
    );
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

    fn config_with_keys() -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.routes.push(ModelRoute {
            model: "gpt-4o".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
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
            balancer: rolter_balancer::build(route.strategy, &[]),
            variant_balancers: vec![Box::new(Fixed(1))],
            route: route.clone(),
        };
        assert_eq!(variant_target_order(&entry, &ctx, 0, 2, &[]), vec![1, 0]);
        // an out-of-range pick degrades to plain declared order
        let entry = crate::state::RouteEntry {
            balancer: rolter_balancer::build(route.strategy, &[]),
            variant_balancers: vec![Box::new(Fixed(9))],
            route: route.clone(),
        };
        assert_eq!(variant_target_order(&entry, &ctx, 0, 2, &[]), vec![0, 1]);
        // no balancer built for the variant: declared order
        let entry = crate::state::RouteEntry {
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
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::default();
        let first = pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false).unwrap();
        // with the first target excluded, the fallback must choose the other one
        let second = pick_untried(&entry, &ctx, &[first], &[], &cd, &hh, &bb, "m", false).unwrap();
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
                false
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
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true),
            Some(1)
        );
        // park both: fail open to an untried target rather than returning None
        cd.park("m", 1, 60);
        assert!(pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", true).is_some());
    }

    #[test]
    fn pick_untried_skips_unhealthy_provider() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
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
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false),
            Some(1)
        );
        // both providers unhealthy: fail open rather than returning None
        hh.set("b", false);
        assert!(pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false).is_some());
    }

    #[test]
    fn pick_untried_skips_open_breaker() {
        let route = ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::RoundRobin,
            params: Default::default(),
            param_policy: Default::default(),
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
            balancer: rolter_balancer::build(route.strategy, &[1, 1]),
            variant_balancers: Vec::new(),
            route,
        };
        let ctx = RouteContext::default();
        let cd = crate::cooldowns::Cooldowns::default();
        let hh = crate::health::Health::default();
        let bb = crate::breaker::Breaker::new(1, 60);
        // trip target 0 open: selection must avoid it and pick 1
        assert!(bb.on_failure("m", 0));
        assert_eq!(
            pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false),
            Some(1)
        );
        // both open: fail open to an untried target rather than returning None
        assert!(bb.on_failure("m", 1));
        assert!(pick_untried(&entry, &ctx, &[], &[], &cd, &hh, &bb, "m", false).is_some());
    }
}
