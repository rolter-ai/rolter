//! Provider status-page polling — a slow, secondary health signal (ROL-200).
//!
//! Providers with a `status_page_url` (statuspage.io-style `status.json`) are
//! polled on a slow cadence. A non-`operational` indicator is recorded as a
//! `status_page` health event so it shows up in the dashboard and metrics ahead
//! of rolter's own probes failing. It is **secondary only**: it never marks a
//! provider unhealthy or affects routing — it does not touch [`crate::health`].
//!
//! Failures degrade gracefully: an unreachable page, a non-2xx, or unparseable
//! JSON is logged at `warn` and the signal is skipped for that cycle, never
//! recorded as a provider error.

use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use rolter_core::GatewayConfig;
use serde::Deserialize;

use crate::health_events::{HealthEvent, HealthOutcome, HealthSource};

/// The subset of a statuspage.io v2 `status.json` we read. The top-level
/// `status.indicator` is `none` when all systems are operational, and one of
/// `minor`/`major`/`critical` during an incident.
#[derive(Debug, Deserialize)]
struct StatusJson {
    status: StatusIndicator,
}

#[derive(Debug, Deserialize)]
struct StatusIndicator {
    #[serde(default)]
    indicator: String,
}

/// Classify a parsed status payload: `operational` when the indicator is empty
/// or `none`, otherwise degraded with the indicator as the reason.
fn classify(status: &StatusJson) -> Option<String> {
    let ind = status.status.indicator.trim().to_lowercase();
    if ind.is_empty() || ind == "none" {
        None
    } else {
        Some(ind)
    }
}

/// Spawn the status-page poller when any provider has a `status_page_url`.
/// Returns without spawning otherwise, so the common case costs nothing.
pub fn spawn_poller(config: &GatewayConfig, state: crate::state::AppState) {
    let any = config.providers.iter().any(|p| p.status_page_url.is_some());
    if !any {
        return;
    }
    let interval = config.health.status_page_interval_secs.max(1);
    tokio::spawn(async move {
        run_poller(interval, state).await;
    });
}

async fn run_poller(interval_secs: u64, state: crate::state::AppState) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        // read the poll targets off the current snapshot each cycle so config
        // hot-reloads are picked up without restarting the poller
        let targets: Vec<(String, String)> = {
            let snap = state.snapshot.load();
            snap.providers
                .values()
                .filter_map(|p| p.status_page_url.clone().map(|u| (p.name.clone(), u)))
                .collect()
        };
        for (provider, url) in targets {
            poll_one(&client, &state, &provider, &url).await;
        }
    }
}

/// Poll a single provider's status page and, on a clean parse, emit one
/// `status_page` event. Any transport/parse failure is logged and skipped.
async fn poll_one(
    client: &reqwest::Client,
    state: &crate::state::AppState,
    provider: &str,
    url: &str,
) {
    let started = std::time::Instant::now();
    let text = match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(%provider, %err, "status-page body read failed; skipping");
                return;
            }
        },
        Ok(resp) => {
            tracing::warn!(%provider, status = %resp.status(), "status-page returned non-2xx; skipping");
            return;
        }
        Err(err) => {
            tracing::warn!(%provider, %err, "status-page request failed; skipping");
            return;
        }
    };
    let parsed: StatusJson = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(%provider, %err, "status-page json parse failed; skipping");
            return;
        }
    };
    let latency_ms = started.elapsed().as_millis() as u32;
    let degraded = classify(&parsed);
    if degraded.is_some() {
        state
            .metrics
            .status_page_degraded_total
            .fetch_add(1, Relaxed);
    }
    state.health_events.emit(HealthEvent {
        target_id: provider.to_string(),
        provider: provider.to_string(),
        source: HealthSource::StatusPage,
        outcome: if degraded.is_some() {
            HealthOutcome::Error
        } else {
            HealthOutcome::Ok
        },
        status_code: None,
        latency_ms,
        error_kind: degraded,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> StatusJson {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn operational_indicator_is_not_degraded() {
        let s = parse(r#"{"status":{"indicator":"none","description":"All Systems Operational"}}"#);
        assert_eq!(classify(&s), None);
    }

    #[test]
    fn empty_indicator_is_not_degraded() {
        let s = parse(r#"{"status":{"description":"ok"}}"#);
        assert_eq!(classify(&s), None);
    }

    #[test]
    fn incident_indicator_is_degraded() {
        let s = parse(r#"{"status":{"indicator":"major","description":"Partial Outage"}}"#);
        assert_eq!(classify(&s), Some("major".to_string()));
        let s = parse(r#"{"status":{"indicator":"Critical"}}"#);
        assert_eq!(classify(&s), Some("critical".to_string()));
    }
}
