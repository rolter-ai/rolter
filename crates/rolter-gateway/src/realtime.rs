//! Persistent WebSocket relay for OpenAI-compatible Realtime sessions.
//!
//! A Realtime connection is intentionally selected once, before the downstream
//! HTTP upgrade is accepted. The selected provider/key is then pinned for the
//! lifetime of the socket: reconnecting is a client operation, never an
//! invisible mid-session failover that could duplicate audio or tool events.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
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

use crate::handlers::{authenticate, key_pool_key, pick_untried, variant_key};
use crate::state::{AppState, Snapshot};

/// Process-local admission counter for persistent sessions.
#[derive(Clone, Default)]
pub(crate) struct Sessions(Arc<AtomicU64>);

impl Sessions {
    fn acquire(&self, limit: u64) -> Option<SessionGuard> {
        loop {
            let current = self.0.load(Relaxed);
            if limit != 0 && current >= limit {
                return None;
            }
            if self
                .0
                .compare_exchange_weak(current, current + 1, Relaxed, Relaxed)
                .is_ok()
            {
                return Some(SessionGuard(self.clone()));
            }
        }
    }
}

struct SessionGuard(Sessions);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Relaxed);
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
    let virtual_key = match authenticate(&state, &snap, &headers) {
        Ok(key) => key,
        Err(response) => return response,
    };
    if let Some(key) = &virtual_key {
        if !rolter_auth::model_allowed(&key.models, &query.model) {
            return api_error(StatusCode::FORBIDDEN, "model not allowed for this key");
        }
    }

    let entry = match snap.routes.get(&query.model) {
        Some(entry) => entry,
        None => return api_error(StatusCode::NOT_FOUND, "no route for requested model"),
    };
    if entry.route.targets.is_empty() && !entry.route.has_variants() {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "route has no targets");
    }
    if let Some(key) = &virtual_key {
        if !super::handlers::key_allows_route(key, entry) {
            return api_error(
                StatusCode::FORBIDDEN,
                "no provider on this route is allowed for this key",
            );
        }
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

    // realtime has no request body, so session affinity uses the caller-supplied
    // session id and strategy-aware balancing sees an empty prompt
    let session_key = headers.get("x-session-id").and_then(|v| v.to_str().ok());
    let context = RouteContext {
        session_key,
        prompt: None,
        token_ids: None,
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
        Err(message) => return api_error(StatusCode::BAD_GATEWAY, &message),
    };

    ws.on_upgrade(move |socket| relay(socket, selected))
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
    model: String,
    provider: String,
    target: String,
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
        let (target, namespace, index) = candidate;
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
                    model: model.to_string(),
                    provider: target.provider.clone(),
                    target: upstream_model.to_string(),
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
) -> Vec<(&'a rolter_core::Target, String, usize)> {
    if !entry.route.has_variants() {
        let mut loads = state.loads.snapshot(model, entry.route.targets.len());
        for (index, target) in entry.route.targets.iter().enumerate() {
            if let Some(load) = loads.get_mut(index) {
                *load = load.saturating_add(state.upstream_metrics.queue_depth(&target.provider));
            }
        }
        let mut ordered = Vec::new();
        let mut tried = Vec::new();
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
            ordered.push((&entry.route.targets[index], model.to_string(), index));
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
            ordered.push((&variant.targets[index], namespace.clone(), index));
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

async fn relay(socket: WebSocket, session: SelectedSession) {
    let started = Instant::now();
    let SelectedSession {
        upstream,
        _load,
        _session,
        state,
        model,
        provider,
        target,
    } = session;
    let (mut client_sender, mut client_receiver) = socket.split();
    let (mut upstream_sender, mut upstream_receiver) = upstream.split();
    let mut ok = true;

    let max_session = state.snapshot.load().realtime.max_session_secs;
    let idle_timeout = state.snapshot.load().realtime.idle_timeout_secs;
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
        tokio::select! {
            _ = session_wait => { break; },
            _ = idle_wait => { break; },
            message = client_receiver.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, Message::Close(_));
                    if upstream_sender.send(to_upstream(message)).await.is_err() { ok = false; break; }
                    idle_deadline = (idle_timeout != 0).then(|| tokio::time::Instant::now() + Duration::from_secs(idle_timeout));
                    if close { break; }
                }
                Some(Err(_)) | None => break,
            },
            message = upstream_receiver.next() => match message {
                Some(Ok(message)) => {
                    let close = matches!(message, UpstreamMessage::Close(_));
                    if client_sender.send(to_client(message)).await.is_err() { ok = false; break; }
                    idle_deadline = (idle_timeout != 0).then(|| tokio::time::Instant::now() + Duration::from_secs(idle_timeout));
                    if close { break; }
                }
                Some(Err(_)) | None => { ok = false; break; },
            }
        }
    }

    state.metrics.observe_target(&provider, &target, ok);
    state
        .metrics
        .observe_request(&model, started.elapsed().as_millis() as u32, 0);
    drop(_load);
    drop(_session);
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

fn api_error(status: StatusCode, message: &str) -> Response {
    crate::error::ApiError::new(status, message).into_response()
}

#[cfg(test)]
mod tests {
    use super::realtime_url;

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
