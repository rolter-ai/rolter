//! Circuit-breaker state reporting (#2254).
//!
//! Every gateway replica keeps its own breakers in memory ([`crate::breaker`]),
//! so the control plane can show the fleet's view only if each replica says what
//! it holds. This module samples the registry on a timer and pushes it over the
//! channel the cluster inventory and the adaptive-routing scoreboard already
//! use: the internal token and the node identity of the snapshot poll.
//!
//! Nothing here runs on a request. The sample takes the registry lock once per
//! tick, off the hot path, and reporting is best effort: a control plane that
//! is down, old, or not configured costs the data plane a warning.

use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::breaker::{BreakerPhase, BreakerView};
use crate::state::AppState;

/// Path the control plane ingests reports on, relative to the origin the
/// gateway polls its config snapshot from.
const REPORT_PATH: &str = "/internal/breaker-state";

/// How often the registry is sampled. Faster than the adaptive scoreboard
/// because a breaker's open window is seconds, not minutes, and a dashboard
/// that lags it by a minute would name a recovered target as down.
const REPORT_PERIOD: Duration = Duration::from_secs(10);

const NODE_ID_HEADER: &str = "x-rolter-node-id";
const NODE_BUILD_HEADER: &str = "x-rolter-node-build";

/// One target's breaker, as this node sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetBreaker {
    /// the org that owns the route, empty for a route from the gateway's own
    /// config file. Two orgs may each have a route of one name (ADR-0040), so
    /// the control plane keys a route by `(org_id, model)` (#3458)
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub org_id: String,
    /// public model name of the route, never its qualified runtime namespace
    pub model: String,
    /// position in the route's target list
    pub target_index: usize,
    /// the provider the target forwards to; empty when the route has since
    /// been removed or shrunk by a config reload
    pub provider: String,
    /// the upstream model the target forwards to
    pub upstream_model: String,
    pub state: BreakerPhase,
    pub consecutive_failures: u32,
    /// whole seconds until an open breaker admits its probe
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<u64>,
}

/// The report body posted to the control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakerReport {
    pub targets: Vec<TargetBreaker>,
}

/// Label the registry's entries with the provider each target points at in the
/// current snapshot. An entry for a route that a reload has since removed is
/// dropped: the registry sweeps it on the next reload, and naming no provider
/// would only make a row nobody can act on.
pub fn collect(state: &AppState) -> Vec<TargetBreaker> {
    let snapshot = state.snapshot.load();
    // the registry is keyed by route namespace, which is the route's name
    // unless another org's route shares it (ADR-0040); the report names the
    // route by its org and name instead, so it reads the same either way
    label(state.breaker.snapshot(), |namespace, idx| {
        let (org, model, entry) = snapshot
            .routes
            .iter()
            .find(|(_, _, entry)| *entry.namespace == *namespace)?;
        let target = entry.route.targets.get(idx)?;
        Some(Label {
            org_id: org.to_string(),
            model: model.to_string(),
            provider: snapshot.provider_name(&target.provider).to_string(),
            upstream_model: target
                .model
                .clone()
                .unwrap_or_else(|| entry.route.model.clone()),
        })
    })
}

/// What a breaker entry is reported as: its route's org and name, and the
/// target's provider and upstream model.
struct Label {
    org_id: String,
    model: String,
    provider: String,
    upstream_model: String,
}

fn label(
    views: Vec<BreakerView>,
    lookup: impl Fn(&str, usize) -> Option<Label>,
) -> Vec<TargetBreaker> {
    let mut targets: Vec<TargetBreaker> = views
        .into_iter()
        .filter_map(|view| {
            let Label {
                org_id,
                model,
                provider,
                upstream_model,
            } = lookup(&view.model, view.target_index)?;
            Some(TargetBreaker {
                org_id,
                model,
                target_index: view.target_index,
                provider,
                upstream_model,
                state: view.state,
                consecutive_failures: view.consecutive_failures,
                retry_after_secs: view.retry_after_secs,
            })
        })
        .collect();
    // stable order so a diff between two reports is about the state
    targets.sort_by(|a, b| {
        (&a.model, &a.org_id, a.target_index).cmp(&(&b.model, &b.org_id, b.target_index))
    });
    targets
}

/// The control plane's ingest endpoint for a gateway configured to poll
/// `snapshot_url`. `None` when the url is not the standard snapshot endpoint,
/// in which case reporting stays off rather than guessing at a url.
fn ingest_url(snapshot_url: &str) -> Option<String> {
    let path = snapshot_url.split(['?', '#']).next()?.trim_end_matches('/');
    path.strip_suffix("/internal/snapshot")
        .map(|origin| format!("{origin}{REPORT_PATH}"))
}

/// Start the reporter. No-op when `snapshot_url` is not a control plane's
/// snapshot endpoint. Runs until the process exits.
pub fn spawn(state: AppState, snapshot_url: &str, internal_token: Option<String>) {
    let Some(url) = ingest_url(snapshot_url) else {
        tracing::warn!(
            snapshot_url = %rolter_core::redact::redact_url(snapshot_url),
            "snapshot url is not a control-plane /internal/snapshot endpoint; \
             circuit-breaker state reporting is disabled"
        );
        return;
    };
    tracing::info!(url = %rolter_core::redact::redact_url(&url), period_secs = REPORT_PERIOD.as_secs(), "circuit-breaker state reporting enabled");
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
    // a node whose breakers have never moved stays silent; once it has
    // reported a non-empty state it keeps reporting, so the control plane sees
    // the last breaker close instead of holding the open state forever
    let mut reported = false;
    loop {
        ticker.tick().await;
        let targets = collect(state);
        if targets.is_empty() && !reported {
            continue;
        }
        reported = !targets.is_empty();
        if let Err(err) = report_once(client, url, token, BreakerReport { targets }).await {
            tracing::warn!(error = %rolter_core::redact::redact_urls_in_text(&err.to_string()), "circuit-breaker state report failed");
        }
    }
}

/// Post one report. Factored out so tests can drive a single round trip.
async fn report_once(
    client: &Client,
    url: &str,
    token: Option<&str>,
    report: BreakerReport,
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
        anyhow::bail!("breaker report endpoint returned {}", response.status());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rolter_core::{BreakerConfig, GatewayConfig, ModelRoute, Target};

    fn route(model: &str, targets: Vec<Target>) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            strategy: Default::default(),
            targets,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
            tenancy: None,
        }
    }

    fn target(provider: &str, model: Option<&str>) -> Target {
        Target {
            provider: provider.to_string(),
            model: model.map(str::to_string),
            ..Default::default()
        }
    }

    fn config() -> GatewayConfig {
        GatewayConfig {
            breaker: BreakerConfig {
                enabled: true,
                failure_threshold: 1,
                open_secs: 30,
            },
            routes: vec![route(
                "gpt-4o",
                vec![target("openai", None), target("azure", Some("gpt-4o-2024"))],
            )],
            ..Default::default()
        }
    }

    #[test]
    fn an_open_target_is_labelled_with_its_provider_and_upstream_model() {
        let state = AppState::new(&config());
        assert!(state.breaker.on_failure("gpt-4o", 1));
        let targets = collect(&state);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].provider, "azure");
        assert_eq!(targets[0].upstream_model, "gpt-4o-2024");
        assert_eq!(targets[0].state, BreakerPhase::Open);
    }

    /// #3458: two orgs' `gpt-4o` have the namespaces `gpt-4o@org-a` and
    /// `gpt-4o@org-b`; each is reported as the route's name with its org, so
    /// the control plane keeps them apart and can match each to its route.
    #[test]
    fn two_orgs_same_named_routes_are_reported_by_org_and_name() {
        let mut config = config();
        let mut second = config.routes[0].clone();
        for (route, org) in [(&mut config.routes[0], "org-a"), (&mut second, "org-b")] {
            route.tenancy = Some(rolter_core::Tenancy {
                org_id: org.to_string(),
                project_id: None,
            });
        }
        config.routes.push(second);
        let state = AppState::new(&config);
        assert!(state.breaker.on_failure("gpt-4o@org-a", 0));
        assert!(state.breaker.on_failure("gpt-4o@org-b", 1));
        let targets: Vec<_> = collect(&state)
            .into_iter()
            .map(|t| (t.org_id, t.model, t.provider))
            .collect();
        assert_eq!(
            targets,
            [
                ("org-a".into(), "gpt-4o".into(), "openai".into()),
                ("org-b".into(), "gpt-4o".into(), "azure".into()),
            ]
        );
    }

    #[test]
    fn an_entry_for_a_route_that_no_longer_exists_is_not_reported() {
        let state = AppState::new(&config());
        state.breaker.on_failure("retired", 0);
        // and an index past the route's current targets
        state.breaker.on_failure("gpt-4o", 9);
        assert!(collect(&state).is_empty());
    }

    #[test]
    fn the_report_round_trips_as_json() {
        let state = AppState::new(&config());
        state.breaker.on_failure("gpt-4o", 0);
        let report = BreakerReport {
            targets: collect(&state),
        };
        let wire = serde_json::to_string(&report).expect("serialize");
        assert!(wire.contains("\"state\":\"open\""), "{wire}");
        let back: BreakerReport = serde_json::from_str(&wire).expect("deserialize");
        assert_eq!(back.targets, report.targets);
    }

    #[test]
    fn the_ingest_url_is_derived_from_the_snapshot_endpoint() {
        assert_eq!(
            ingest_url("http://control:4001/internal/snapshot").as_deref(),
            Some("http://control:4001/internal/breaker-state")
        );
        assert_eq!(ingest_url("http://control:4001/"), None);
    }
}
