//! Reload-free config watcher.
//!
//! Polls the control plane's `GET /internal/snapshot?version=N` on an
//! interval and hot-swaps the gateway's routing [`Snapshot`] via
//! [`AppState::reload`] whenever a newer config version is published. The
//! control plane replies `304 Not Modified` when the gateway is already
//! current, so steady-state polling is cheap.
//!
//! This is the polling transport; a Redis pub/sub wake-up (ROL-27) can later
//! trigger an immediate poll on top of the same apply path.

use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::Deserialize;

use rolter_core::GatewayConfig;

use crate::state::AppState;

/// A `{"version": N, "config": {...}}` document from the snapshot endpoint.
#[derive(Deserialize)]
struct SnapshotResponse {
    version: u64,
    config: GatewayConfig,
}

/// Spawn the background watcher. `snapshot_url` is the control plane's
/// snapshot endpoint (e.g. `http://control:4001/internal/snapshot`); `period`
/// is the poll interval. Returns immediately; the task runs until the process
/// exits.
pub fn spawn(state: AppState, snapshot_url: String, period: Duration) {
    tokio::spawn(async move {
        // a dedicated short-timeout client so a hung control plane can't wedge
        // the watcher loop
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        run(&client, &state, &snapshot_url, period).await;
    });
}

/// The poll loop, factored out so tests can drive a single tick.
async fn run(client: &Client, state: &AppState, snapshot_url: &str, period: Duration) {
    let mut ticker = tokio::time::interval(period);
    // the initial config was applied at startup; treat it as version 0 so the
    // first successful poll always applies the authoritative version
    loop {
        ticker.tick().await;
        if let Err(err) = poll_once(client, state, snapshot_url).await {
            state
                .metrics
                .config_reload_failures_total
                .fetch_add(1, Relaxed);
            tracing::warn!(error = %err, "config snapshot poll failed");
        }
    }
}

/// Fetch the snapshot once and apply it if newer. Returns `Ok(Some(version))`
/// when a reload happened, `Ok(None)` on `304`/no-change, `Err` on transport
/// or decode failure.
async fn poll_once(
    client: &Client,
    state: &AppState,
    snapshot_url: &str,
) -> anyhow::Result<Option<u64>> {
    let current = state.metrics.config_version.load(Relaxed);
    let resp = client
        .get(snapshot_url)
        .query(&[("version", current.to_string())])
        .send()
        .await?;

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
    Ok(Some(body.version))
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
        let applied = poll_once(&client, &state, &url).await.unwrap();
        assert_eq!(applied, Some(5));
        assert_eq!(state.metrics.config_version.load(Relaxed), 5);
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);

        // second poll is a no-op (control replies 304)
        let applied = poll_once(&client, &state, &url).await.unwrap();
        assert_eq!(applied, None);
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 1);
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

        let res = poll_once(&client, &state, &url).await;
        assert!(res.is_err(), "invalid snapshot must be an error");
        // old config stays: no reload recorded, version unchanged
        assert_eq!(state.metrics.config_reloads_total.load(Relaxed), 0);
        assert_eq!(state.metrics.config_version.load(Relaxed), 0);
    }

    #[tokio::test]
    async fn counts_failure_on_unreachable_control() {
        let state = AppState::new(&GatewayConfig::default());
        let client = Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        // nothing is listening here
        let err = poll_once(&client, &state, "http://127.0.0.1:1/internal/snapshot").await;
        assert!(err.is_err());
    }
}
