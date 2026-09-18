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
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
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

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/cluster/nodes", get(list_nodes))
        .route(
            "/api/v1/cluster/nodes/{id}",
            axum::routing::delete(forget_node),
        )
        .route("/api/v1/cluster/nodes/{id}/drain", put(set_node_drain))
}

/// Whether a node is still polling, and whether it runs the current config.
#[derive(Debug, Serialize)]
struct ClusterNodeView {
    #[serde(flatten)]
    node: ClusterNode,
    /// true when the node polled inside the liveness window
    live: bool,
    /// true when the node reported the control plane's current config version
    converged: bool,
}

fn view(node: ClusterNode, now: DateTime<Utc>, current_version: i64) -> ClusterNodeView {
    let live = is_live(&node, now);
    let converged = node.config_version >= current_version;
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
pub(crate) async fn record_heartbeat(
    state: &ControlState,
    headers: &HeaderMap,
    reported_version: Option<i64>,
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
    match ClusterNodeRepo(pool)
        .heartbeat(&id, role, &build, reported_version.unwrap_or_default())
        .await
    {
        Ok(node) => Some(node.desired_state),
        Err(err) => {
            tracing::warn!(error = %err, node = %id, "failed to record cluster node heartbeat");
            None
        }
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
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
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
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
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
        }
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
