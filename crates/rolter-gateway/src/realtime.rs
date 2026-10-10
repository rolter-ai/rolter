//! Persistent WebSocket relay for OpenAI-compatible Realtime sessions.
//!
//! A Realtime connection is intentionally selected once, before the downstream
//! HTTP upgrade is accepted. The selected provider/key is then pinned for the
//! lifetime of the socket: reconnecting is a client operation, never an
//! invisible mid-session failover that could duplicate audio or tool events.
//!
//! Admission is the HTTP request path's, keyed on the same scope chain: a spent
//! budget, a full rate-limit window or an unpriced model under a `block` policy
//! refuses the upgrade before any upstream is dialled. Once the session is
//! live, [`crate::realtime_metering`] meters it per response turn and closes it
//! when its budget runs out (#1396).
//!
//! A session outlives the HTTP request that opened it, so axum's graceful
//! shutdown does not wait for it. [`Sessions`] tracks every relay and meter
//! task instead, and a shutting-down gateway closes each session and waits for
//! its meter's last flush before the process exits.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{
    ws::{close_code, CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
    Query, State,
};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use rolter_balancer::RouteContext;
use serde::Deserialize;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as UpstreamMessage},
};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::handlers::{
    authenticate, authorize_model, authorize_route, budget_refusal, key_pool_key, pick_untried,
    rate_limit_refusal, request_scope, unpriced_admission, variant_key,
};
use crate::realtime_guard::{ClientVerdict, ContentPolicy, Frame};
use crate::realtime_metering::{Closure, SessionEnd, SessionMeter, TurnTracker};
use crate::state::{AppState, Snapshot};

/// How long a shutting-down relay may spend writing its close frames. A
/// client that stopped reading must not hold the drain past its grace and cost
/// the meter its last flush.
const CLOSE_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// Process-local registry of persistent sessions: the admission counter, plus
/// what shutdown needs to end them.
///
/// axum's graceful shutdown stops tracking a connection once it upgrades to a
/// WebSocket, so it would let the process exit under a live session and drop
/// the turns its meter had not flushed yet. Every relay and meter task is
/// tracked here instead, and `closing` tells the relays to end.
#[derive(Clone, Default)]
pub(crate) struct Sessions {
    live: Arc<AtomicU64>,
    closing: CancellationToken,
    tasks: TaskTracker,
}

impl Sessions {
    fn acquire(&self, limit: u64) -> Option<SessionGuard> {
        loop {
            let current = self.live.load(Relaxed);
            if limit != 0 && current >= limit {
                return None;
            }
            if self
                .live
                .compare_exchange_weak(current, current + 1, Relaxed, Relaxed)
                .is_ok()
            {
                return Some(SessionGuard(self.clone()));
            }
        }
    }

    /// Tell every live session to end, and refuse new ones. Idempotent.
    pub(crate) fn close(&self) {
        self.closing.cancel();
    }

    /// Wait, at most `grace`, for every session's relay and meter to finish.
    /// Returns whether they all did.
    pub(crate) async fn drained(&self, grace: Duration) -> bool {
        self.tasks.close();
        tokio::time::timeout(grace, self.tasks.wait()).await.is_ok()
    }

    /// Run a session's meter where shutdown waits for it.
    pub(crate) fn spawn_meter<F>(&self, meter: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(meter);
    }
}

struct SessionGuard(Sessions);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Relaxed);
    }
}

#[derive(Deserialize)]
pub struct RealtimeQuery {
    model: String,
}

/// Upgrade a client into a pinned, bidirectional Realtime WebSocket session.
pub async fn realtime(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RealtimeQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    state.metrics.requests_total.fetch_add(1, Relaxed);
    let snap = state.snapshot.load();
    let virtual_key = match authenticate(&state, &snap, &headers, "/v1/realtime") {
        Ok(key) => key,
        Err(response) => return response,
    };
    if let Err(denial) = authorize_model(virtual_key.as_ref(), &query.model) {
        return denial.into_response();
    }

    // another org's route is absent here, as it is on the HTTP pipelines
    let entry = match snap.named_route_for(&query.model, virtual_key.as_ref()) {
        Some(entry) => entry,
        // the same answer as the HTTP pipelines give a model nobody configured
        None => {
            return crate::error::ApiError::new(
                StatusCode::NOT_FOUND,
                format!("no route for model '{}'", query.model),
            )
            .with_code("model_not_found")
            .with_param("model")
            .into_response()
        }
    };
    if entry.route.targets.is_empty() && !entry.route.has_variants() {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "route has no targets");
    }
    // the same route gate as the HTTP pipelines, so a route hidden from this
    // key cannot be reached by upgrading to a socket instead (#1485)
    if let Err(denial) = authorize_route(virtual_key.as_ref(), entry) {
        return denial.into_response();
    }

    // the sanitizer cannot run on a realtime socket (#2489), so a deployment
    // that made it a hard requirement must not get a session that silently
    // skips it. fail-open keeps its availability-over-enforcement contract and
    // is admitted. refused before any budget, rate-limit or upstream side effect
    if sanitizer_blocks_realtime(&snap.pii_sanitizer) {
        return crate::error::ApiError::new(
            StatusCode::BAD_REQUEST,
            "this deployment requires the PII sanitizer (failure_mode = fail_closed), which \
             cannot be applied to a realtime session; use a non-realtime endpoint",
        )
        .with_code("sanitizer_unsupported_on_realtime")
        .into_response();
    }

    // the policy an HTTP request meets, on the same scope chain (#1396). a
    // session opens only while every budget it draws on has room left, and
    // only for a model this deployment is willing to serve unpriced
    let scope = request_scope(virtual_key.as_ref());
    if let Some(refusal) = budget_refusal(&state, &snap, &scope).await {
        return refusal;
    }
    let priced = snap.prices.contains_key(&entry.route.model);
    if let Some(refusal) = unpriced_admission(&state, &snap, &scope, &entry.route.model, priced) {
        return refusal;
    }
    // a session opened now would be closed before its first turn
    if state.realtime_sessions.closing.is_cancelled() {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "gateway shutting down");
    }
    let Some(session_guard) = state
        .realtime_sessions
        .acquire(snap.realtime.max_connections)
    else {
        return api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "realtime session limit reached",
        );
    };
    // after the process-local cap, so a session this gateway could not have
    // held anyway never takes a slot of the key's `rpm`. opening a session is
    // one request; the tokens its turns use reach `tpm` as they are metered
    if let Some(refusal) = rate_limit_refusal(&state, &snap, &scope).await {
        return refusal;
    }

    // realtime has no request body, so session affinity uses the caller-supplied
    // session id and strategy-aware balancing sees an empty prompt
    let session_key = headers.get("x-session-id").and_then(|v| v.to_str().ok());
    let context = RouteContext {
        session_key,
        prompt: None,
        prompt_len: None,
        prompt_digest: None,
        token_ids: None,
        // realtime sessions are not adapter-addressed
        adapter: None,
    };
    let selected = match connect_selected(
        &state,
        &snap,
        entry,
        &query.model,
        &context,
        &headers,
        session_guard,
        virtual_key.as_ref(),
    )
    .await
    {
        Ok(selected) => selected,
        Err(message) => {
            tracing::warn!(error = %message, "realtime upstream connection failed");
            return api_error(
                StatusCode::BAD_GATEWAY,
                "upstream realtime connection failed",
            );
        }
    };

    // the ensure_request_id middleware guarantees this header is present
    let request_id = headers
        .get(crate::trace::REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    crate::trace::record_tenant(&scope.org, &scope.team, &scope.project);
    // resolved before `scope` moves into the meter, and pinned for the session
    let trace_id = crate::trace::request_trace_id(&headers);
    let policy = ContentPolicy::for_session(
        &snap,
        entry,
        &scope,
        &query.model,
        &trace_id,
        &state.side_client,
    );
    let meter = SessionMeter {
        state: state.clone(),
        scope,
        model: query.model,
        price_model: entry.route.model.clone(),
        provider: selected.provider.clone(),
        target: selected.target.clone(),
        variant: selected.variant.clone(),
        request_id,
        trace_id,
        key_digest: virtual_key
            .as_ref()
            .and_then(|_| crate::handlers::presented_key_digest(&snap, &headers)),
        flush_every: match snap.realtime.usage_flush_secs {
            0 => None,
            secs => Some(Duration::from_secs(secs)),
        },
    };

    // counted from here rather than from inside the callback, which only runs
    // once the handshake completes: a drain that starts in between still waits
    let tracked = state.realtime_sessions.tasks.token();
    ws.on_upgrade(move |socket| async move {
        let _tracked = tracked;
        relay(socket, selected, meter, policy).await;
    })
    .into_response()
}

struct SelectedSession {
    upstream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    // held until both sockets close so the selected target remains in the
    // load-balancer's in-flight view for the entire persistent session
    _load: crate::load::LoadGuard,
    _session: SessionGuard,
    state: AppState,
    provider: String,
    target: String,
    /// the variant the session was pinned to; empty on a route without
    /// variants
    variant: String,
}

/// One target a session may be pinned to, in the order it is tried.
struct Candidate<'a> {
    target: &'a rolter_core::Target,
    /// load-tracker key: the public model, or the variant key
    namespace: String,
    index: usize,
    variant: &'a str,
}

#[expect(
    clippy::too_many_arguments,
    reason = "realtime setup needs distinct borrowed routing and session inputs"
)]
async fn connect_selected(
    state: &AppState,
    snap: &Snapshot,
    entry: &crate::state::RouteEntry,
    model: &str,
    context: &RouteContext<'_>,
    headers: &HeaderMap,
    session_guard: SessionGuard,
    key_meta: Option<&crate::state::KeyMeta>,
) -> Result<SelectedSession, String> {
    let candidates = realtime_candidates(state, snap, entry, model, context, key_meta);
    let mut last_error = "no target selected".to_string();

    for candidate in candidates {
        let Candidate {
            target,
            namespace,
            index,
            variant,
        } = candidate;
        let Some(provider) = snap.providers.get(&target.provider) else {
            last_error = "configured target provider not found".to_string();
            continue;
        };
        let multi_key = provider.api_keys.len() > 1;
        let key_namespace = key_pool_key(&target.provider);
        let api_key = provider
            .pick_api_key_indexed(0.0, |i| {
                multi_key && state.cooldowns.is_parked(&key_namespace, i)
            })
            .map(|(_, key)| key);
        let upstream_model = target.model.as_deref().unwrap_or(model);
        let url = realtime_url(&provider.api_base, upstream_model);
        let request = realtime_request(&url, api_key.as_deref(), headers)?;
        let mut load = state.loads.begin(&namespace, index);
        match connect_async(request).await {
            Ok((upstream, _)) => {
                load.mark_ok();
                return Ok(SelectedSession {
                    upstream,
                    _load: load,
                    _session: session_guard,
                    state: state.clone(),
                    provider: target.provider.clone(),
                    target: upstream_model.to_string(),
                    variant: variant.to_string(),
                });
            }
            Err(error) => {
                last_error = format!("upstream realtime connection failed: {error}");
                if snap.cooldown.enabled() {
                    state
                        .cooldowns
                        .park(&namespace, index, snap.cooldown.duration_secs(None));
                }
                state.breaker.on_failure(&namespace, index);
            }
        }
    }
    Err(last_error)
}

/// Flatten routes into their strategy-led target order. Connections are tried
/// only during establishment; after a successful upgrade the session is pinned.
fn realtime_candidates<'a>(
    state: &AppState,
    snap: &Snapshot,
    entry: &'a crate::state::RouteEntry,
    model: &str,
    context: &RouteContext<'_>,
    key_meta: Option<&crate::state::KeyMeta>,
) -> Vec<Candidate<'a>> {
    if !entry.route.has_variants() {
        let mut loads = state.loads.snapshot(model, entry.route.targets.len());
        for (index, target) in entry.route.targets.iter().enumerate() {
            if let Some(load) = loads.get_mut(index) {
                *load = load.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
            }
        }
        let mut ordered = Vec::with_capacity(entry.route.targets.len());
        let mut tried = Vec::with_capacity(entry.route.targets.len());
        while let Some(index) = pick_untried(
            entry,
            context,
            &tried,
            &loads,
            &state.cooldowns,
            &state.health,
            &state.breaker,
            model,
            snap.cooldown.enabled(),
            key_meta,
        ) {
            entry.balancer.observe(index, context);
            tried.push(index);
            ordered.push(Candidate {
                target: &entry.route.targets[index],
                namespace: model.to_string(),
                index,
                variant: "",
            });
        }
        return ordered;
    }

    let primary = entry.route.sample_variant(0.0).unwrap_or(0);
    let mut ordered = Vec::new();
    for variant_index in entry.route.fallback_order(primary) {
        let Some(variant) = entry.route.variants.get(variant_index) else {
            continue;
        };
        let namespace = variant_key(model, &variant.name);
        let loads = state.loads.snapshot(&namespace, variant.targets.len());
        let lead = entry
            .variant_balancers
            .get(variant_index)
            .and_then(|balancer| balancer.pick(context, &loads))
            .filter(|index| *index < variant.targets.len());
        let indexes: Vec<_> = lead
            .into_iter()
            .chain((0..variant.targets.len()).filter(|i| Some(*i) != lead))
            .collect();
        let available: Vec<_> = indexes
            .iter()
            .copied()
            .filter(|&index| {
                let target = &variant.targets[index];
                key_meta.is_none_or(|key| key.provider_allowed(&target.provider))
                    && (!snap.cooldown.enabled() || !state.cooldowns.is_parked(&namespace, index))
                    && state.health.is_healthy(&target.provider)
                    && state.breaker.allows(&namespace, index)
            })
            .collect();
        // preserve the HTTP route's fail-open behaviour when all candidates
        // are temporarily unavailable
        let allowed: Vec<_> = indexes
            .iter()
            .copied()
            .filter(|&index| {
                key_meta.is_none_or(|key| key.provider_allowed(&variant.targets[index].provider))
            })
            .collect();
        for index in if available.is_empty() {
            &allowed
        } else {
            &available
        } {
            let index = *index;
            if let Some(balancer) = entry.variant_balancers.get(variant_index) {
                balancer.observe(index, context);
            }
            ordered.push(Candidate {
                target: &variant.targets[index],
                namespace: namespace.clone(),
                index,
                variant: &variant.name,
            });
        }
    }
    ordered
}

fn realtime_url(api_base: &str, model: &str) -> String {
    let base = api_base.trim_end_matches('/');
    let scheme = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{scheme}/v1/realtime?model={model}")
}

fn realtime_request(
    url: &str,
    api_key: Option<&str>,
    headers: &HeaderMap,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let mut request = url
        .into_client_request()
        .map_err(|error| error.to_string())?;
    if let Some(key) = api_key {
        let value = format!("Bearer {key}")
            .parse()
            .map_err(|error| format!("invalid upstream api key: {error}"))?;
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    // openai's Realtime beta required this header; forwarding it also keeps
    // compatibility with providers that still gate the endpoint this way
    if let Some(value) = headers.get("openai-beta") {
        request.headers_mut().insert("openai-beta", value.clone());
    }
    Ok(request)
}

async fn relay(
    socket: WebSocket,
    session: SelectedSession,
    meter: SessionMeter,
    mut policy: Option<ContentPolicy>,
) {
    let SelectedSession {
        upstream,
        _load,
        _session,
        state,
        provider,
        target,
        variant: _,
    } = session;
    let closing = state.realtime_sessions.closing.clone();
    let channels = meter.spawn();
    let meter = channels.handle;
    let mut exhausted = Some(channels.closure);
    // only a content policy reads the scope after the upgrade, so a session
    // without one has nothing to follow a re-scope with
    let mut rescoped = policy.is_some().then_some(channels.scope);
    let mut turns = TurnTracker::default();
    let (mut client_sender, mut client_receiver) = socket.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream.split();
    // what the session ended on, for the row of any response it cut short
    let mut end = SessionEnd::client_left();

    let (max_session, idle_timeout) = {
        let snap = state.snapshot.load();
        (
            snap.realtime.max_session_secs,
            snap.realtime.idle_timeout_secs,
        )
    };
    let session_deadline =
        (max_session != 0).then(|| tokio::time::Instant::now() + Duration::from_secs(max_session));
    let mut idle_deadline = (idle_timeout != 0)
        .then(|| tokio::time::Instant::now() + Duration::from_secs(idle_timeout));

    loop {
        let session_wait = async {
            if let Some(deadline) = session_deadline {
                tokio::time::sleep_until(deadline).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let idle_wait = async {
            if let Some(deadline) = idle_deadline {
                tokio::time::sleep_until(deadline).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let budget_wait = async {
            match exhausted.as_mut() {
                Some(verdict) => verdict.await.ok(),
                None => std::future::pending().await,
            }
        };
        let scope_wait = async {
            match rescoped.as_mut() {
                Some(scope) => scope.changed().await.is_ok(),
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = closing.cancelled() => {
                let _ = tokio::time::timeout(CLOSE_WRITE_TIMEOUT, client_sender.send(shutdown_close_frame())).await;
                let _ = tokio::time::timeout(CLOSE_WRITE_TIMEOUT, upstream_sender.send(UpstreamMessage::Close(None))).await;
                end = SessionEnd::shutting_down();
                break;
            },
            _ = session_wait => {
                end = SessionEnd::limit_reached("max_session_secs");
                break;
            },
            _ = idle_wait => {
                end = SessionEnd::limit_reached("idle_timeout_secs");
                break;
            },
            closure = budget_wait => match closure {
                Some(closure) => {
                    let (event, reason, ended) = match closure {
                        Closure::BudgetSpent(message) => (
                            budget_error_event(&message),
                            "budget exceeded",
                            SessionEnd::budget_spent(message),
                        ),
                        Closure::AccessRevoked(revoked) => (
                            error_event(revoked.status, revoked.code, &revoked.message),
                            "access revoked",
                            SessionEnd::access_revoked(&revoked),
                        ),
                    };
                    // the error event first, so the client learns why before
                    // the close frame arrives
                    let _ = client_sender.send(Message::Text(event.into())).await;
                    let _ = client_sender.send(policy_close_frame(reason)).await;
                    let _ = upstream_sender.send(UpstreamMessage::Close(None)).await;
                    end = ended;
                    break;
                }
                // the meter stopped without a verdict: keep relaying under the
                // session's other limits rather than polling a closed channel
                None => exhausted = None,
            },
            changed = scope_wait => match rescoped.as_mut() {
                Some(scope) if changed => {
                    if let Some(policy) = policy.as_mut() {
                        policy.rescope(&scope.borrow_and_update());
                    }
                }
                // the meter is gone, and with it any later re-scope
                _ => rescoped = None,
            },
            message = client_receiver.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, Message::Close(_));
                    let message = match (policy.as_mut(), message) {
                        (Some(policy), Message::Text(text)) => {
                            match policy.client_event(&state.metrics, text.as_str()).await {
                                ClientVerdict::Pass => Message::Text(text),
                                ClientVerdict::Replace(frame) => Message::Text(frame.into()),
                                ClientVerdict::Reject(event) => {
                                    if client_sender.send(Message::Text(event.into())).await.is_err() {
                                        break;
                                    }
                                    idle_deadline = (idle_timeout != 0).then(|| tokio::time::Instant::now() + Duration::from_secs(idle_timeout));
                                    continue;
                                }
                            }
                        }
                        // not part of the protocol, and an upstream that read
                        // JSON out of one would skip the checks above
                        (Some(_), Message::Binary(_)) => {
                            let event = error_event(
                                StatusCode::BAD_REQUEST,
                                "guardrail_blocked",
                                "binary frames are not accepted while content policy applies",
                            );
                            if client_sender.send(Message::Text(event.into())).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        (_, message) => message,
                    };
                    if upstream_sender.send(to_upstream(message)).await.is_err() {
                        end = SessionEnd::upstream_failed();
                        break;
                    }
                    idle_deadline = (idle_timeout != 0).then(|| tokio::time::Instant::now() + Duration::from_secs(idle_timeout));
                    if close { break; }
                }
                Some(Err(_)) | None => break,
            },
            message = upstream_receiver.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, UpstreamMessage::Close(_));
                    let turn = match &message {
                        UpstreamMessage::Text(text) => turns.observe(text.as_str()),
                        _ => None,
                    };
                    let mut message = message;
                    let mut deliver = true;
                    if let (Some(policy), UpstreamMessage::Text(text)) = (policy.as_mut(), &message) {
                        let outcome = policy.server_event(&state.metrics, text.as_str());
                        if let Some(notice) = outcome.notice {
                            let _ = client_sender.send(Message::Text(notice.into())).await;
                        }
                        if let Some(cancel) = outcome.cancel {
                            let _ = upstream_sender.send(UpstreamMessage::Text(cancel.into())).await;
                        }
                        match outcome.frame {
                            Frame::Pass => {}
                            Frame::Replace(frame) => message = UpstreamMessage::Text(frame.into()),
                            // withheld, but the turn it belongs to still counts
                            Frame::Drop => deliver = false,
                        }
                    }
                    let delivered = !deliver || client_sender.send(to_client(message)).await.is_ok();
                    // the upstream billed a finished turn whether or not the
                    // client read its last event
                    if let Some(turn) = turn {
                        meter.turn(turn);
                    }
                    if !delivered { break; }
                    idle_deadline = (idle_timeout != 0).then(|| tokio::time::Instant::now() + Duration::from_secs(idle_timeout));
                    if close {
                        end = SessionEnd::upstream_closed();
                        break;
                    }
                }
                Some(Err(_)) | None => {
                    end = SessionEnd::upstream_failed();
                    break;
                },
            }
        }
    }

    // each finished turn reports its own outcome through the request log, so
    // only a broken upstream leg is attributed to the target here
    if end.upstream_fault() {
        state.metrics.observe_target(&provider, &target, false);
    }
    drop(_load);
    drop(_session);
    meter.end(end, turns.into_open()).await;
}

/// The Realtime `error` event a session closed for its budget receives.
///
/// It is the shape the Realtime API uses for its own failures, so an SDK
/// surfaces it the way it surfaces the provider's errors, and its `error`
/// object is the HTTP refusal's, `insufficient_quota` code included.
fn budget_error_event(message: &str) -> String {
    error_event(StatusCode::PAYMENT_REQUIRED, "insufficient_quota", message)
}

/// A Realtime `error` event wrapping the HTTP refusal's own error object.
pub(crate) fn error_event(status: StatusCode, code: &'static str, message: &str) -> String {
    let mut event = crate::error::ApiError::new(status, message)
        .with_code(code)
        .body();
    event["type"] = "error".into();
    event["event_id"] = format!("event_rolter_{}", uuid::Uuid::new_v4().simple()).into();
    event.to_string()
}

/// A policy-violation close, which a client can branch on without parsing the
/// event before it.
fn policy_close_frame(reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code: close_code::POLICY,
        reason: Utf8Bytes::from_static(reason),
    }))
}

/// A going-away close, the code a WebSocket server sends when it shuts down, so
/// a client knows to reconnect rather than treat the session as refused.
fn shutdown_close_frame() -> Message {
    Message::Close(Some(CloseFrame {
        code: close_code::AWAY,
        reason: Utf8Bytes::from_static("gateway shutting down"),
    }))
}

fn to_upstream(message: Message) -> UpstreamMessage {
    match message {
        Message::Text(text) => UpstreamMessage::Text(text.to_string().into()),
        Message::Binary(bytes) => UpstreamMessage::Binary(bytes),
        Message::Ping(bytes) => UpstreamMessage::Ping(bytes),
        Message::Pong(bytes) => UpstreamMessage::Pong(bytes),
        Message::Close(_) => UpstreamMessage::Close(None),
    }
}

fn to_client(message: UpstreamMessage) -> Message {
    match message {
        UpstreamMessage::Text(text) => Message::Text(text.to_string().into()),
        UpstreamMessage::Binary(bytes) => Message::Binary(bytes),
        UpstreamMessage::Ping(bytes) => Message::Ping(bytes),
        UpstreamMessage::Pong(bytes) => Message::Pong(bytes),
        UpstreamMessage::Close(_) => Message::Close(None),
        UpstreamMessage::Frame(_) => Message::Close(None),
    }
}

/// Whether the PII sanitizer is enabled and fail-closed, the one setting under
/// which skipping it on realtime must refuse the session. The sanitizer config
/// is deployment-wide, so there is no per-tenant or per-route scope to consult.
fn sanitizer_blocks_realtime(config: &rolter_core::PiiSanitizerConfig) -> bool {
    config.enabled && config.failure_mode == rolter_core::FailureMode::FailClosed
}

fn api_error(status: StatusCode, message: &str) -> Response {
    crate::error::ApiError::new(status, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::{realtime_url, Sessions};
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::sync::Arc;
    use std::time::Duration;

    /// The drain is what keeps the runtime alive under a meter's last flush,
    /// so it must hold for as long as any meter is still running.
    #[tokio::test]
    async fn a_drain_waits_for_every_meter_to_finish() {
        let sessions = Sessions::default();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let flushed = Arc::new(AtomicBool::new(false));
        let done = flushed.clone();
        sessions.spawn_meter(async move {
            let _ = released.await;
            done.store(true, SeqCst);
        });
        sessions.close();
        assert!(
            !sessions.drained(Duration::from_millis(50)).await,
            "a meter still flushing holds the drain"
        );
        release.send(()).unwrap();
        assert!(sessions.drained(Duration::from_secs(5)).await);
        assert!(flushed.load(SeqCst));
    }

    /// A session is counted from admission, before its handshake completes,
    /// so a drain that starts in between still waits for it.
    #[tokio::test]
    async fn a_drain_waits_for_a_session_still_upgrading() {
        let sessions = Sessions::default();
        let upgrading = sessions.tasks.token();
        sessions.close();
        assert!(sessions.closing.is_cancelled(), "new sessions are refused");
        assert!(!sessions.drained(Duration::from_millis(50)).await);
        drop(upgrading);
        assert!(sessions.drained(Duration::from_secs(5)).await);
    }

    /// A `wss://` dial builds its rustls config from the crate features alone,
    /// and rustls panics there when two crypto providers are compiled in. The
    /// panic is invisible to the compiler and to every plain-http test, and
    /// reaches a user as a realtime session that never opens. The launcher
    /// links the gateway with the control plane's store, so this only sees a
    /// second provider in a workspace-wide build, which CI runs.
    #[tokio::test]
    async fn a_wss_dial_finds_its_crypto_provider() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // accepts the tcp connection and closes it, so the dial reaches the
        // point where the tls config is built and then fails the handshake
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        let dial = tokio_tungstenite::connect_async(format!("wss://localhost:{port}/"));
        let outcome = tokio::time::timeout(Duration::from_secs(10), dial)
            .await
            .expect("the dial ends");
        assert!(outcome.is_err(), "nothing answers the handshake");
    }

    #[test]
    fn converts_http_base_to_websocket_realtime_url() {
        assert_eq!(
            realtime_url("https://api.openai.com", "gpt-realtime"),
            "wss://api.openai.com/v1/realtime?model=gpt-realtime"
        );
        assert_eq!(
            realtime_url("http://localhost:8080/", "m"),
            "ws://localhost:8080/v1/realtime?model=m"
        );
    }
}
