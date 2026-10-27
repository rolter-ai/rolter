//! Cluster nodes and adaptive routing telemetry.

use sqlx::PgPool;

use rolter_core::Result;

use super::super::models::{AdaptiveRoutingTelemetry, ClusterNode};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Cluster inventory: one row per gateway/control node, upserted from the
/// snapshot poll each node already performs.
pub struct ClusterNodeRepo<'a>(pub &'a PgPool);

impl ClusterNodeRepo<'_> {
    /// Record a node sighting. `first_seen_at` is preserved across restarts of
    /// the same node id so an operator can tell a flapping node from a new one.
    pub async fn heartbeat(
        &self,
        id: &str,
        role: &str,
        build_version: &str,
        config_version: i64,
    ) -> Result<ClusterNode> {
        sqlx::query_as(
            "insert into cluster_nodes (id, role, build_version, config_version) \
             values ($1, $2, $3, $4) \
             on conflict (id) do update set \
                role = excluded.role, build_version = excluded.build_version, \
                config_version = excluded.config_version, last_seen_at = now() \
             returning id, role, build_version, config_version, desired_state, \
                state_changed_at, first_seen_at, last_seen_at",
        )
        .bind(id)
        .bind(role)
        .bind(build_version)
        .bind(config_version)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self) -> Result<Vec<ClusterNode>> {
        sqlx::query_as(
            "select id, role, build_version, config_version, desired_state, \
                    state_changed_at, first_seen_at, last_seen_at \
             from cluster_nodes order by role, id",
        )
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Set the operator-requested state for a node. Returns the updated row so
    /// the caller can audit what actually changed.
    pub async fn set_desired_state(&self, id: &str, desired_state: &str) -> Result<ClusterNode> {
        fetch_optional_or_not_found(
            sqlx::query_as(
                "update cluster_nodes set desired_state = $2, state_changed_at = now() \
             where id = $1 \
             returning id, role, build_version, config_version, desired_state, \
                       state_changed_at, first_seen_at, last_seen_at",
            )
            .bind(id)
            .bind(desired_state),
            self.0,
            || format!("cluster node {id}"),
        )
        .await
    }

    /// The state a node should move to, read on its snapshot poll. `None` when
    /// the node is not in the inventory yet.
    pub async fn desired_state(&self, id: &str) -> Result<Option<String>> {
        sqlx::query_scalar("select desired_state from cluster_nodes where id = $1")
            .bind(id)
            .fetch_optional(self.0)
            .await
            .map_err(store_err)
    }

    /// Forget a node an operator has decommissioned. A node that is still
    /// running reappears on its next poll.
    pub async fn delete(&self, id: &str) -> Result<()> {
        require_affected(
            sqlx::query("delete from cluster_nodes where id = $1").bind(id),
            self.0,
            || format!("cluster node {id}"),
        )
        .await?;
        Ok(())
    }
}

/// Adaptive-routing telemetry: the newest sample each gateway node has pushed
/// for each route it balances adaptively (#751).
pub struct AdaptiveRoutingTelemetryRepo<'a>(pub &'a PgPool);

/// One route's sample as a node reports it. The `policy` and `targets`
/// documents are stored as sent: the control plane owns the API shape, and the
/// gateway owns what a target signal means.
pub struct AdaptiveRoutingSample<'a> {
    pub model: &'a str,
    pub engaged: bool,
    pub observed: i64,
    pub blend_picks: i64,
    pub exploration_picks: i64,
    pub fallback_picks: i64,
    pub policy: serde_json::Value,
    pub targets: serde_json::Value,
}

impl AdaptiveRoutingTelemetryRepo<'_> {
    /// Replace everything `node_id` has reported with `samples`, atomically.
    /// A route the node no longer balances adaptively disappears in the same
    /// transaction that records the rest, so the scoreboard cannot show a
    /// route that has been switched off or deleted.
    pub async fn report(&self, node_id: &str, samples: &[AdaptiveRoutingSample<'_>]) -> Result<()> {
        let models: Vec<String> = samples.iter().map(|s| s.model.to_string()).collect();
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query(
            "delete from adaptive_routing_telemetry \
             where node_id = $1 and model <> all($2)",
        )
        .bind(node_id)
        .bind(&models)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        for sample in samples {
            sqlx::query(
                "insert into adaptive_routing_telemetry \
                    (node_id, model, engaged, observed, blend_picks, exploration_picks, \
                     fallback_picks, policy, targets, reported_at) \
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
                 on conflict (node_id, model) do update set \
                    engaged = excluded.engaged, observed = excluded.observed, \
                    blend_picks = excluded.blend_picks, \
                    exploration_picks = excluded.exploration_picks, \
                    fallback_picks = excluded.fallback_picks, \
                    policy = excluded.policy, targets = excluded.targets, \
                    reported_at = now()",
            )
            .bind(node_id)
            .bind(sample.model)
            .bind(sample.engaged)
            .bind(sample.observed)
            .bind(sample.blend_picks)
            .bind(sample.exploration_picks)
            .bind(sample.fallback_picks)
            .bind(&sample.policy)
            .bind(&sample.targets)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)
    }

    /// Every sample reported within `max_age_secs`. Older rows are left in
    /// place — a node that stopped reporting is a fact the caller may want to
    /// see — but they are never served as if they were current.
    pub async fn list_fresh(&self, max_age_secs: i64) -> Result<Vec<AdaptiveRoutingTelemetry>> {
        sqlx::query_as(
            "select node_id, model, engaged, observed, blend_picks, exploration_picks, \
                    fallback_picks, policy, targets, reported_at \
             from adaptive_routing_telemetry \
             where reported_at > now() - make_interval(secs => $1::double precision) \
             order by model, node_id",
        )
        .bind(max_age_secs as f64)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Drop samples older than `max_age_secs`, so a node that is scaled down
    /// eventually leaves the scoreboard instead of lingering forever.
    pub async fn prune(&self, max_age_secs: i64) -> Result<u64> {
        let res = sqlx::query(
            "delete from adaptive_routing_telemetry \
             where reported_at < now() - make_interval(secs => $1::double precision)",
        )
        .bind(max_age_secs as f64)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected())
    }
}
