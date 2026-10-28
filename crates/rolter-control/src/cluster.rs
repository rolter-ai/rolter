//! Cluster inventory (#543).
//!
//! Nodes are not enrolled through a separate API: every gateway already polls
//! `/internal/snapshot` on a timer, so a poll that carries the node identity
//! headers *is* the heartbeat. That keeps single-node deployments zero-config —
//! a node that sends no headers simply never appears in the inventory — and
//! keeps config propagation the one source of truth for convergence.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use rolter_core::Error;
use rolter_store::postgres::models::ClusterNode;
use rolter_store::postgres::repo::{AuditLogRepo, ClusterNodeRepo};

use crate::crud::{pool, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain};
use crate::rbac_matrix::{cap, superadmin_cap};
use crate::ControlState;

/// Header names a node uses to identify itself on its snapshot poll.
pub(crate) const NODE_ID_HEADER: &str = "x-rolter-node-id";
pub(crate) const NODE_ROLE_HEADER: &str = "x-rolter-node-role";
pub(crate) const NODE_BUILD_HEADER: &str = "x-rolter-node-build";
/// Response header telling the polling node its operator-requested state, so a
/// drain takes effect on the channel the node already listens on.
pub(crate) const NODE_STATE_HEADER: &str = "x-rolter-node-state";

/// A node is considered live if it polled within this window. The default
/// snapshot poll period is measured in seconds, so a node that misses several
/// polls in a row is stale rather than merely slow.
const LIVENESS_WINDOW_SECS: i64 = 60;

/// A gateway that has not polled for this long is no longer part of the fleet
/// the summary describes. A rollout leaves the old pods' `HOSTNAME`-based ids
/// behind in the inventory, so without a horizon the summary would count them
/// `stale` for as long as nobody forgets them and read degraded after every
/// deploy. Inside the horizon a silent node is `stale`, which is what a crashed
/// or partitioned gateway looks like; beyond it, it is a node that went away.
const SUMMARY_HORIZON_SECS: i64 = 15 * 60;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/cluster/nodes", get(list_nodes))
        .route(
            "/api/v1/cluster/nodes/{id}",
            axum::routing::delete(forget_node),
        )
        .route("/api/v1/cluster/nodes/{id}/drain", put(set_node_drain))
}

/// The routes that must answer even where the rest of this module is not
/// mounted (a control plane with no database), so a reader gets an explicit
/// `inventory: false` instead of a `404` it cannot tell from an old build.
pub(crate) fn summary_router() -> Router<ControlState> {
    Router::new().route("/api/v1/cluster/summary", get(cluster_summary))
}

/// Whether a node is still polling, and whether it runs the current config.
#[derive(Debug, Serialize)]
struct ClusterNodeView {
    #[serde(flatten)]
    node: ClusterNode,
    /// true when the node polled inside the liveness window
    live: bool,
    /// true when the node reported the control plane's current config version,
    /// and was last served it in this control plane's own snapshot format
    converged: bool,
}

/// Whether `node` runs all of the current config: it reported the current
/// `config_version`, and was not last served an older snapshot format, whose
/// down-steps may have left out what that format cannot carry (ADR-0038,
/// #3351). A node with no recorded format (refused one, or not seen since the
/// column existed) is judged on its version alone: a refused node holds an
/// older version as soon as the config moves.
fn converged(node: &ClusterNode, current_version: i64) -> bool {
    let current_format = i32::try_from(rolter_core::snapshot_format::SNAPSHOT_FORMAT).ok();
    node.config_version >= current_version
        && node
            .served_format
            .is_none_or(|served| current_format.is_none_or(|current| served >= current))
}

fn view(node: ClusterNode, now: DateTime<Utc>, current_version: i64) -> ClusterNodeView {
    let live = is_live(&node, now);
    let converged = converged(&node, current_version);
    ClusterNodeView {
        node,
        live,
        converged,
    }
}

/// Sanitize a node-supplied header value. Identity comes from a node holding
/// the internal token, but the value still reaches the database and the
/// dashboard, so it is length-bounded and stripped of control characters.
fn header_value(headers: &HeaderMap, name: &str, max_len: usize) -> Option<String> {
    let raw = headers.get(name)?.to_str().ok()?.trim();
    if raw.is_empty() || raw.len() > max_len || raw.chars().any(char::is_control) {
        return None;
    }
    Some(raw.to_string())
}

/// Record a snapshot poll as a node sighting. Silent when the caller sends no
/// identity (an anonymous poller stays out of the inventory) and best-effort
/// otherwise: a bookkeeping failure must never fail config propagation.
/// Record a poll and return the state the node should be in, if any.
///
/// `formats` is the snapshot format range the poll stated, read as format 1
/// when it stated none and `None` when the statement did not parse;
/// `served_format` is the format it is answered in, `None` for a refusal
/// (ADR-0038, #3351).
pub(crate) async fn record_heartbeat(
    state: &ControlState,
    headers: &HeaderMap,
    reported_version: Option<i64>,
    formats: Option<rolter_core::snapshot_format::FormatRange>,
    served_format: Option<u32>,
) -> Option<String> {
    let pool = state.pool.as_ref()?;
    // the poll itself must still succeed — config propagation is not worth
    // failing over a bookkeeping header — but the drop is said out loud, since
    // an anonymous poller is an inventory that never fills (#1644)
    let Some(id) = header_value(headers, NODE_ID_HEADER, 128) else {
        tracing::debug!(
            header = NODE_ID_HEADER,
            "snapshot poll carries no usable node identity; not recording a cluster sighting"
        );
        return None;
    };
    let role = match header_value(headers, NODE_ROLE_HEADER, 16).as_deref() {
        Some("control") => "control",
        // an unrecognized role is recorded as a gateway rather than dropped;
        // the inventory is more useful with the node than without it
        _ => "gateway",
    };
    let build = header_value(headers, NODE_BUILD_HEADER, 64).unwrap_or_default();
    // a format past i32 is not one any build writes; it is recorded as unknown
    let formats = formats.and_then(|range| {
        Some((
            i32::try_from(range.min).ok()?,
            i32::try_from(range.max).ok()?,
        ))
    });
    let served_format = served_format.and_then(|format| i32::try_from(format).ok());
    match ClusterNodeRepo(pool)
        .heartbeat(
            &id,
            role,
            &build,
            reported_version.unwrap_or_default(),
            formats,
            served_format,
        )
        .await
    {
        Ok(node) => Some(node.desired_state),
        Err(err) => {
            tracing::warn!(error = %err, node = %id, "failed to record cluster node heartbeat");
            None
        }
    }
}

/// The polling node was told a format it would be served, and then could not
/// be: the config holds what that format cannot express. Best-effort, like the
/// heartbeat it corrects.
pub(crate) async fn record_format_refused(state: &ControlState, headers: &HeaderMap) {
    let (Some(pool), Some(id)) = (
        state.pool.as_ref(),
        header_value(headers, NODE_ID_HEADER, 128),
    ) else {
        return;
    };
    if let Err(err) = ClusterNodeRepo(pool).clear_served_format(&id).await {
        tracing::warn!(error = %err, node = %id, "failed to record a refused snapshot format");
    }
}

#[derive(Debug, Deserialize)]
struct SetNodeDrain {
    /// true removes the node from service, false returns it
    draining: bool,
}

/// Drain or un-drain a node. Draining the only live gateway would take the
/// data plane offline, so that is refused rather than confirmed — an operator
/// who means it can forget the node or scale up first.
async fn set_node_drain(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
    SafeJson(body): SafeJson<SetNodeDrain>,
) -> ApiResult<Json<ClusterNodeView>> {
    authorize_superadmin(&principal, superadmin_cap!("cluster_node", Update))?;
    let repo = ClusterNodeRepo(pool(&state));
    let nodes = repo.list().await?;
    let now = Utc::now();
    if body.draining && last_serving_gateway(&nodes, &id, now) {
        return Err(ApiError::Core(Error::Config(format!(
            "node {id} is the only live gateway still serving; draining it would take the data plane offline"
        ))));
    }
    let desired_state = if body.draining { "draining" } else { "active" };
    let node = repo.set_desired_state(&id, desired_state).await?;
    let actor = principal.account_id();
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "cluster_node.set_drain",
            Some("cluster_node"),
            None,
            Some(serde_json::json!({ "node_id": id, "desired_state": desired_state })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write cluster node audit log");
    }
    let current_version = state.store.current_version().await.unwrap_or_default();
    Ok(Json(view(node, now, current_version)))
}

/// Whether `id` is the last gateway that is both live and not already draining.
fn last_serving_gateway(nodes: &[ClusterNode], id: &str, now: DateTime<Utc>) -> bool {
    let serving: Vec<&ClusterNode> = nodes
        .iter()
        .filter(|n| n.role == "gateway" && n.desired_state != "draining" && is_live(n, now))
        .collect();
    serving.len() == 1 && serving[0].id == id
}

fn is_live(node: &ClusterNode, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(node.last_seen_at) <= Duration::seconds(LIVENESS_WINDOW_SECS)
}

/// Whether `node` is a gateway that polled inside the liveness window.
pub(crate) fn is_live_gateway(node: &ClusterNode, now: DateTime<Utc>) -> bool {
    node.role == "gateway" && is_live(node, now)
}

/// Counts of the gateway fleet, with no node identities.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
struct GatewayCounts {
    /// gateways that polled inside the liveness window
    live: usize,
    /// gateways that stopped polling within the last 15 minutes: down or
    /// partitioned, as opposed to scaled away
    stale: usize,
    /// live gateways that have not applied the current config version, or were
    /// last served an older snapshot format (#3351); a subset of `live`, so a
    /// healthy-looking pod running old config is visible
    lagging: usize,
}

#[derive(Debug, Serialize)]
struct ClusterSummary {
    /// false when there is no cluster inventory to count (no database): the
    /// numbers are then zero because nothing was counted, not because no
    /// gateway runs, and a reader should fall back to probing a gateway
    inventory: bool,
    gateways: GatewayCounts,
    /// the control plane's current config version
    config_version: i64,
    checked_at: DateTime<Utc>,
}

/// Roll the inventory up into counts. Control nodes are not gateways. A
/// gateway an operator has drained is out of service on purpose, so it is
/// counted nowhere: it should neither read as healthy capacity nor as a fault.
fn summarize(nodes: &[ClusterNode], now: DateTime<Utc>, current_version: i64) -> GatewayCounts {
    let mut counts = GatewayCounts::default();
    for node in nodes
        .iter()
        .filter(|n| n.role == "gateway" && n.desired_state != "draining")
    {
        if is_live(node, now) {
            counts.live += 1;
            if !converged(node, current_version) {
                counts.lagging += 1;
            }
        } else if now.signed_duration_since(node.last_seen_at)
            <= Duration::seconds(SUMMARY_HORIZON_SECS)
        {
            counts.stale += 1;
        }
    }
    counts
}

/// A read-only roll-up of the gateway fleet that every signed-in caller may
/// read (#2013). `GET /api/v1/cluster/nodes` names hosts and builds and is
/// superadmin-only; this carries counts and nothing that identifies a node, so
/// the header's gateway pill and the dashboard home can show a viewer whether
/// the fleet is up and converged.
async fn cluster_summary(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<ClusterSummary>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("cluster_summary", Read),
    )
    .await?;
    let current_version = state.store.current_version().await.unwrap_or_default();
    let now = Utc::now();
    let Some(pool) = state.pool.as_ref() else {
        return Ok(Json(ClusterSummary {
            inventory: false,
            gateways: GatewayCounts::default(),
            config_version: current_version,
            checked_at: now,
        }));
    };
    let nodes = ClusterNodeRepo(pool).list().await?;
    Ok(Json(ClusterSummary {
        inventory: true,
        gateways: summarize(&nodes, now, current_version),
        config_version: current_version,
        checked_at: now,
    }))
}

async fn list_nodes(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<ClusterNodeView>>> {
    authorize_superadmin(&principal, superadmin_cap!("cluster_node", Read))?;
    let nodes = ClusterNodeRepo(pool(&state)).list().await?;
    let current_version = state.store.current_version().await.unwrap_or_default();
    let now = Utc::now();
    Ok(Json(
        nodes
            .into_iter()
            .map(|node| view(node, now, current_version))
            .collect(),
    ))
}

/// Drop a decommissioned node from the inventory. A node that is still running
/// reappears on its next poll, so this cannot be used to silence a live node.
async fn forget_node(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("cluster_node", Delete))?;
    ClusterNodeRepo(pool(&state)).delete(&id).await?;
    let actor = principal.account_id();
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "cluster_node.forget",
            Some("cluster_node"),
            None,
            Some(serde_json::json!({ "node_id": id })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write cluster node audit log");
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn node(last_seen_at: DateTime<Utc>, config_version: i64) -> ClusterNode {
        named_node("gw-1", last_seen_at, config_version, "active")
    }

    fn named_node(
        id: &str,
        last_seen_at: DateTime<Utc>,
        config_version: i64,
        desired_state: &str,
    ) -> ClusterNode {
        ClusterNode {
            id: id.to_string(),
            role: "gateway".to_string(),
            build_version: "0.0.10".to_string(),
            config_version,
            desired_state: desired_state.to_string(),
            state_changed_at: last_seen_at,
            first_seen_at: last_seen_at,
            last_seen_at,
            formats_min: 1,
            formats_max: 1,
            served_format: Some(1),
        }
    }

    /// #3351: a node on the current config version that was served an older
    /// snapshot format may be missing what that format cannot carry, so it is
    /// not converged. One with no recorded format is judged on its version.
    #[test]
    fn a_node_served_an_older_format_is_not_converged() {
        let now = Utc::now();
        let current = i32::try_from(rolter_core::snapshot_format::SNAPSHOT_FORMAT).unwrap();
        let mut served = node(now, 7);
        served.served_format = Some(current);
        assert!(converged(&served, 7));
        served.served_format = Some(current - 1);
        assert!(!converged(&served, 7));
        assert_eq!(summarize(&[served.clone()], now, 7).lagging, 1);
        served.served_format = None;
        assert!(converged(&served, 7));
        assert!(!converged(&served, 8));
    }

    #[test]
    fn the_summary_counts_gateways_by_liveness_and_convergence() {
        let now = Utc::now();
        let nodes = vec![
            // live and converged
            named_node("gw-1", now, 7, "active"),
            // live but running the previous snapshot
            named_node("gw-2", now, 6, "active"),
            // silent for two minutes: stale, not live
            named_node("gw-3", now - Duration::seconds(120), 7, "active"),
        ];
        assert_eq!(
            summarize(&nodes, now, 7),
            GatewayCounts {
                live: 2,
                stale: 1,
                lagging: 1
            }
        );
    }

    #[test]
    fn a_node_gone_longer_than_the_horizon_is_dropped_not_stale() {
        let now = Utc::now();
        // a pod from before the last rollout: its HOSTNAME id never polls again
        let rollout_leftover = named_node(
            "gw-old",
            now - Duration::seconds(SUMMARY_HORIZON_SECS + 1),
            7,
            "active",
        );
        assert_eq!(
            summarize(&[rollout_leftover], now, 7),
            GatewayCounts::default()
        );
    }

    #[test]
    fn control_nodes_and_drained_gateways_are_not_counted() {
        let now = Utc::now();
        let mut control = named_node("ctl-1", now, 7, "active");
        control.role = "control".to_string();
        let drained = named_node("gw-1", now, 7, "draining");
        assert_eq!(
            summarize(&[control, drained], now, 7),
            GatewayCounts::default()
        );
    }

    /// A control plane with no database has no inventory to count, and has to
    /// say so rather than answer `404` (which a reader cannot tell from an old
    /// build) or a zero it did not count.
    #[tokio::test]
    async fn without_a_database_the_summary_says_it_counted_nothing() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt as _;

        let app = crate::build_app_with(crate::tests::state_with_token(Some("tok")), true);
        let response = app
            .oneshot(
                Request::get("/api/v1/cluster/summary")
                    .header("authorization", "Bearer tok")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body");
        let view: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(view["inventory"], false);
        assert_eq!(view["gateways"]["live"], 0);
    }

    #[test]
    fn draining_the_last_serving_gateway_is_refused() {
        let now = Utc::now();
        let only = vec![named_node("gw-1", now, 1, "active")];
        assert!(last_serving_gateway(&only, "gw-1", now));

        // a second live gateway makes the drain safe
        let pair = vec![
            named_node("gw-1", now, 1, "active"),
            named_node("gw-2", now, 1, "active"),
        ];
        assert!(!last_serving_gateway(&pair, "gw-1", now));

        // a dead or already-draining sibling does not count as cover
        let with_dead = vec![
            named_node("gw-1", now, 1, "active"),
            named_node("gw-2", now - Duration::seconds(300), 1, "active"),
            named_node("gw-3", now, 1, "draining"),
        ];
        assert!(last_serving_gateway(&with_dead, "gw-1", now));
    }

    #[test]
    fn liveness_and_convergence_are_reported_separately() {
        let now = Utc::now();
        // a node polling now on the current version
        let fresh = view(node(now, 7), now, 7);
        assert!(fresh.live && fresh.converged);
        // still polling, but has not applied the newest snapshot yet
        let lagging = view(node(now, 6), now, 7);
        assert!(lagging.live && !lagging.converged);
        // stopped polling while on the current version: dead, not converged-safe
        let stale = view(node(now - Duration::seconds(120), 7), now, 7);
        assert!(!stale.live && stale.converged);
    }

    #[test]
    fn node_headers_are_bounded_and_control_free() {
        let mut headers = HeaderMap::new();
        headers.insert(NODE_ID_HEADER, HeaderValue::from_static("gw-1"));
        assert_eq!(
            header_value(&headers, NODE_ID_HEADER, 128).as_deref(),
            Some("gw-1")
        );
        // over-long values are dropped rather than truncated into a new identity
        assert_eq!(header_value(&headers, NODE_ID_HEADER, 2), None);
        // and a missing header is simply absent
        assert_eq!(header_value(&headers, NODE_ROLE_HEADER, 16), None);
    }
}
