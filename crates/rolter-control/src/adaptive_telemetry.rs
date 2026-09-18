//! Adaptive-routing telemetry (#751): what the `adaptive` strategy is actually
//! doing, read back out of the data plane.
//!
//! The scores live in the gateway's balancers, which the control plane cannot
//! see. Rather than open a second channel, each gateway pushes a periodic
//! sample over the internal token it already holds — the same shape of
//! arrangement as the cluster inventory (#543), which piggybacks on the
//! snapshot poll — and this module keeps the newest sample per (node, route).
//!
//! It is deliberately *current state*, not a time series: one row per node and
//! route, overwritten on every report. That is enough for the dashboard's
//! first screen and cheap enough to be free. History, if it is ever wanted,
//! belongs in the request log where per-request rows already go, and would be
//! served as a separate endpoint rather than by changing the shape of this one.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rolter_store::postgres::models::AdaptiveRoutingTelemetry;
use rolter_store::postgres::repo::{AdaptiveRoutingSample, AdaptiveRoutingTelemetryRepo};

use crate::cluster::NODE_ID_HEADER;
use crate::crud::{pool, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

/// A sample older than this is not current state any more. Nodes report every
/// 15s, so this rides out three missed reports before a node goes quiet.
const FRESH_WINDOW_SECS: i64 = 60;
/// How long a sample survives after a node stops reporting. Long enough to
/// diagnose a node that just went away, short enough that a scaled-down fleet
/// does not leave rows behind forever.
const RETENTION_SECS: i64 = 3600;

/// Caps on one report, so a misbehaving or hostile node holding the internal
/// token cannot turn the scoreboard into a landfill. Anything past the cap is
/// dropped, not rejected: partial telemetry beats none.
const MAX_ROUTES: usize = 500;
const MAX_TARGETS: usize = 256;
const MAX_MODEL_LEN: usize = 512;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route("/api/v1/adaptive-routing-telemetry", get(get_telemetry))
}

/// The ingest half, mounted under `/internal/*` behind the internal token.
pub(crate) fn internal_router() -> Router<ControlState> {
    Router::new().route("/internal/adaptive-telemetry", post(ingest))
}

/// Pick counts by mode, mirroring `rolter_balancer::DecisionCounts`.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
struct DecisionCounts {
    #[serde(default)]
    blend: i64,
    #[serde(default)]
    exploration: i64,
    #[serde(default)]
    fallback: i64,
}

/// One route as a gateway reports it. Every field past `model` is optional so
/// a node running an older or newer build still lands a usable row instead of
/// failing the whole report.
#[derive(Debug, Clone, Deserialize)]
struct RouteReport {
    model: String,
    #[serde(default)]
    engaged: bool,
    #[serde(default)]
    observed: i64,
    #[serde(default)]
    decisions: DecisionCounts,
    #[serde(default)]
    policy: serde_json::Value,
    #[serde(default)]
    targets: Vec<serde_json::Value>,
    /// which upstream each target index points at, target-order aligned
    #[serde(default)]
    target_labels: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelemetryReport {
    #[serde(default)]
    routes: Vec<RouteReport>,
}

/// Fold a target's label into the signals for the same index, so the stored
/// document is one array of self-describing targets rather than two arrays a
/// reader has to zip.
fn merge_targets(report: &RouteReport) -> serde_json::Value {
    let merged: Vec<serde_json::Value> = report
        .targets
        .iter()
        .take(MAX_TARGETS)
        .enumerate()
        .map(|(i, signals)| {
            let mut target = signals.clone();
            if let (Some(object), Some(serde_json::Value::Object(label))) =
                (target.as_object_mut(), report.target_labels.get(i))
            {
                for (key, value) in label {
                    object.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            target
        })
        .collect();
    serde_json::Value::Array(merged)
}

/// Record one node's report.
///
/// A report with no usable node identity is answered `400`, not `204`. The
/// scoreboard is keyed on the node, so such a report cannot be stored under
/// any circumstance, and pretending it succeeded is what made #1644 invisible:
/// the gateway's reporter only warns on a failure status, so a silent `204`
/// left two dashboard screens empty with nothing logged on either plane.
///
/// Silent (and successful) when the deployment has no database — that is a
/// deployment shape, not a caller error, and must never look like a config
/// problem to a gateway.
async fn ingest(
    State(state): State<ControlState>,
    headers: HeaderMap,
    SafeJson(body): SafeJson<TelemetryReport>,
) -> StatusCode {
    let Some(node_id) = node_id(&headers) else {
        tracing::debug!(
            header = NODE_ID_HEADER,
            "dropping an adaptive-routing telemetry report with no usable node identity"
        );
        return StatusCode::BAD_REQUEST;
    };
    let Some(pool) = state.pool.as_ref() else {
        return StatusCode::NO_CONTENT;
    };
    let reports: Vec<RouteReport> = body
        .routes
        .into_iter()
        .filter(|route| {
            let model = route.model.trim();
            !model.is_empty() && model.len() <= MAX_MODEL_LEN
        })
        .take(MAX_ROUTES)
        .collect();
    let samples: Vec<AdaptiveRoutingSample<'_>> = reports
        .iter()
        .map(|route| AdaptiveRoutingSample {
            model: route.model.trim(),
            engaged: route.engaged,
            observed: route.observed,
            blend_picks: route.decisions.blend,
            exploration_picks: route.decisions.exploration,
            fallback_picks: route.decisions.fallback,
            policy: route.policy.clone(),
            targets: merge_targets(route),
        })
        .collect();
    let repo = AdaptiveRoutingTelemetryRepo(pool);
    if let Err(err) = repo.report(&node_id, &samples).await {
        tracing::warn!(error = %err, node = %node_id, "failed to record adaptive routing telemetry");
        return StatusCode::NO_CONTENT;
    }
    // pruning rides on the write path: there is no other timer that has to
    // exist for this, and the cost is one indexed delete every 15s per node
    if let Err(err) = repo.prune(RETENTION_SECS).await {
        tracing::warn!(error = %err, "failed to prune adaptive routing telemetry");
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

/// One node's view of one route.
#[derive(Debug, Serialize)]
struct NodeTelemetry {
    node_id: String,
    engaged: bool,
    observed: i64,
    decisions: DecisionCounts,
    /// the sanitized policy that node runs; it can lag the stored policy until
    /// the node converges on the config version carrying it
    policy: serde_json::Value,
    targets: serde_json::Value,
    reported_at: DateTime<Utc>,
}

/// A route and every node balancing it, so the dashboard renders one card per
/// route rather than one per (node, route) pair.
#[derive(Debug, Serialize)]
struct RouteTelemetry {
    model: String,
    /// true when at least one node has the blend engaged for this route
    engaged: bool,
    nodes: Vec<NodeTelemetry>,
}

#[derive(Debug, Serialize)]
struct TelemetryView {
    /// when this view was assembled, so a stale dashboard tab is obvious
    generated_at: DateTime<Utc>,
    /// samples older than this were not included
    fresh_window_secs: i64,
    routes: Vec<RouteTelemetry>,
}

/// Current adaptive-routing state across the fleet. Superadmin-only, like the
/// policy it reflects: it names every route and upstream in the deployment and
/// has no tenancy scope to be an admin of.
async fn get_telemetry(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<TelemetryView>> {
    authorize_superadmin(
        &principal,
        superadmin_cap!("adaptive_routing_telemetry", Read),
    )?;
    let rows = AdaptiveRoutingTelemetryRepo(pool(&state))
        .list_fresh(FRESH_WINDOW_SECS)
        .await?;
    Ok(Json(TelemetryView {
        generated_at: Utc::now(),
        fresh_window_secs: FRESH_WINDOW_SECS,
        routes: group_by_route(rows),
    }))
}

/// Fold `(node, route)` rows into one entry per route. Rows arrive ordered by
/// model, so this is a single pass and the route order is stable.
fn group_by_route(rows: Vec<AdaptiveRoutingTelemetry>) -> Vec<RouteTelemetry> {
    let mut routes: Vec<RouteTelemetry> = Vec::new();
    for row in rows {
        let node = NodeTelemetry {
            node_id: row.node_id,
            engaged: row.engaged,
            observed: row.observed,
            decisions: DecisionCounts {
                blend: row.blend_picks,
                exploration: row.exploration_picks,
                fallback: row.fallback_picks,
            },
            policy: row.policy,
            targets: row.targets,
            reported_at: row.reported_at,
        };
        match routes.last_mut() {
            Some(route) if route.model == row.model => {
                route.engaged |= node.engaged;
                route.nodes.push(node);
            }
            _ => routes.push(RouteTelemetry {
                model: row.model,
                engaged: node.engaged,
                nodes: vec![node],
            }),
        }
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;

    fn row(node_id: &str, model: &str, engaged: bool) -> AdaptiveRoutingTelemetry {
        AdaptiveRoutingTelemetry {
            node_id: node_id.to_string(),
            model: model.to_string(),
            engaged,
            observed: 10,
            blend_picks: 7,
            exploration_picks: 1,
            fallback_picks: 2,
            policy: json!({"enabled": true}),
            targets: json!([]),
            reported_at: Utc::now(),
        }
    }

    #[test]
    fn rows_group_into_one_entry_per_route() {
        // ordered by model, as the repo returns them
        let rows = vec![
            row("gw-1", "claude", false),
            row("gw-1", "gpt-4o", false),
            row("gw-2", "gpt-4o", true),
        ];
        let routes = group_by_route(rows);
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].model, "claude");
        assert_eq!(routes[1].nodes.len(), 2);
        // a route counts as engaged when any node has the blend routing
        assert!(!routes[0].engaged);
        assert!(routes[1].engaged);
    }

    #[test]
    fn target_labels_are_folded_into_the_signals() {
        let report = RouteReport {
            model: "gpt-4o".to_string(),
            engaged: true,
            observed: 3,
            decisions: DecisionCounts::default(),
            policy: json!({}),
            targets: vec![json!({"target": 0, "score": 1.5}), json!({"target": 1})],
            target_labels: vec![json!({"provider": "openai"}), json!({"provider": "azure"})],
        };
        let merged = merge_targets(&report);
        assert_eq!(merged[0]["provider"], "openai");
        assert_eq!(merged[0]["score"], 1.5);
        assert_eq!(merged[1]["provider"], "azure");
    }

    #[test]
    fn a_label_never_overwrites_a_reported_signal() {
        let report = RouteReport {
            model: "gpt-4o".to_string(),
            engaged: false,
            observed: 0,
            decisions: DecisionCounts::default(),
            policy: json!({}),
            targets: vec![json!({"target": 0, "provider": "real"})],
            target_labels: vec![json!({"provider": "spoofed"})],
        };
        assert_eq!(merge_targets(&report)[0]["provider"], "real");
    }

    #[test]
    fn missing_labels_leave_the_signals_alone() {
        let report = RouteReport {
            model: "gpt-4o".to_string(),
            engaged: false,
            observed: 0,
            decisions: DecisionCounts::default(),
            policy: json!({}),
            targets: vec![json!({"target": 0}), json!({"target": 1})],
            target_labels: Vec::new(),
        };
        let merged = merge_targets(&report);
        assert_eq!(merged.as_array().map(Vec::len), Some(2));
        assert!(merged[0].get("provider").is_none());
    }

    #[test]
    fn an_oversized_target_list_is_truncated_not_rejected() {
        let report = RouteReport {
            model: "gpt-4o".to_string(),
            engaged: false,
            observed: 0,
            decisions: DecisionCounts::default(),
            policy: json!({}),
            targets: (0..MAX_TARGETS + 50)
                .map(|i| json!({"target": i}))
                .collect(),
            target_labels: Vec::new(),
        };
        assert_eq!(
            merge_targets(&report).as_array().map(Vec::len),
            Some(MAX_TARGETS)
        );
    }

    #[test]
    fn an_unidentified_or_malformed_node_has_no_identity() {
        let mut headers = HeaderMap::new();
        assert_eq!(node_id(&headers), None);
        headers.insert(NODE_ID_HEADER, HeaderValue::from_static("  gw-1  "));
        assert_eq!(node_id(&headers).as_deref(), Some("gw-1"));
        headers.insert(NODE_ID_HEADER, HeaderValue::from_static("   "));
        assert_eq!(node_id(&headers), None);
    }
}
