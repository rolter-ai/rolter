//! Routes and their targets.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{Route, RouteTarget};
use super::providers::ProviderRepo;
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Every column of `route_targets` that its row type reads.
const ROUTE_TARGET_COLUMNS: &str = "id, route_id, provider_id, upstream_model, weight, created_at";

/// Every column of `routes` that its row type reads.
const ROUTE_COLUMNS: &str =
    "id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at";

/// Routes, scoped to a project.
pub struct RouteRepo<'a>(pub &'a PgPool);

impl RouteRepo<'_> {
    /// Whether any route in the deployment already answers to `model`. The
    /// gateway keys routes by public name alone, across every org, so a
    /// second one would make the snapshot refuse (#1845); per-org names are
    /// #1857.
    pub async fn model_in_use(&self, model: &str) -> Result<bool> {
        sqlx::query_scalar("select exists(select 1 from routes where model = $1)")
            .bind(model)
            .fetch_one(self.0)
            .await
            .map_err(store_err)
    }

    /// Whether a route named `model` in `org_id` would sit on the
    /// `provider-slug/model` or `group-slug/model` address of a provider or
    /// provider group outside that org. `None` treats every org as another one.
    pub async fn name_takes_address_outside_org(
        &self,
        model: &str,
        org_id: Option<Uuid>,
    ) -> Result<bool> {
        match rolter_core::slug::address_slug(model) {
            Some(slug) => {
                ProviderRepo(self.0)
                    .address_slug_outside_org(slug, org_id)
                    .await
            }
            None => Ok(false),
        }
    }

    pub async fn list(&self, project_id: Uuid) -> Result<Vec<Route>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ROUTE_COLUMNS}
             from routes where project_id = $1 order by model"
        )))
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every enabled route on `strategy`, across projects. Used by the global
    /// routing policies to report which routes a change actually affects.
    pub async fn list_by_strategy(&self, strategy: &str) -> Result<Vec<Route>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ROUTE_COLUMNS}
             from routes where strategy = $1 and enabled order by model"
        )))
        .bind(strategy)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Route> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {ROUTE_COLUMNS} from routes where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("route {id}"),
        )
        .await
    }

    pub async fn create(&self, project_id: Uuid, model: &str, strategy: &str) -> Result<Route> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into routes (project_id, model, strategy) values ($1, $2, $3)
             returning {ROUTE_COLUMNS}"
        )))
        .bind(project_id)
        .bind(model)
        .bind(strategy)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Change the balancing strategy of an existing route.
    ///
    /// The bootstrap import needs this to make a re-imported `rolter.toml` the
    /// desired state rather than a first-write-wins snapshot (#927); the value
    /// is constrained by the database to the strategies a migration allows.
    pub async fn set_strategy(&self, id: Uuid, strategy: &str) -> Result<Route> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update routes set strategy = $2 where id = $1
             returning {ROUTE_COLUMNS}"
            )))
            .bind(id)
            .bind(strategy),
            self.0,
            || format!("route {id}"),
        )
        .await
    }

    pub async fn set_enabled(&self, id: Uuid, enabled: bool) -> Result<Route> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update routes set enabled = $2 where id = $1
             returning {ROUTE_COLUMNS}"
            )))
            .bind(id)
            .bind(enabled),
            self.0,
            || format!("route {id}"),
        )
        .await
    }

    /// Set a route's admin param defaults and override policy (both jsonb).
    /// `params` is an object of default inference params; `param_policy` is
    /// `{mode, allow, deny}`. Mirrors config `[routes.params]`/`[routes.param_policy]`.
    pub async fn set_params(
        &self,
        id: Uuid,
        params: &serde_json::Value,
        param_policy: &serde_json::Value,
    ) -> Result<Route> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update routes set params = $2, param_policy = $3 where id = $1
             returning {ROUTE_COLUMNS}"
            )))
            .bind(id)
            .bind(params)
            .bind(param_policy),
            self.0,
            || format!("route {id}"),
        )
        .await
    }

    /// Set the model-catalog configuration projected into gateway snapshots.
    pub async fn set_advanced(&self, id: Uuid, advanced: &serde_json::Value) -> Result<Route> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update routes set advanced = $2 where id = $1
             returning {ROUTE_COLUMNS}"
            )))
            .bind(id)
            .bind(advanced),
            self.0,
            || format!("route {id}"),
        )
        .await
    }

    /// Change any of a route's `enabled` flag, strategy and target list in one
    /// transaction, so a gateway polling `/internal/snapshot` sees the route as
    /// it was or as it is now and never a mix of the two.
    ///
    /// A field left `None` in `update` keeps its stored value. When
    /// `update.targets` is present it is the route's whole target list, matched
    /// against the stored targets by provider and upstream model, the natural
    /// key the `rolter.toml` import uses as well: a match keeps its id and
    /// `created_at` and takes the new weight, a stored target the list no longer
    /// names is deleted, and an entry with no match is inserted. Each entry
    /// claims at most one stored target, so a provider and model listed twice
    /// keeps two rows. Nothing here validates the values; the caller applies the
    /// same checks as route and target creation.
    ///
    /// Returns the route as stored afterwards and what happened to its targets.
    pub async fn update(
        &self,
        id: Uuid,
        update: &RouteUpdate<'_>,
    ) -> Result<(Route, RouteTargetChanges)> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        // `for update` rather than the weaker lock an update statement takes:
        // a target insert holds `for key share` on its route through the
        // foreign key, which only this mode conflicts with, so a concurrent
        // `POST .../targets` cannot slip a row in between the read of the
        // stored targets below and the commit
        let mut route: Route = fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {ROUTE_COLUMNS}
             from routes where id = $1 for update"
            )))
            .bind(id),
            &mut *tx,
            || format!("route {id}"),
        )
        .await?;

        if update.enabled.is_some() || update.strategy.is_some() {
            route = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update routes
                 set enabled = coalesce($2, enabled), strategy = coalesce($3, strategy)
                 where id = $1
                 returning {ROUTE_COLUMNS}"
            )))
            .bind(id)
            .bind(update.enabled)
            .bind(update.strategy)
            .fetch_one(&mut *tx)
            .await
            .map_err(store_err)?;
        }

        let mut changes = RouteTargetChanges::default();
        if let Some(desired) = update.targets {
            let stored: Vec<RouteTarget> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {ROUTE_TARGET_COLUMNS}
                 from route_targets where route_id = $1 order by created_at, id"
            )))
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(store_err)?;
            let plan = plan_target_changes(&stored, desired);

            if !plan.delete.is_empty() {
                let ids: Vec<Uuid> = plan.delete.iter().map(|t| t.id).collect();
                sqlx::query("delete from route_targets where id = any($1)")
                    .bind(&ids)
                    .execute(&mut *tx)
                    .await
                    .map_err(store_err)?;
            }
            for (target, weight) in plan.reweight {
                let updated: RouteTarget = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "update route_targets set weight = $2 where id = $1
                     returning {ROUTE_TARGET_COLUMNS}"
                )))
                .bind(target.id)
                .bind(weight)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                changes.reweighted.push(ReweightedTarget {
                    previous_weight: target.weight,
                    target: updated,
                });
            }
            for spec in plan.insert {
                // `clock_timestamp()` rather than the column's `now()` default,
                // which is fixed for the whole transaction: distinct times keep
                // the new targets listed in the order the caller sent them
                let created: RouteTarget = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "insert into route_targets (route_id, \
                        provider_id, upstream_model, weight, created_at)
                     values ($1, $2, $3, $4, clock_timestamp())
                     returning {ROUTE_TARGET_COLUMNS}"
                )))
                .bind(id)
                .bind(spec.provider_id)
                .bind(spec.upstream_model.as_deref())
                .bind(spec.weight)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                changes.added.push(created);
            }
            changes.removed = plan.delete.into_iter().cloned().collect();
        }

        tx.commit().await.map_err(store_err)?;
        Ok((route, changes))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from routes where id = $1").bind(id),
            self.0,
            || format!("route {id}"),
        )
        .await?;
        Ok(())
    }

    /// Delete every route serving `model` (a public model name can be routed
    /// from several projects). Returns how many routes were removed.
    pub async fn delete_by_model(&self, model: &str) -> Result<u64> {
        let res = sqlx::query("delete from routes where model = $1")
            .bind(model)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("model {model}")));
        }
        Ok(res.rows_affected())
    }
}

/// Route targets, scoped to a route.
pub struct RouteTargetRepo<'a>(pub &'a PgPool);

impl RouteTargetRepo<'_> {
    /// Every target of every route in `project_id`, in one read.
    ///
    /// The routes list embeds them (`?include=targets`) so a screen of N routes
    /// costs one query rather than N. Rows come back grouped by route and in
    /// creation order inside it, the order [`RouteTargetRepo::list`] gives.
    pub async fn list_for_project(&self, project_id: Uuid) -> Result<Vec<RouteTarget>> {
        sqlx::query_as(
            "select t.id, t.route_id, t.provider_id, t.upstream_model, t.weight, t.created_at
             from route_targets t join routes r on r.id = t.route_id
             where r.project_id = $1 order by t.route_id, t.created_at, t.id",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, route_id: Uuid) -> Result<Vec<RouteTarget>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ROUTE_TARGET_COLUMNS}
             from route_targets where route_id = $1 order by created_at"
        )))
        .bind(route_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<RouteTarget> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {ROUTE_TARGET_COLUMNS}
             from route_targets where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("route target {id}"),
        )
        .await
    }

    pub async fn create(
        &self,
        route_id: Uuid,
        provider_id: Uuid,
        upstream_model: Option<&str>,
        weight: i32,
    ) -> Result<RouteTarget> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into route_targets (route_id, provider_id, upstream_model, weight)
             values ($1, $2, $3, $4)
             returning {ROUTE_TARGET_COLUMNS}"
        )))
        .bind(route_id)
        .bind(provider_id)
        .bind(upstream_model)
        .bind(weight)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Change the weight of an existing target.
    ///
    /// The bootstrap import needs this so a re-imported `rolter.toml` whose
    /// weights were edited actually applies them (#927), instead of matching
    /// the target as "already there" and moving on.
    pub async fn set_weight(&self, id: Uuid, weight: i32) -> Result<RouteTarget> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update route_targets set weight = $2 where id = $1
             returning {ROUTE_TARGET_COLUMNS}"
            )))
            .bind(id)
            .bind(weight),
            self.0,
            || format!("route target {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from route_targets where id = $1").bind(id),
            self.0,
            || format!("route target {id}"),
        )
        .await?;
        Ok(())
    }
}

/// One target of a route as [`RouteRepo::update`] is asked to leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTargetSpec {
    pub provider_id: Uuid,
    /// the model name sent upstream; `None` forwards the route's public name
    pub upstream_model: Option<String>,
    pub weight: i32,
}

/// The parts of a route [`RouteRepo::update`] changes. A field left `None`
/// keeps the stored value.
#[derive(Debug, Clone, Copy, Default)]
pub struct RouteUpdate<'a> {
    pub enabled: Option<bool>,
    pub strategy: Option<&'a str>,
    /// the route's complete target list when present
    pub targets: Option<&'a [RouteTargetSpec]>,
}

/// A stored target whose weight [`RouteRepo::update`] changed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReweightedTarget {
    /// the target as stored afterwards
    #[serde(flatten)]
    pub target: RouteTarget,
    pub previous_weight: i32,
}

/// What [`RouteRepo::update`] did to a route's targets. All three are empty
/// when the update carried no target list or the list matched what was stored.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RouteTargetChanges {
    /// entries that matched no stored target, as inserted
    pub added: Vec<RouteTarget>,
    /// stored targets that matched an entry with a different weight
    pub reweighted: Vec<ReweightedTarget>,
    /// stored targets the list no longer names, as they were before deletion
    pub removed: Vec<RouteTarget>,
}

impl RouteTargetChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.reweighted.is_empty() && self.removed.is_empty()
    }
}

/// How a desired target list maps onto a route's stored targets.
#[derive(Debug, Default)]
pub(super) struct TargetPlan<'a> {
    /// matched stored targets whose weight changes, with the new weight
    pub(super) reweight: Vec<(&'a RouteTarget, i32)>,
    /// entries with no stored match, in the order they were given
    pub(super) insert: Vec<&'a RouteTargetSpec>,
    /// stored targets no entry matched
    pub(super) delete: Vec<&'a RouteTarget>,
}

/// Match `desired` against `stored` by provider and upstream model.
///
/// The key is the one the `rolter.toml` import matches targets on (#927): the
/// weight is what an operator edits, while a different provider or upstream
/// model is a different target. Each entry claims the first unclaimed stored
/// target with its key, so a route holding one provider and model twice keeps
/// both rows when the list names it twice and loses one when it names it once.
/// A match whose weight is unchanged is left alone, so it is neither written
/// nor reported.
pub(super) fn plan_target_changes<'a>(
    stored: &'a [RouteTarget],
    desired: &'a [RouteTargetSpec],
) -> TargetPlan<'a> {
    let mut claimed = vec![false; stored.len()];
    let mut plan = TargetPlan::default();
    for spec in desired {
        let found = stored.iter().enumerate().find(|(i, target)| {
            !claimed[*i]
                && target.provider_id == spec.provider_id
                && target.upstream_model == spec.upstream_model
        });
        match found {
            Some((i, target)) => {
                claimed[i] = true;
                if target.weight != spec.weight {
                    plan.reweight.push((target, spec.weight));
                }
            }
            None => plan.insert.push(spec),
        }
    }
    plan.delete = stored
        .iter()
        .zip(&claimed)
        .filter(|(_, claimed)| !**claimed)
        .map(|(target, _)| target)
        .collect();
    plan
}
