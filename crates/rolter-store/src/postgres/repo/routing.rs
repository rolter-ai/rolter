//! Routes and their targets.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{Route, RouteTarget, RouteVariant, RouteVariantTarget};
use super::providers::ProviderRepo;
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Every column of `route_targets` that its row type reads.
const ROUTE_TARGET_COLUMNS: &str =
    "id, route_id, provider_id, upstream_model, weight, takes_over, created_at";

/// Every column of `route_variants` that its row type reads.
const ROUTE_VARIANT_COLUMNS: &str = "id, route_id, name, weight, position, params, created_at";

/// Every column of `route_variant_targets` that its row type reads.
const ROUTE_VARIANT_TARGET_COLUMNS: &str =
    "id, variant_id, provider_id, upstream_model, weight, position, takes_over";

/// Every column of `routes` that its row type reads.
const ROUTE_COLUMNS: &str =
    "id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at";

/// Routes, scoped to a project.
pub struct RouteRepo<'a>(pub &'a PgPool);

impl RouteRepo<'_> {
    /// How many orgs have a route answering to `model`.
    pub async fn orgs_holding_model(&self, model: &str) -> Result<i64> {
        sqlx::query_scalar(
            "select count(distinct t.org_id) from routes r \
               join projects p on p.id = r.project_id \
               join teams t on t.id = p.team_id \
              where r.model = $1",
        )
        .bind(model)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether a route of `org_id` already answers to `model`, in any of the
    /// org's projects. The gateway names routes by `(org, model)` (#1857,
    /// ADR-0040), so another org may hold the same name, but a key resolves a
    /// name per org, not per project, so two projects of one org may not.
    /// `None` asks across every org.
    pub async fn model_in_use(&self, model: &str, org_id: Option<Uuid>) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from routes r \
               join projects p on p.id = r.project_id \
               join teams t on t.id = p.team_id \
              where r.model = $1 and ($2::uuid is null or t.org_id = $2))",
        )
        .bind(model)
        .bind(org_id)
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
            for (target, spec) in plan.reweight {
                let updated: RouteTarget = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "update route_targets set weight = $2, takes_over = $3 where id = $1
                     returning {ROUTE_TARGET_COLUMNS}"
                )))
                .bind(target.id)
                .bind(spec.weight)
                .bind(&spec.takes_over)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                changes.reweighted.push(ReweightedTarget {
                    previous_weight: target.weight,
                    previous_takes_over: target.takes_over.clone(),
                    target: updated,
                });
            }
            for spec in plan.insert {
                // `clock_timestamp()` rather than the column's `now()` default,
                // which is fixed for the whole transaction: distinct times keep
                // the new targets listed in the order the caller sent them
                let created: RouteTarget = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "insert into route_targets (route_id, \
                        provider_id, upstream_model, weight, takes_over, created_at)
                     values ($1, $2, $3, $4, $5, clock_timestamp())
                     returning {ROUTE_TARGET_COLUMNS}"
                )))
                .bind(id)
                .bind(spec.provider_id)
                .bind(spec.upstream_model.as_deref())
                .bind(spec.weight)
                .bind(&spec.takes_over)
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

    /// Delete the routes serving `model` in `org_id`, whichever of its
    /// projects they sit in; `None` deletes them in every org. Names are per
    /// org (#1857), so a caller deleting one org's model names the org.
    /// Returns how many routes were removed.
    pub async fn delete_by_model(&self, model: &str, org_id: Option<Uuid>) -> Result<u64> {
        let res = sqlx::query(
            "delete from routes r using projects p, teams t \
              where p.id = r.project_id and t.id = p.team_id \
                and r.model = $1 and ($2::uuid is null or t.org_id = $2)",
        )
        .bind(model)
        .bind(org_id)
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
            "select t.id, t.route_id, t.provider_id, t.upstream_model, t.weight, t.takes_over,
                    t.created_at
             from route_targets t join routes r on r.id = t.route_id
             where r.project_id = $1 order by t.route_id, t.created_at, t.id",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The `(org, provider name)` of every provider a route in these teams or
    /// projects targets, directly or through a variant (#391), once each. Bounded by the scopes a caller holds a role
    /// in, like `ProjectRepo::viewer_payload_projects`, so provider health can
    /// follow the routes a project member actually uses (#1833). Nothing about
    /// the provider but its name leaves here.
    pub async fn targeted_providers(
        &self,
        team_ids: &[Uuid],
        project_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, String)>> {
        if team_ids.is_empty() && project_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as(
            "select org_id, name from (
                 select p.org_id, p.name
                 from route_targets t
                 join routes r on r.id = t.route_id
                 join projects pr on pr.id = r.project_id
                 join providers p on p.id = t.provider_id
                 where r.project_id = any($1) or pr.team_id = any($2)
                 union
                 select p.org_id, p.name
                 from route_variant_targets t
                 join route_variants v on v.id = t.variant_id
                 join routes r on r.id = v.route_id
                 join projects pr on pr.id = r.project_id
                 join providers p on p.id = t.provider_id
                 where r.project_id = any($1) or pr.team_id = any($2)
             ) targeted order by org_id, name",
        )
        .bind(project_ids)
        .bind(team_ids)
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
        takes_over: &[String],
    ) -> Result<RouteTarget> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into route_targets (route_id, provider_id, upstream_model, weight, takes_over)
             values ($1, $2, $3, $4, $5)
             returning {ROUTE_TARGET_COLUMNS}"
        )))
        .bind(route_id)
        .bind(provider_id)
        .bind(upstream_model)
        .bind(weight)
        .bind(takes_over)
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

    /// Replace the failure classes an existing target takes over (#1584), so
    /// a re-imported `rolter.toml` applies an edited `takes_over` the way it
    /// applies an edited weight.
    pub async fn set_takes_over(&self, id: Uuid, takes_over: &[String]) -> Result<RouteTarget> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update route_targets set takes_over = $2 where id = $1
             returning {ROUTE_TARGET_COLUMNS}"
            )))
            .bind(id)
            .bind(takes_over),
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

/// The weighted variants of a route (#391), scoped to a route.
pub struct RouteVariantRepo<'a>(pub &'a PgPool);

impl RouteVariantRepo<'_> {
    /// The variants of `route_id` in declared order, each with its targets in
    /// routing order.
    pub async fn list(&self, route_id: Uuid) -> Result<Vec<RouteVariant>> {
        let mut conn = self.0.acquire().await.map_err(store_err)?;
        variants_of(&mut conn, route_id).await
    }

    /// Every variant of every route in `project_id`, in two reads, for the
    /// routes list's `?include=variants`. Grouped by route and in declared
    /// order inside it, the order [`RouteVariantRepo::list`] gives.
    pub async fn list_for_project(&self, project_id: Uuid) -> Result<Vec<RouteVariant>> {
        let variants: Vec<RouteVariant> = sqlx::query_as(
            "select v.id, v.route_id, v.name, v.weight, v.position, v.params, v.created_at
             from route_variants v join routes r on r.id = v.route_id
             where r.project_id = $1 order by v.route_id, v.position",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        let targets: Vec<RouteVariantTarget> = sqlx::query_as(
            "select t.id, t.variant_id, t.provider_id, t.upstream_model, t.weight, t.position,
                    t.takes_over
             from route_variant_targets t
             join route_variants v on v.id = t.variant_id
             join routes r on r.id = v.route_id
             where r.project_id = $1 order by t.variant_id, t.position",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(attach_variant_targets(variants, targets))
    }

    /// Make `desired` the route's whole variant set, in one transaction, so a
    /// gateway polling `/internal/snapshot` sees the old split or the new one
    /// and never a mix. An empty `desired` clears the variants, which puts the
    /// route back on its classic target pool.
    ///
    /// A set equal to the stored one (same names, weights, params and targets
    /// in the same order) writes nothing, so a re-import of an unchanged file
    /// neither bumps the config version nor churns the rows' ids. Otherwise
    /// the stored variants are replaced whole: a variant has no identity
    /// outside its route but its name, and nothing else references its row.
    /// Nothing here validates the values; the caller does.
    ///
    /// Returns the variants as stored afterwards and whether anything changed.
    pub async fn replace(
        &self,
        route_id: Uuid,
        desired: &[RouteVariantSpec],
    ) -> Result<(Vec<RouteVariant>, bool)> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        // the same lock `RouteRepo::update` takes, so two edits of one route's
        // variants serialise rather than interleaving their deletes and inserts
        let _: Uuid = super::support::fetch_scalar_or_not_found(
            sqlx::query_scalar("select id from routes where id = $1 for update").bind(route_id),
            &mut *tx,
            || format!("route {route_id}"),
        )
        .await?;
        let stored = variants_of(&mut tx, route_id).await?;
        if specs_match(&stored, desired) {
            tx.commit().await.map_err(store_err)?;
            return Ok((stored, false));
        }
        sqlx::query("delete from route_variants where route_id = $1")
            .bind(route_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        let mut written = Vec::new();
        for (position, spec) in desired.iter().enumerate() {
            let mut variant: RouteVariant = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "insert into route_variants (route_id, name, weight, position, params)
                 values ($1, $2, $3, $4, $5)
                 returning {ROUTE_VARIANT_COLUMNS}"
            )))
            .bind(route_id)
            .bind(&spec.name)
            .bind(spec.weight)
            .bind(position as i32)
            .bind(&spec.params)
            .fetch_one(&mut *tx)
            .await
            .map_err(store_err)?;
            for (position, target) in spec.targets.iter().enumerate() {
                let row: RouteVariantTarget = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "insert into route_variant_targets \
                        (variant_id, provider_id, upstream_model, weight, position, takes_over)
                     values ($1, $2, $3, $4, $5, $6)
                     returning {ROUTE_VARIANT_TARGET_COLUMNS}"
                )))
                .bind(variant.id)
                .bind(target.provider_id)
                .bind(target.upstream_model.as_deref())
                .bind(target.weight)
                .bind(position as i32)
                .bind(&target.takes_over)
                .fetch_one(&mut *tx)
                .await
                .map_err(store_err)?;
                variant.targets.push(row);
            }
            written.push(variant);
        }
        tx.commit().await.map_err(store_err)?;
        Ok((written, true))
    }
}

/// The variants of `route_id` with their targets, read on `conn` so
/// [`RouteVariantRepo::replace`] can read inside its own transaction.
async fn variants_of(conn: &mut sqlx::PgConnection, route_id: Uuid) -> Result<Vec<RouteVariant>> {
    let variants: Vec<RouteVariant> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {ROUTE_VARIANT_COLUMNS}
         from route_variants where route_id = $1 order by position"
    )))
    .bind(route_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_err)?;
    let targets: Vec<RouteVariantTarget> = sqlx::query_as(
        "select t.id, t.variant_id, t.provider_id, t.upstream_model, t.weight, t.position,
                    t.takes_over
         from route_variant_targets t join route_variants v on v.id = t.variant_id
         where v.route_id = $1 order by t.variant_id, t.position",
    )
    .bind(route_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(store_err)?;
    Ok(attach_variant_targets(variants, targets))
}

/// Deal `targets` (sorted by variant, then position) out to their variants.
fn attach_variant_targets(
    mut variants: Vec<RouteVariant>,
    targets: Vec<RouteVariantTarget>,
) -> Vec<RouteVariant> {
    let mut by_variant: std::collections::HashMap<Uuid, Vec<RouteVariantTarget>> =
        std::collections::HashMap::new();
    for target in targets {
        by_variant
            .entry(target.variant_id)
            .or_default()
            .push(target);
    }
    for variant in &mut variants {
        variant.targets = by_variant.remove(&variant.id).unwrap_or_default();
    }
    variants
}

/// Whether the stored variants already are `desired`, field for field and in
/// the same order.
fn specs_match(stored: &[RouteVariant], desired: &[RouteVariantSpec]) -> bool {
    stored.len() == desired.len()
        && stored.iter().zip(desired).all(|(row, spec)| {
            row.name == spec.name
                && row.weight == spec.weight
                && row.params == spec.params
                && row.targets.len() == spec.targets.len()
                && row.targets.iter().zip(&spec.targets).all(|(t, s)| {
                    t.provider_id == s.provider_id
                        && t.upstream_model == s.upstream_model
                        && t.weight == s.weight
                        && t.takes_over == s.takes_over
                })
        })
}

/// One variant as [`RouteVariantRepo::replace`] is asked to leave it.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteVariantSpec {
    pub name: String,
    pub weight: i32,
    /// variant-scoped default inference params, a json object
    pub params: serde_json::Value,
    /// in routing order
    pub targets: Vec<RouteVariantTargetSpec>,
}

/// One target of a [`RouteVariantSpec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteVariantTargetSpec {
    pub provider_id: Uuid,
    /// the model name sent upstream; `None` forwards the route's public name
    pub upstream_model: Option<String>,
    pub weight: i32,
    /// the failure class tokens the target takes over; empty makes it an
    /// ordinary member of the variant (#3373)
    pub takes_over: Vec<String>,
}

/// One target of a route as [`RouteRepo::update`] is asked to leave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTargetSpec {
    pub provider_id: Uuid,
    /// the model name sent upstream; `None` forwards the route's public name
    pub upstream_model: Option<String>,
    pub weight: i32,
    /// the failure class tokens the target takes over; empty makes it an
    /// ordinary member of the pool (#1584)
    pub takes_over: Vec<String>,
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

/// A stored target whose weight or `takes_over` set [`RouteRepo::update`]
/// changed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReweightedTarget {
    /// the target as stored afterwards
    #[serde(flatten)]
    pub target: RouteTarget,
    pub previous_weight: i32,
    pub previous_takes_over: Vec<String>,
}

/// What [`RouteRepo::update`] did to a route's targets. All three are empty
/// when the update carried no target list or the list matched what was stored.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct RouteTargetChanges {
    /// entries that matched no stored target, as inserted
    pub added: Vec<RouteTarget>,
    /// stored targets that matched an entry with a different weight or
    /// `takes_over` set
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
    /// matched stored targets whose weight or `takes_over` set changes, with
    /// the entry that changes them
    pub(super) reweight: Vec<(&'a RouteTarget, &'a RouteTargetSpec)>,
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
                if target.weight != spec.weight || target.takes_over != spec.takes_over {
                    plan.reweight.push((target, spec));
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
