//! Provider status-page polling — a slow, secondary health signal (ROL-200).
//!
//! Providers with a `status_page_url` (statuspage.io-style `status.json`) are
//! polled on a slow cadence. A non-`operational` indicator is recorded as a
//! `status_page` health event so it shows up in the dashboard and metrics ahead
//! of rolter's own probes failing. It is **secondary only**: it never marks a
//! provider unhealthy or affects routing — it does not touch [`crate::health`].
//!
//! Failures degrade gracefully: an unreachable page, a non-2xx, or invalid
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
            poll_one(&state, &provider, &url).await;
        }
    }
}

/// Poll a single provider's status page and, on a clean parse, emit one
/// `status_page` event. Any transport/parse failure is logged and skipped.
async fn poll_one(state: &crate::state::AppState, provider: &str, url: &str) {
    let started = std::time::Instant::now();
    // the url is operator-written, so it goes through the egress policy at
    // request time as well as at save, and never follows a redirect
    let Some(request) = state.side_client.get(url) else {
        tracing::warn!(%provider, "status-page url denied by the egress policy; skipping");
        return;
    };
    let text = match request.timeout(Duration::from_secs(10)).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(t) => t,
            Err(err) => {
                let error = rolter_core::redact::redact_urls_in_text(&err.to_string());
                tracing::warn!(%provider, %error, "status-page body read failed; skipping");
                return;
            }
        },
        Ok(resp) => {
            tracing::warn!(%provider, status = %resp.status(), "status-page returned non-2xx; skipping");
            return;
        }
        Err(err) => {
            let error = rolter_core::redact::redact_urls_in_text(&err.to_string());
            tracing::warn!(%provider, %error, "status-page request failed; skipping");
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
        // the poll's own instant, not the batch flush time
        ts: chrono::Utc::now(),
        target_id: provider.to_string(),
        provider: provider.to_string(),
        org_id: String::new(),
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

    #[tokio::test]
    async fn status_page_error_redacts_credentials_in_urls() {
        use parking_lot::Mutex;
        use std::sync::Arc;
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<String>>>);

        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                struct Fields(String);
                impl tracing::field::Visit for Fields {
                    fn record_debug(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &dyn std::fmt::Debug,
                    ) {
                        self.0.push_str(&format!("{}={value:?} ", field.name()));
                    }
                }
                let mut fields = Fields(String::new());
                event.record(&mut fields);
                self.0.lock().push(fields.0);
            }
        }

        let param_val = format!("val-{}", uuid::Uuid::new_v4().simple());
        let mut config = GatewayConfig::default();
        config.providers.push(rolter_core::ProviderConfig {
            name: "provider-with-secret-status-url".to_string(),
            status_page_url: Some(format!("https://127.0.0.1:1/status?token={param_val}")),
            ..Default::default()
        });
        let state = crate::state::AppState::new(&config);

        let provider = &config.providers[0].name;
        let url = config.providers[0].status_page_url.as_ref().unwrap();

        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());

        let _guard = tracing::subscriber::set_default(subscriber);
        poll_one(&state, provider, url).await;
        drop(_guard);

        let logs = captured.0.lock().join("\n");
        assert!(
            !logs.contains(&param_val),
            "secret value was leaked in tracing logs: {logs}"
        );
        assert!(
            logs.contains("token=***"),
            "token parameter was not redacted in logs: {logs}"
        );
    }
}
