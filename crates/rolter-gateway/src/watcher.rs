//! Reload-free config watcher.
//!
//! Polls the control plane's `GET /internal/snapshot?version=N` on an
//! interval and hot-swaps the gateway's routing [`Snapshot`] via
//! [`AppState::reload`] whenever a newer config version is published. The
//! control plane replies `304 Not Modified` when the gateway is already
//! current, so steady-state polling is cheap.
//!
//! Polling is the reliable baseline transport. When a redis url is
//! configured, a subscriber on [`rolter_core::CONFIG_CHANNEL`] additionally
//! wakes the loop the moment the control plane publishes a version bump, so
//! changes propagate immediately instead of waiting out the poll interval.
//! Redis being down never breaks config propagation — the subscriber
//! reconnects with backoff while interval polling keeps working.
//!
//! Which of the two is in effect is logged at startup. Without a redis url the
//! gateway is polling-only, and a write can take up to the poll interval to
//! take effect: a virtual key minted in the dashboard is rejected until then,
//! which is indistinguishable from a wrong key (#933). That is a supported way
//! to run, but it is stated rather than left to be inferred from a key that
//! "does not work yet".

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use tokio::sync::Notify;

use rolter_core::GatewayConfig;

use crate::state::AppState;

/// A `{"version": N, "config": {...}}` document from the snapshot endpoint.
#[derive(Deserialize)]
struct SnapshotResponse {
    version: u64,
    config: GatewayConfig,
    /// entries the control plane dropped because they were unservable on their
    /// own, one line each saying which and why (#926). Absent when everything
    /// was served, and absent entirely from an older control plane, so it
    /// defaults rather than failing the decode
    #[serde(default)]
    problems: Vec<String>,
}

/// Ask the control plane where the fleet logs, before the log writer is built.
///
/// The ClickHouse writer is spawned once, from the bootstrap config, at
/// startup — the hot-swappable [`Snapshot`] carries routing, not background
/// tasks. So a destination that only arrives on the first *poll* arrives too
/// late to open a sink. This is the one read that happens early enough to
/// matter (#929).
///
/// Best-effort by construction: a control plane that is not up yet, is slow, or
/// answers with something unparsable yields `None` and the gateway starts
/// exactly as it did before. Startup must not depend on the control plane
/// being reachable, so every failure here is a debug line, not an error.
pub async fn fetch_log_destination(
    snapshot_url: &str,
    admin_token: Option<&str>,
    timeout: Duration,
) -> Option<String> {
    let client = Client::builder().timeout(timeout).build().ok()?;
    // version=0 asks for the current snapshot rather than a 304
    let mut request = client.get(snapshot_url).query(&[("version", "0")]);
    if let Some(token) = admin_token {
        request = request.bearer_auth(token);
    }
    let resp = match request.send().await {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "no log destination from the control plane");
            return None;
        }
        Err(err) => {
            tracing::debug!(error = %err, "no log destination from the control plane");
            return None;
        }
    };
    let body: SnapshotResponse = match resp.json().await {
        Ok(body) => body,
        Err(err) => {
            tracing::debug!(error = %err, "unparsable snapshot while reading log destination");
            return None;
        }
    };
    body.config
        .logging
        .clickhouse_url
        .filter(|url| !url.trim().is_empty())
}

/// Spawn the background watcher. `snapshot_url` is the control plane's
/// snapshot endpoint (e.g. `http://control:4001/internal/snapshot`); `period`
/// is the poll interval; `redis_url`, when set, adds an instant pub/sub
/// wake-up on top of the interval. Returns immediately; the tasks run until
/// the process exits.
pub fn spawn(
    state: AppState,
    snapshot_url: String,
    period: Duration,
    redis_url: Option<String>,
    admin_token: Option<String>,
) {
    let wakeup = Arc::new(Notify::new());
    // which transport is in effect decides whether a config change lands on the
    // next request or up to `period` later, and that difference is what an
    // operator experiences as a brand-new virtual key being rejected as if it
    // were a wrong one (#933). Polling-only is a supported deployment, but it
    // has to be stated rather than inferred from a key that "does not work yet"
    match redis_url {
        Some(url) => {
            tracing::info!(
                poll_interval_secs = period.as_secs(),
                "config propagation: redis pub/sub + interval polling (changes apply immediately)"
            );
            tokio::spawn(subscribe(url, Arc::clone(&wakeup)));
        }
        None => tracing::warn!(
            poll_interval_secs = period.as_secs(),
            "config propagation: interval polling only — no redis url configured, so a new \
             virtual key, provider or route can take up to the poll interval to take effect \
             and will be rejected until then"
        ),
    }
    tokio::spawn(async move {
        // a dedicated short-timeout client so a hung control plane can't wedge
        // the watcher loop
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        run(
            &client,
            &state,
            &snapshot_url,
            admin_token.as_deref(),
            period,
            &wakeup,
        )
        .await;
    });
}

/// The poll loop, factored out so tests can drive a single tick. Polls on
/// the interval and immediately whenever the redis subscriber signals.
async fn run(
    client: &Client,
    state: &AppState,
    snapshot_url: &str,
    admin_token: Option<&str>,
    period: Duration,
    wakeup: &Notify,
) {
    let mut ticker = tokio::time::interval(period);
    // the initial config was applied at startup; treat it as version 0 so the
    // first successful poll always applies the authoritative version
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = wakeup.notified() => {}
        }
        if let Err(err) = poll_once(client, state, snapshot_url, admin_token).await {
            state
                .metrics
                .config_reload_failures_total
                .fetch_add(1, Relaxed);
            tracing::warn!(error = %err, "config snapshot poll failed");
        }
    }
}

/// Subscribe to [`rolter_core::CONFIG_CHANNEL`] and signal `wakeup` on every
/// message. Reconnects with a fixed backoff forever; interval polling covers
/// propagation while redis is unavailable.
async fn subscribe(redis_url: String, wakeup: Arc<Notify>) {
    const BACKOFF: Duration = Duration::from_secs(5);
    loop {
        let session = async {
            let client = redis::Client::open(redis_url.as_str())?;
            let mut pubsub = client.get_async_pubsub().await?;
            pubsub.subscribe(rolter_core::CONFIG_CHANNEL).await?;
            tracing::info!(
                channel = rolter_core::CONFIG_CHANNEL,
                "config pub/sub connected"
            );
            let mut stream = pubsub.on_message();
            while stream.next().await.is_some() {
                wakeup.notify_one();
            }
            Ok::<_, redis::RedisError>(())
        };
        if let Err(err) = session.await {
            tracing::warn!(error = %err, "config pub/sub disconnected; retrying");
        }
        tokio::time::sleep(BACKOFF).await;
    }
}

/// Header names this node identifies itself with on each poll, so the control
/// plane can keep a cluster inventory without a second channel (#543).
const NODE_ID_HEADER: &str = "x-rolter-node-id";
const NODE_ROLE_HEADER: &str = "x-rolter-node-role";
const NODE_BUILD_HEADER: &str = "x-rolter-node-build";
/// Response header carrying the operator-requested state for this node.
const NODE_STATE_HEADER: &str = "x-rolter-node-state";

/// Stable identity for this gateway process, resolved by `rolter_core` so the
/// cluster inventory, the adaptive-routing telemetry and `service.instance.id`
/// all name this node the same way. An unidentified node polls exactly as
/// before and stays out of the inventory rather than churning it with a
/// per-restart id.
pub(crate) fn node_id() -> Option<String> {
    rolter_core::node_identity::node_id()
}

/// Say once, at boot, which node this is — or that it is nobody.
///
/// Both the cluster inventory and the adaptive-routing scoreboard are keyed on
/// the node header, and the control plane cannot record a report that does not
/// carry one. Before #1644 that produced two permanently empty dashboard
/// screens and not one line of log on either plane, so the absence is now
/// stated at the only moment an operator can act on it.
pub(crate) fn log_node_identity() {
    match rolter_core::node_identity::node_identity() {
        Some(identity) => tracing::info!(
            node_id = %identity.id,
            source = identity.source.as_str(),
            "node identity resolved"
        ),
        None => tracing::warn!(
            "no node identity could be resolved (ROLTER_NODE_ID, HOSTNAME and the \
             hostname syscall all came up empty); this gateway will stay out of the \
             cluster inventory and report no adaptive-routing telemetry until \
             ROLTER_NODE_ID is set"
        ),
    }
}

/// Fetch the snapshot once and apply it if newer. Returns `Ok(Some(version))`
/// when a reload happened, `Ok(None)` on `304`/no-change, `Err` on transport
/// or decode failure.
async fn poll_once(
    client: &Client,
    state: &AppState,
    snapshot_url: &str,
    admin_token: Option<&str>,
) -> anyhow::Result<Option<u64>> {
    let current = state.metrics.config_version.load(Relaxed);
    let mut request = client
        .get(snapshot_url)
        .query(&[("version", current.to_string())]);
    if let Some(token) = admin_token {
        request = request.bearer_auth(token);
    }
    if let Some(id) = node_id() {
        request = request
            .header(NODE_ID_HEADER, id)
            .header(NODE_ROLE_HEADER, "gateway")
            .header(NODE_BUILD_HEADER, env!("CARGO_PKG_VERSION"));
    }
    let resp = request.send().await?;

    // the control plane answers every poll with the node's requested state, so
    // a drain lands even when the config itself has not changed
    apply_node_state(state, resp.headers());

    if resp.status() == StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !resp.status().is_success() {
        anyhow::bail!("snapshot endpoint returned {}", resp.status());
    }

    let body: SnapshotResponse = resp.json().await?;
    // guard against a stale/racy response older than what we already run
    if body.version <= current && current != 0 {
        return Ok(None);
    }
    // never apply a broken snapshot; keep serving the last good config
    if let Err(problems) = body.config.validate() {
        anyhow::bail!("snapshot v{} failed validation: {problems:?}", body.version);
    }
    state.reload(&body.config, body.version);
    tracing::info!(version = body.version, "applied new config snapshot");
    // logged here rather than on every poll: this line is only reached when the
    // version actually moved, so a fleet polling every 5s does not repeat the
    // same complaint 17k times a day (#926)
    if !body.problems.is_empty() {
        tracing::warn!(
            version = body.version,
            count = body.problems.len(),
            problems = ?body.problems,
            "control plane omitted unservable entries from this config; \
             requests to them will error"
        );
    }
    Ok(Some(body.version))
}

/// Apply the operator-requested node state carried on the snapshot response.
/// A response without the header leaves the current state alone, so an
/// unmanaged or unidentified node never flips itself out of service.
fn apply_node_state(state: &AppState, headers: &reqwest::header::HeaderMap) {
    let Some(requested) = headers.get(NODE_STATE_HEADER).and_then(|v| v.to_str().ok()) else {
        return;
    };
    let draining = requested.eq_ignore_ascii_case("draining");
    let previous = state.draining.swap(draining, Relaxed);
    if previous != draining {
        tracing::info!(draining, "control plane changed this node's serving state");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn applies_newer_version_and_skips_same() {
        // spin a tiny control-plane stub that serves an incrementing version
        use axum::extract::Query;
        use axum::routing::get;
        use axum::{Json, Router};
        use std::collections::HashMap;

        async fn snapshot(Query(q): Query<HashMap<String, String>>) -> axum::response::Response {
            use axum::response::IntoResponse;
            let seen: u64 = q.get("version").and_then(|v| v.parse().ok()).unwrap_or(0);
            // authoritative version is 5; reply 304 once the caller has it
            if seen >= 5 {
                return axum::http::StatusCode::NOT_MODIFIED.into_response();
            }
            Json(serde_json::json!({
                "version": 5,
                "config": {"server": {"host": "0.0.0.0", "port": 4000}}
            }))
            .into_response()
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        // first poll applies version 5
        let applied = poll_once(&client, &state, &url, None).await.unwrap();
        assert_eq!(applied, Some(5));
        assert_eq!(state.metrics.config_version.load(Relaxed), 5);
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);

        // second poll is a no-op (control replies 304)
        let applied = poll_once(&client, &state, &url, None).await.unwrap();
        assert_eq!(applied, None);
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);
    }

    #[tokio::test]
    async fn drain_state_from_the_snapshot_response_flips_readiness() {
        use reqwest::header::{HeaderMap, HeaderValue};

        let state = AppState::new(&GatewayConfig::default());
        assert!(!state.draining.load(Relaxed));

        // a response with no header leaves the node serving
        apply_node_state(&state, &HeaderMap::new());
        assert!(!state.draining.load(Relaxed));

        let mut draining = HeaderMap::new();
        draining.insert(NODE_STATE_HEADER, HeaderValue::from_static("draining"));
        apply_node_state(&state, &draining);
        assert!(state.draining.load(Relaxed));

        // an unchanged poll must not silently return the node to service
        apply_node_state(&state, &HeaderMap::new());
        assert!(state.draining.load(Relaxed));

        let mut active = HeaderMap::new();
        active.insert(NODE_STATE_HEADER, HeaderValue::from_static("active"));
        apply_node_state(&state, &active);
        assert!(!state.draining.load(Relaxed));
    }

    #[tokio::test]
    async fn rejects_invalid_snapshot_and_keeps_old_config() {
        use axum::routing::get;
        use axum::{Json, Router};

        // route targets a provider that doesn't exist -> must be rejected
        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 9,
                "config": {
                    "routes": [{"model": "broken", "targets": [{"provider": "ghost"}]}]
                }
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        let res = poll_once(&client, &state, &url, None).await;
        assert!(res.is_err(), "invalid snapshot must be an error");
        // old config stays: no reload recorded, version unchanged
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 0);
        assert_eq!(state.metrics.config_version.load(Relaxed), 0);
    }

    /// #926: a snapshot that carries `problems` is a *partial* config, not a
    /// broken one. It must apply — the alternative is the gateway refusing the
    /// same fleet config the control plane just decided to serve.
    #[tokio::test]
    async fn a_snapshot_carrying_problems_still_applies() {
        use axum::routing::get;
        use axum::{Json, Router};

        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 12,
                "config": {
                    "providers": [{
                        "name": "openai",
                        "kind": "openai",
                        "api_base": "https://api.openai.com/v1",
                    }],
                    "routes": [{"model": "gpt-4o", "targets": [{"provider": "openai"}]}],
                },
                "problems": [
                    "provider 'openrouter-edge' omitted from the snapshot: openrouter \
                     provider 'openrouter-edge' api_base must be https://openrouter.ai/api/v1"
                ],
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        let applied = poll_once(&client, &state, &url, None).await.unwrap();
        assert_eq!(applied, Some(12));
        assert_eq!(state.metrics.config_version.load(Relaxed), 12);

        // and a second poll at the same version is a no-op, which is what keeps
        // the warning to once per change rather than once per poll
        assert_eq!(poll_once(&client, &state, &url, None).await.unwrap(), None);
    }

    /// A control plane that has not been upgraded yet sends no `problems` key
    /// at all; the field must default rather than fail the decode and take
    /// config propagation down during a rollout.
    #[tokio::test]
    async fn a_snapshot_without_problems_still_decodes() {
        use axum::routing::get;
        use axum::{Json, Router};

        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 3,
                "config": {
                    "providers": [{
                        "name": "openai",
                        "kind": "openai",
                        "api_base": "https://api.openai.com/v1",
                    }],
                    "routes": [{"model": "gpt-4o", "targets": [{"provider": "openai"}]}],
                },
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = AppState::new(&GatewayConfig::default());
        let client = Client::new();
        let url = format!("http://{addr}/internal/snapshot");
        assert_eq!(
            poll_once(&client, &state, &url, None).await.unwrap(),
            Some(3)
        );
    }

    #[tokio::test]
    async fn counts_failure_on_unreachable_control() {
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        // nothing is listening here
        let err = poll_once(
            &client,
            &state,
            "http://127.0.0.1:1/internal/snapshot",
            None,
        )
        .await;
        assert!(err.is_err());
    }

    /// #929: the log writer is spawned once at startup from the bootstrap
    /// config, so a destination that only arrives on the first poll arrives too
    /// late to open a sink. This read is the one that happens early enough.
    #[tokio::test]
    async fn the_control_planes_log_destination_is_readable_before_startup() {
        use axum::routing::get;
        use axum::{Json, Router};

        async fn snapshot() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 7,
                "config": {
                    "server": {"host": "0.0.0.0", "port": 4000},
                    "logging": {"clickhouse_url": "http://clickhouse:8123"}
                }
            }))
        }

        let app = Router::new().route("/internal/snapshot", get(snapshot));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let url = format!("http://{addr}/internal/snapshot");
        let found = fetch_log_destination(&url, None, Duration::from_secs(2)).await;
        assert_eq!(found.as_deref(), Some("http://clickhouse:8123"));
    }

    #[tokio::test]
    async fn a_control_plane_that_names_no_destination_yields_none() {
        use axum::routing::get;
        use axum::{Json, Router};

        // both shapes a real control plane produces: the field absent, and the
        // field present but empty. Neither may open a sink pointed at nothing
        async fn absent() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 1,
                "config": {"server": {"host": "0.0.0.0", "port": 4000}}
            }))
        }
        async fn blank() -> Json<serde_json::Value> {
            Json(serde_json::json!({
                "version": 1,
                "config": {
                    "server": {"host": "0.0.0.0", "port": 4000},
                    "logging": {"clickhouse_url": "   "}
                }
            }))
        }

        let app = Router::new()
            .route("/absent", get(absent))
            .route("/blank", get(blank));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        for path in ["absent", "blank"] {
            let url = format!("http://{addr}/{path}");
            assert_eq!(
                fetch_log_destination(&url, None, Duration::from_secs(2)).await,
                None,
                "{path}"
            );
        }
    }

    /// startup must not depend on the control plane being up: an unreachable
    /// one costs one short timeout and the gateway starts as it always did
    #[tokio::test]
    async fn an_unreachable_control_plane_costs_a_timeout_and_nothing_else() {
        let found = fetch_log_destination(
            "http://127.0.0.1:1/internal/snapshot",
            None,
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(found, None);
    }
}
