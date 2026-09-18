//! Adaptive-routing telemetry reporting (#751).
//!
//! The `adaptive` strategy keeps its per-target scores in the balancer, which
//! lives and dies with a config snapshot inside this process. The dashboard
//! needs to read them, and the dashboard only ever talks to the control plane.
//!
//! So this module samples the live balancers on a timer and pushes the result
//! over the channel that already exists between the planes: the same internal
//! token, the same node identity headers as the snapshot poll (#543). Nothing
//! here runs on a request — the sample is taken by a background task, and the
//! request path pays only the two relaxed atomics
//! `rolter_balancer::adaptive::Adaptive::observe` already does.
//!
//! Reporting is best effort. A control plane that is down, old, or simply not
//! configured costs the data plane a warning and nothing else.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use rolter_balancer::AdaptiveTelemetry;

use crate::state::AppState;

/// Path the control plane ingests reports on, relative to the same origin the
/// gateway polls its config snapshot from.
const TELEMETRY_PATH: &str = "/internal/adaptive-telemetry";

/// How often the balancers are sampled. Deliberately slower than the config
/// poll: this is a dashboard signal, not a control loop, and every sample
/// re-runs the scorer stack for each adaptive route.
const REPORT_PERIOD: Duration = Duration::from_secs(15);

/// Header names the report identifies its node with, matching the snapshot
/// poll so one node is one row in both the cluster inventory and here.
const NODE_ID_HEADER: &str = "x-rolter-node-id";
const NODE_BUILD_HEADER: &str = "x-rolter-node-build";

/// One adaptive route's state, as this node sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTelemetry {
    /// public model name of the route
    pub model: String,
    #[serde(flatten)]
    pub state: AdaptiveTelemetry,
    /// the upstream each target forwards to, route-order aligned with
    /// `state.targets`, so a reader can name a target instead of numbering it
    pub target_labels: Vec<TargetLabel>,
}

/// Which upstream a target index points at.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetLabel {
    pub provider: String,
    /// upstream model this target forwards to (the route's model when the
    /// target does not rewrite it)
    pub upstream_model: String,
}

/// The report body posted to the control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryReport {
    pub routes: Vec<RouteTelemetry>,
}

/// Sample every route whose balancer reports adaptive telemetry. Routes on any
/// other strategy are absent rather than present-and-empty, so an operator
/// reading the report sees exactly the routes adaptive routing governs.
pub fn collect(state: &AppState) -> Vec<RouteTelemetry> {
    let snapshot = state.snapshot.load();
    let mut routes: Vec<RouteTelemetry> = snapshot
        .routes
        .iter()
        .filter_map(|(model, entry)| {
            let loads = state.loads.snapshot(model, entry.route.targets.len());
            let state = entry.balancer.telemetry(&loads)?;
            let target_labels = entry
                .route
                .targets
                .iter()
                .map(|target| TargetLabel {
                    provider: target.provider.clone(),
                    upstream_model: target.model.clone().unwrap_or_else(|| model.clone()),
                })
                .collect();
            Some(RouteTelemetry {
                model: model.clone(),
                state,
                target_labels,
            })
        })
        .collect();
    // stable order so a diff between two reports is about the numbers
    routes.sort_by(|a, b| a.model.cmp(&b.model));
    routes
}

/// The control plane's ingest endpoint for a gateway configured to poll
/// `snapshot_url`. `None` when the url is not the standard snapshot endpoint,
/// in which case the origin is unknown and reporting stays off rather than
/// guessing at a url and hammering something else with POSTs.
fn ingest_url(snapshot_url: &str) -> Option<String> {
    let path = snapshot_url.split(['?', '#']).next()?.trim_end_matches('/');
    path.strip_suffix("/internal/snapshot")
        .map(|origin| format!("{origin}{TELEMETRY_PATH}"))
}

/// Start the reporter. No-op when `snapshot_url` is not a control plane's
/// snapshot endpoint. Runs until the process exits.
pub fn spawn(state: AppState, snapshot_url: &str, internal_token: Option<String>) {
    let Some(url) = ingest_url(snapshot_url) else {
        tracing::warn!(
            %snapshot_url,
            "snapshot url is not a control-plane /internal/snapshot endpoint; \
             adaptive-routing telemetry reporting is disabled"
        );
        return;
    };
    tracing::info!(%url, period_secs = REPORT_PERIOD.as_secs(), "adaptive-routing telemetry reporting enabled");
    tokio::spawn(async move {
        // short timeout: a wedged control plane must not pin this task
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| Client::new());
        run(&client, &state, &url, internal_token.as_deref()).await;
    });
}

async fn run(client: &Client, state: &AppState, url: &str, token: Option<&str>) {
    let mut ticker = tokio::time::interval(REPORT_PERIOD);
    // a node with no adaptive routes stays silent until it has something to
    // say; once it has reported, it keeps reporting so the control plane sees
    // the route being switched off instead of holding the last state forever
    let mut reported = false;
    loop {
        ticker.tick().await;
        let routes = collect(state);
        if routes.is_empty() && !reported {
            continue;
        }
        reported = !routes.is_empty();
        if let Err(err) = report_once(client, url, token, TelemetryReport { routes }).await {
            tracing::warn!(error = %err, "adaptive-routing telemetry report failed");
        }
    }
}

/// Post one report. Factored out so tests can drive a single round trip.
async fn report_once(
    client: &Client,
    url: &str,
    token: Option<&str>,
    report: TelemetryReport,
) -> anyhow::Result<()> {
    let mut request = client.post(url).json(&report);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(id) = crate::watcher::node_id() {
        request = request
            .header(NODE_ID_HEADER, id)
            .header(NODE_BUILD_HEADER, env!("CARGO_PKG_VERSION"));
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("telemetry endpoint returned {}", response.status());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{
        AdaptiveRoutingConfig, BalancingStrategy, GatewayConfig, ModelRoute, Target,
    };

    fn target(provider: &str, model: Option<&str>) -> Target {
        Target {
            provider: provider.to_string(),
            model: model.map(str::to_string),
            weight: 1,
        }
    }

    fn route(model: &str, strategy: BalancingStrategy, targets: Vec<Target>) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            strategy,
            targets,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
        }
    }

    fn adaptive_config() -> GatewayConfig {
        GatewayConfig {
            adaptive_routing: AdaptiveRoutingConfig {
                enabled: true,
                min_samples: 0,
                ..Default::default()
            },
            routes: vec![
                route(
                    "gpt-4o",
                    BalancingStrategy::Adaptive,
                    vec![target("openai", None), target("azure", Some("gpt-4o-2024"))],
                ),
                route(
                    "claude",
                    BalancingStrategy::default(),
                    vec![target("anthropic", None)],
                ),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn only_adaptive_routes_are_reported() {
        let state = AppState::new(&adaptive_config());
        let routes = collect(&state);
        assert_eq!(routes.len(), 1, "expected only the adaptive route");
        let route = &routes[0];
        assert_eq!(route.model, "gpt-4o");
        assert_eq!(route.state.targets.len(), 2);
        // a target that does not rewrite the model is labelled with the route's
        assert_eq!(route.target_labels[0].provider, "openai");
        assert_eq!(route.target_labels[0].upstream_model, "gpt-4o");
        assert_eq!(route.target_labels[1].upstream_model, "gpt-4o-2024");
    }

    #[test]
    fn the_report_round_trips_as_json() {
        let state = AppState::new(&adaptive_config());
        let report = TelemetryReport {
            routes: collect(&state),
        };
        let wire = serde_json::to_string(&report).expect("serialize");
        let back: TelemetryReport = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(back.routes[0].model, report.routes[0].model);
        assert_eq!(back.routes[0].state, report.routes[0].state);
    }

    #[test]
    fn the_ingest_url_is_derived_from_the_snapshot_endpoint() {
        assert_eq!(
            ingest_url("http://control:4001/internal/snapshot").as_deref(),
            Some("http://control:4001/internal/adaptive-telemetry")
        );
        // a trailing slash or an inherited query string is still the same origin
        assert_eq!(
            ingest_url("http://control:4001/internal/snapshot/?version=3").as_deref(),
            Some("http://control:4001/internal/adaptive-telemetry")
        );
        // anything else is not a control plane we know how to talk to
        assert_eq!(ingest_url("http://control:4001/"), None);
        assert_eq!(ingest_url("http://control:4001/internal/snapshots"), None);
    }

    /// Serve one request, answering with `status`, and hand back what the
    /// reporter sent.
    async fn capture_one_report(status: axum::http::StatusCode) -> (String, anyhow::Result<()>) {
        use std::sync::Arc;

        use axum::{extract::State, http::HeaderMap, routing::post, Router};
        use tokio::sync::oneshot;

        let (tx, rx) = oneshot::channel::<String>();
        let tx = Arc::new(parking_lot::Mutex::new(Some(tx)));
        let app = Router::new()
            .route(
                "/internal/adaptive-telemetry",
                post(
                    move |State(tx): State<
                        Arc<parking_lot::Mutex<Option<oneshot::Sender<String>>>>,
                    >,
                          headers: HeaderMap| async move {
                        let seen = headers
                            .get(NODE_ID_HEADER)
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string();
                        if let Some(tx) = tx.lock().take() {
                            let _ = tx.send(seen);
                        }
                        status
                    },
                ),
            )
            .with_state(tx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let outcome = report_once(
            &Client::new(),
            &format!("http://{addr}/internal/adaptive-telemetry"),
            None,
            TelemetryReport { routes: Vec::new() },
        )
        .await;
        let seen = rx.await.expect("the server saw a request");
        (seen, outcome)
    }

    #[tokio::test]
    async fn a_report_from_a_process_with_no_configured_id_still_names_its_node() {
        // a gateway started from a shell has no HOSTNAME in its environment;
        // before #1644 it sent no node header at all and the control plane
        // dropped the report without a word
        let (seen, outcome) = capture_one_report(axum::http::StatusCode::NO_CONTENT).await;
        assert!(outcome.is_ok());
        assert!(
            !seen.is_empty(),
            "the report must carry a node id, got an empty header"
        );
    }

    #[tokio::test]
    async fn a_refused_report_is_surfaced_as_an_error() {
        // the reporter only logs when the round trip fails, so the control
        // plane refusing an unidentified report is what makes the drop audible
        let (_, outcome) = capture_one_report(axum::http::StatusCode::BAD_REQUEST).await;
        let err = outcome.expect_err("a 400 must not look like a delivered report");
        assert!(
            err.to_string().contains("400"),
            "expected the status in the error, got {err}"
        );
    }
}
