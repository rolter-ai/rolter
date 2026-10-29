//! Circuit-breaker state across the fleet (#2254).
//!
//! Each gateway keeps its breakers in memory, one registry per replica, so the
//! control plane can only show them if every replica says what it holds. Each
//! gateway pushes a periodic report over the internal channel it already uses
//! for the config snapshot and the adaptive-routing scoreboard, carrying the
//! node identity of the snapshot poll (#543), and this module keeps the newest
//! report per node.
//!
//! It is *current state*, not history: a report replaces everything its node
//! sent before, so a breaker that has closed again simply leaves the table. A
//! target absent from a live node's report is therefore closed, which is what
//! the gateway's own registry means by an absent entry.

use std::collections::{BTreeMap, HashSet};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rolter_store::postgres::models::BreakerState;
use rolter_store::postgres::repo::{BreakerSample, BreakerStateRepo, ClusterNodeRepo};

use crate::access_control::caller_policy;
use crate::cluster::{is_live_gateway, NODE_ID_HEADER};
use crate::crud::{pool, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::tenant_view::TenantView;
use crate::ControlState;

/// A report older than this is not current state any more. Gateways report
/// every 10s, so this rides out three missed reports before a node goes quiet.
const FRESH_WINDOW_SECS: i64 = 40;
/// How long a report survives after its node stops reporting.
const RETENTION_SECS: i64 = 3600;

/// Caps on one report, so a misbehaving or hostile node holding the internal
/// token cannot turn the table into a landfill. Anything past the cap is
/// dropped, not rejected: partial state beats none.
const MAX_TARGETS: usize = 5000;
const MAX_MODEL_LEN: usize = 512;
const MAX_LABEL_LEN: usize = 512;
const MAX_ORG_LEN: usize = 128;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/breaker-state", get(get_breaker_state))
}

/// The ingest half, mounted under `/internal/*` behind the internal token.
pub(crate) fn internal_router() -> Router<ControlState> {
    Router::new().route("/internal/breaker-state", post(ingest))
}

#[derive(Debug, Clone, Deserialize)]
struct TargetReport {
    /// the org that owns the route, empty for a deployment-wide route and from
    /// a gateway that predates per-org namespaces (#3458)
    #[serde(default)]
    org_id: String,
    /// the route's name, never its qualified runtime namespace
    model: String,
    target_index: i64,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    upstream_model: String,
    state: String,
    #[serde(default)]
    consecutive_failures: i64,
    #[serde(default)]
    retry_after_secs: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct BreakerReport {
    #[serde(default)]
    targets: Vec<TargetReport>,
}

/// Whether a reported row is storable. An unknown `state` is dropped rather
/// than stored, since the column is constrained to the three the gateway sends
/// and a newer gateway's extra phase must not fail the whole report.
fn storable(target: &TargetReport) -> bool {
    let model = target.model.trim();
    !model.is_empty()
        && model.len() <= MAX_MODEL_LEN
        && target.org_id.len() <= MAX_ORG_LEN
        && target.provider.len() <= MAX_LABEL_LEN
        && target.upstream_model.len() <= MAX_LABEL_LEN
        && (0..=i64::from(i32::MAX)).contains(&target.target_index)
        && matches!(target.state.as_str(), "closed" | "open" | "half_open")
}

/// Record one node's report.
///
/// A report with no usable node identity is answered `400`, not `204`, for the
/// reason the adaptive-routing scoreboard does (#1644): the table is keyed on
/// the node, and a silent success would leave the reporter with nothing to log.
/// Silent (and successful) when the deployment has no database.
async fn ingest(
    State(state): State<ControlState>,
    headers: HeaderMap,
    SafeJson(body): SafeJson<BreakerReport>,
) -> StatusCode {
    let Some(node_id) = node_id(&headers) else {
        tracing::debug!(
            header = NODE_ID_HEADER,
            "dropping a circuit-breaker report with no usable node identity"
        );
        return StatusCode::BAD_REQUEST;
    };
    let Some(pool) = state.pool.as_ref() else {
        return StatusCode::NO_CONTENT;
    };
    let reports: Vec<TargetReport> = body
        .targets
        .into_iter()
        .filter(storable)
        .take(MAX_TARGETS)
        .collect();
    let samples: Vec<BreakerSample<'_>> = reports
        .iter()
        .map(|target| BreakerSample {
            org_id: target.org_id.trim(),
            model: target.model.trim(),
            // the range was checked by `storable`
            target_index: target.target_index as i32,
            provider: &target.provider,
            upstream_model: &target.upstream_model,
            state: &target.state,
            consecutive_failures: target.consecutive_failures.clamp(0, i64::from(i32::MAX)) as i32,
            retry_after_secs: target
                .retry_after_secs
                .map(|secs| secs.clamp(0, i64::from(i32::MAX)) as i32),
        })
        .collect();
    let repo = BreakerStateRepo(pool);
    if let Err(err) = repo.report(&node_id, &samples).await {
        tracing::warn!(error = %err, node = %node_id, "failed to record circuit-breaker state");
        return StatusCode::NO_CONTENT;
    }
    // pruning rides on the write path: no other timer has to exist for it
    if let Err(err) = repo.prune(RETENTION_SECS).await {
        tracing::warn!(error = %err, "failed to prune circuit-breaker state");
    }
    StatusCode::NO_CONTENT
}

/// The node identity header the snapshot poll already sends, sanitized the
/// same way (#543): bounded, control-character free, or absent.
fn node_id(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(NODE_ID_HEADER)?.to_str().ok()?.trim();
    if raw.is_empty() || raw.len() > 128 || raw.chars().any(char::is_control) {
        return None;
    }
    Some(raw.to_string())
}

/// One node's breaker for one target. Serialized only for a caller who may
/// read the cluster inventory, since a node id is a host name.
#[derive(Debug, Serialize)]
struct NodeBreaker {
    node_id: String,
    state: String,
    consecutive_failures: i32,
    /// seconds until an open breaker admits its probe, as of `reported_at`
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<i32>,
    reported_at: DateTime<Utc>,
}

/// A route's target and what the fleet says about its breaker. A route is
/// `(org_id, model)`: two orgs may each have a route of one name (#1857).
#[derive(Debug, Serialize)]
struct TargetBreakers {
    /// the org that owns the route, empty for a deployment-wide route
    org_id: String,
    model: String,
    /// position in the route's target list
    target_index: i32,
    provider: String,
    upstream_model: String,
    /// the worst state any node reports: `open`, else `half_open`, else `closed`
    state: String,
    /// gateways reporting this breaker open
    nodes_open: usize,
    nodes_half_open: usize,
    /// per-node detail; empty unless the caller may read the cluster inventory
    nodes: Vec<NodeBreaker>,
}

#[derive(Debug, Serialize)]
struct BreakerView {
    /// when this view was assembled, so a stale dashboard tab is obvious
    generated_at: DateTime<Utc>,
    /// reports older than this were not included
    fresh_window_secs: i64,
    /// gateways that polled recently enough to count as live, whether or not
    /// they hold a non-closed breaker. A target not listed is closed on every
    /// one of them; `0` means nothing can be said about the fleet at all
    gateways_live: usize,
    /// every target at least one live node holds a breaker entry for. A target
    /// that is not listed is closed
    targets: Vec<TargetBreakers>,
}

fn rank(state: &str) -> u8 {
    match state {
        "open" => 2,
        "half_open" => 1,
        _ => 0,
    }
}

/// Fold per-node rows into one entry per `(org, model, target)`. The output
/// is ordered by model, org and target, so it is stable. Targets whose route
/// is not in `visible`, as `(org, model)`, are dropped: another org may have a
/// route of the same name (#3458).
fn group(
    rows: Vec<BreakerState>,
    visible: &HashSet<(String, String)>,
    with_nodes: bool,
) -> Vec<TargetBreakers> {
    let mut grouped: BTreeMap<(String, String, i32), TargetBreakers> = BTreeMap::new();
    for row in rows {
        if !visible.contains(&(row.org_id.clone(), row.model.clone())) {
            continue;
        }
        let entry = grouped
            .entry((row.model.clone(), row.org_id.clone(), row.target_index))
            .or_insert_with(|| TargetBreakers {
                org_id: row.org_id.clone(),
                model: row.model.clone(),
                target_index: row.target_index,
                provider: row.provider.clone(),
                upstream_model: row.upstream_model.clone(),
                state: "closed".to_string(),
                nodes_open: 0,
                nodes_half_open: 0,
                nodes: Vec::new(),
            });
        match row.state.as_str() {
            "open" => entry.nodes_open += 1,
            "half_open" => entry.nodes_half_open += 1,
            _ => {}
        }
        if rank(&row.state) > rank(&entry.state) {
            entry.state = row.state.clone();
        }
        if with_nodes {
            entry.nodes.push(NodeBreaker {
                node_id: row.node_id,
                state: row.state,
                consecutive_failures: row.consecutive_failures,
                retry_after_secs: row.retry_after_secs,
                reported_at: row.reported_at,
            });
        }
    }
    grouped.into_values().collect()
}

/// Circuit-breaker state across the gateway fleet. Any signed-in caller may
/// ask, and sees the targets of the models they may see in the catalog
/// (`GET /api/v1/models`), by the same tenancy and access-profile rules; node
/// ids are shown only to a caller who sees everything.
async fn get_breaker_state(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<BreakerView>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("breaker_state", Read),
    )
    .await?;
    let config = state.store.load().await?;
    let policy = caller_policy(&state, &principal).await?;
    let mut view = TenantView::load(&state, Some(&principal)).await?;
    let mut visible = HashSet::new();
    for route in &config.routes {
        if policy.permits_model(&route.model) && view.sees(&state, route.tenancy.as_ref()).await {
            let org = rolter_core::namespace::org_of(route.tenancy.as_ref());
            visible.insert((org.to_string(), route.model.clone()));
        }
    }
    let pool = pool(&state);
    let rows = BreakerStateRepo(pool).list_fresh(FRESH_WINDOW_SECS).await?;
    let now = Utc::now();
    let gateways_live = ClusterNodeRepo(pool)
        .list()
        .await?
        .iter()
        .filter(|node| is_live_gateway(node, now))
        .count();
    Ok(Json(BreakerView {
        generated_at: now,
        fresh_window_secs: FRESH_WINDOW_SECS,
        gateways_live,
        targets: group(rows, &visible, view.sees_everything()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn row(node: &str, model: &str, idx: i32, state: &str) -> BreakerState {
        BreakerState {
            node_id: node.to_string(),
            org_id: String::new(),
            model: model.to_string(),
            target_index: idx,
            provider: "openai".to_string(),
            upstream_model: model.to_string(),
            state: state.to_string(),
            consecutive_failures: 5,
            retry_after_secs: (state == "open").then_some(20),
            reported_at: Utc::now(),
        }
    }

    fn visible(models: &[&str]) -> HashSet<(String, String)> {
        models
            .iter()
            .map(|m| (String::new(), m.to_string()))
            .collect()
    }

    /// #3458: two orgs' `gpt-4o` are two routes. An admin of one org sees
    /// only that org's breaker, and a caller who sees both gets two entries
    /// rather than one merged one.
    #[test]
    fn two_orgs_same_named_routes_are_two_entries_each_visible_to_its_own_org() {
        let of = |org: &str, node: &str, state: &str| BreakerState {
            org_id: org.to_string(),
            ..row(node, "gpt-4o", 0, state)
        };
        let rows = || {
            vec![
                of("org-a", "gw-1", "open"),
                of("org-b", "gw-1", "half_open"),
                of("org-b", "gw-2", "closed"),
            ]
        };
        let org = |org: &str| -> HashSet<(String, String)> {
            [(org.to_string(), "gpt-4o".to_string())].into()
        };
        let a = group(rows(), &org("org-a"), false);
        assert_eq!(a.len(), 1);
        assert_eq!(
            (a[0].org_id.as_str(), a[0].state.as_str()),
            ("org-a", "open")
        );
        let b = group(rows(), &org("org-b"), false);
        assert_eq!(b.len(), 1);
        assert_eq!(
            (
                b[0].org_id.as_str(),
                b[0].state.as_str(),
                b[0].nodes_half_open
            ),
            ("org-b", "half_open", 1)
        );
        // a global route of the same name is a third route, seen by neither
        assert!(group(rows(), &visible(&["gpt-4o"]), false).is_empty());
        let both: HashSet<_> = org("org-a").union(&org("org-b")).cloned().collect();
        let all = group(rows(), &both, true);
        let seen: Vec<_> = all
            .iter()
            .map(|t| (t.org_id.as_str(), t.state.as_str(), t.nodes.len()))
            .collect();
        assert_eq!(seen, [("org-a", "open", 1), ("org-b", "half_open", 2)]);
    }

    #[test]
    fn nodes_fold_into_one_entry_per_target_with_the_worst_state() {
        let rows = vec![
            row("gw-1", "gpt-4o", 0, "half_open"),
            row("gw-2", "gpt-4o", 0, "open"),
            row("gw-1", "gpt-4o", 1, "closed"),
        ];
        let targets = group(rows, &visible(&["gpt-4o"]), true);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].state, "open");
        assert_eq!((targets[0].nodes_open, targets[0].nodes_half_open), (1, 1));
        assert_eq!(targets[0].nodes.len(), 2);
        assert_eq!(targets[1].state, "closed");
    }

    #[test]
    fn a_model_the_caller_cannot_see_is_dropped() {
        let rows = vec![
            row("gw-1", "gpt-4o", 0, "open"),
            row("gw-1", "secret-model", 0, "open"),
        ];
        let targets = group(rows, &visible(&["gpt-4o"]), false);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].model, "gpt-4o");
    }

    #[test]
    fn node_ids_are_withheld_unless_asked_for() {
        let rows = vec![row("gw-1", "gpt-4o", 0, "open")];
        let targets = group(rows, &visible(&["gpt-4o"]), false);
        assert!(targets[0].nodes.is_empty());
        // the count of nodes still comes through
        assert_eq!(targets[0].nodes_open, 1);
    }

    #[test]
    fn unknown_states_and_out_of_range_indices_are_not_stored() {
        let report = |state: &str, idx: i64| TargetReport {
            org_id: String::new(),
            model: "m".to_string(),
            target_index: idx,
            provider: String::new(),
            upstream_model: String::new(),
            state: state.to_string(),
            consecutive_failures: 0,
            retry_after_secs: None,
        };
        assert!(storable(&report("open", 0)));
        assert!(!storable(&report("exploded", 0)));
        assert!(!storable(&report("open", -1)));
        assert!(!storable(&report("open", i64::MAX)));
        let mut blank = report("open", 0);
        blank.model = "  ".to_string();
        assert!(!storable(&blank));
        let mut long_org = report("open", 0);
        long_org.org_id = "o".repeat(MAX_ORG_LEN + 1);
        assert!(!storable(&long_org));
    }

    #[test]
    fn the_node_header_is_bounded_and_control_free() {
        let mut headers = HeaderMap::new();
        headers.insert(NODE_ID_HEADER, HeaderValue::from_static("  gw-1 "));
        assert_eq!(node_id(&headers).as_deref(), Some("gw-1"));
        headers.insert(NODE_ID_HEADER, HeaderValue::from_static("   "));
        assert_eq!(node_id(&headers), None);
    }
}
