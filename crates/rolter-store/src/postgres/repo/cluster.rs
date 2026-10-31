//! Cluster nodes and adaptive routing telemetry.

use sqlx::PgPool;

use rolter_core::Result;

use super::super::models::{AdaptiveRoutingTelemetry, BreakerState, ClusterNode};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Cluster inventory: one row per gateway/control node, upserted from the
/// snapshot poll each node already performs.
pub struct ClusterNodeRepo<'a>(pub &'a PgPool);

impl ClusterNodeRepo<'_> {
    /// Record a node sighting. `first_seen_at` is preserved across restarts of
    /// the same node id so an operator can tell a flapping node from a new one.
    ///
    /// `formats` is the snapshot format range the poll stated (ADR-0038,
    /// #3351): `None` when the statement did not parse, which leaves the range
    /// already recorded. `served_format` is the format the poll was answered
    /// in, `None` when it was refused one.
    pub async fn heartbeat(
        &self,
        id: &str,
        role: &str,
        build_version: &str,
        config_version: i64,
        formats: Option<(i32, i32)>,
        served_format: Option<i32>,
    ) -> Result<ClusterNode> {
        sqlx::query_as(
            "insert into cluster_nodes \
                (id, role, build_version, config_version, formats_min, formats_max, \
                 served_format) \
             values ($1, $2, $3, $4, coalesce($5, 1), coalesce($6, 1), $7) \
             on conflict (id) do update set \
                role = excluded.role, build_version = excluded.build_version, \
                config_version = excluded.config_version, \
                formats_min = coalesce($5, cluster_nodes.formats_min), \
                formats_max = coalesce($6, cluster_nodes.formats_max), \
                served_format = excluded.served_format, last_seen_at = now() \
             returning id, role, build_version, config_version, desired_state, \
                state_changed_at, first_seen_at, last_seen_at, formats_min, formats_max, \
                served_format",
        )
        .bind(id)
        .bind(role)
        .bind(build_version)
        .bind(config_version)
        .bind(formats.map(|(min, _)| min))
        .bind(formats.map(|(_, max)| max))
        .bind(served_format)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Record that the format a node was just told it would be served could not
    /// express the config after all (`snapshot_format_inexpressible`), so it
    /// was served nothing.
    pub async fn clear_served_format(&self, id: &str) -> Result<()> {
        sqlx::query("update cluster_nodes set served_format = null where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<ClusterNode>> {
        sqlx::query_as(
            "select id, role, build_version, config_version, desired_state, \
                    state_changed_at, first_seen_at, last_seen_at, formats_min, formats_max, \
                    served_format \
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
                       state_changed_at, first_seen_at, last_seen_at, formats_min, \
                       formats_max, served_format",
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
    /// the org that owns the route, empty for a deployment-wide route (#3458)
    pub org_id: &'a str,
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
        let orgs: Vec<&str> = samples.iter().map(|s| s.org_id).collect();
        let models: Vec<&str> = samples.iter().map(|s| s.model).collect();
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query(
            "delete from adaptive_routing_telemetry t \
             where t.node_id = $1 \
               and not exists (select 1 from unnest($2::text[], $3::text[]) as k(org_id, model) \
                                where k.org_id = t.org_id and k.model = t.model)",
        )
        .bind(node_id)
        .bind(&orgs)
        .bind(&models)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        for sample in samples {
            sqlx::query(
                "insert into adaptive_routing_telemetry \
                    (node_id, model, engaged, observed, blend_picks, exploration_picks, \
                     fallback_picks, policy, targets, org_id, reported_at) \
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now()) \
                 on conflict (node_id, org_id, model) do update set \
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
            .bind(sample.org_id)
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
            "select node_id, org_id, model, engaged, observed, blend_picks, exploration_picks, \
                    fallback_picks, policy, targets, reported_at \
             from adaptive_routing_telemetry \
             where reported_at > now() - make_interval(secs => $1::double precision) \
             order by model, org_id, node_id",
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

/// Circuit-breaker state: what each gateway node has most recently reported
/// for each `(model, target)` it holds a breaker entry for (#2254).
pub struct BreakerStateRepo<'a>(pub &'a PgPool);

/// One target's breaker as a node reports it.
pub struct BreakerSample<'a> {
    /// the org that owns the route, empty for a deployment-wide route (#3458)
    pub org_id: &'a str,
    pub model: &'a str,
    pub target_index: i32,
    pub provider: &'a str,
    pub upstream_model: &'a str,
    pub state: &'a str,
    pub consecutive_failures: i32,
    pub retry_after_secs: Option<i32>,
}

impl BreakerStateRepo<'_> {
    /// Replace everything `node_id` has reported with `samples`, atomically, so
    /// a breaker that has closed (and left the node's registry) disappears in
    /// the same transaction that records the rest.
    pub async fn report(&self, node_id: &str, samples: &[BreakerSample<'_>]) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from breaker_state where node_id = $1")
            .bind(node_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        for sample in samples {
            sqlx::query(
                "insert into breaker_state \
                    (node_id, model, target_index, provider, upstream_model, state, \
                     consecutive_failures, retry_after_secs, org_id, reported_at) \
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
                 on conflict (node_id, org_id, model, target_index) do update set \
                    provider = excluded.provider, upstream_model = excluded.upstream_model, \
                    state = excluded.state, \
                    consecutive_failures = excluded.consecutive_failures, \
                    retry_after_secs = excluded.retry_after_secs, reported_at = now()",
            )
            .bind(node_id)
            .bind(sample.model)
            .bind(sample.target_index)
            .bind(sample.provider)
            .bind(sample.upstream_model)
            .bind(sample.state)
            .bind(sample.consecutive_failures)
            .bind(sample.retry_after_secs)
            .bind(sample.org_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)
    }

    /// Every row reported within `max_age_secs`. Older rows are never served as
    /// if they were current.
    pub async fn list_fresh(&self, max_age_secs: i64) -> Result<Vec<BreakerState>> {
        sqlx::query_as(
            "select node_id, org_id, model, target_index, provider, upstream_model, state, \
                    consecutive_failures, retry_after_secs, reported_at \
             from breaker_state \
             where reported_at > now() - make_interval(secs => $1::double precision) \
             order by model, org_id, target_index, node_id",
        )
        .bind(max_age_secs as f64)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Drop rows older than `max_age_secs`, so a node that is scaled down
    /// eventually leaves the view instead of lingering forever.
    pub async fn prune(&self, max_age_secs: i64) -> Result<u64> {
        let res = sqlx::query(
            "delete from breaker_state \
             where reported_at < now() - make_interval(secs => $1::double precision)",
        )
        .bind(max_age_secs as f64)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected())
    }
}
