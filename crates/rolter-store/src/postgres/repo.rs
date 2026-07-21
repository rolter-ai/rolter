//! Thin CRUD repositories over the `postgres` feature's schema. Each
//! repository wraps a [`PgPool`] and exposes `list`/`get`/`create`/`delete`
//! (plus narrow `update`s where a resource has mutable fields worth editing
//! independently). Domain interpretation of row contents (e.g. balancer
//! strategy parsing) is left to callers; see [`super::PostgresConfigStore`]
//! for the read path the gateway uses.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::models::{
    AuditLogEntry, Budget, Membership, ModelPrice, Org, OwnedVirtualKey, Project, Provider,
    ProviderGroup, ProviderGroupMember, RateLimit, Route, RouteTarget, SecuritySettings, Session,
    Team, User, VirtualKey,
};

fn store_err(err: sqlx::Error) -> Error {
    Error::Store(err.to_string())
}

/// Orgs: the top of the org → team → project tenancy hierarchy.
pub struct OrgRepo<'a>(pub &'a PgPool);

impl OrgRepo<'_> {
    pub async fn list(&self) -> Result<Vec<Org>> {
        sqlx::query_as("select id, name, slug, created_at from orgs order by name")
            .fetch_all(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Org> {
        sqlx::query_as("select id, name, slug, created_at from orgs where id = $1")
            .bind(id)
            .fetch_optional(self.0)
            .await
            .map_err(store_err)?
            .ok_or_else(|| Error::NotFound(format!("org {id}")))
    }

    pub async fn create(&self, name: &str, slug: &str) -> Result<Org> {
        sqlx::query_as(
            "insert into orgs (name, slug) values ($1, $2)
             returning id, name, slug, created_at",
        )
        .bind(name)
        .bind(slug)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from orgs where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("org {id}")));
        }
        Ok(())
    }
}

/// Teams, scoped to an org.
pub struct TeamRepo<'a>(pub &'a PgPool);

impl TeamRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Team>> {
        sqlx::query_as(
            "select id, org_id, name, created_at from teams where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Team> {
        sqlx::query_as("select id, org_id, name, created_at from teams where id = $1")
            .bind(id)
            .fetch_optional(self.0)
            .await
            .map_err(store_err)?
            .ok_or_else(|| Error::NotFound(format!("team {id}")))
    }

    pub async fn create(&self, org_id: Uuid, name: &str) -> Result<Team> {
        sqlx::query_as(
            "insert into teams (org_id, name) values ($1, $2)
             returning id, org_id, name, created_at",
        )
        .bind(org_id)
        .bind(name)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from teams where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("team {id}")));
        }
        Ok(())
    }
}

/// Projects, scoped to a team.
pub struct ProjectRepo<'a>(pub &'a PgPool);

impl ProjectRepo<'_> {
    pub async fn list(&self, team_id: Uuid) -> Result<Vec<Project>> {
        sqlx::query_as(
            "select id, team_id, name, created_at from projects where team_id = $1 order by name",
        )
        .bind(team_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Project> {
        sqlx::query_as("select id, team_id, name, created_at from projects where id = $1")
            .bind(id)
            .fetch_optional(self.0)
            .await
            .map_err(store_err)?
            .ok_or_else(|| Error::NotFound(format!("project {id}")))
    }

    pub async fn create(&self, team_id: Uuid, name: &str) -> Result<Project> {
        sqlx::query_as(
            "insert into projects (team_id, name) values ($1, $2)
             returning id, team_id, name, created_at",
        )
        .bind(team_id)
        .bind(name)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from projects where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("project {id}")));
        }
        Ok(())
    }
}

/// Upstream providers, scoped to an org.
pub struct ProviderRepo<'a>(pub &'a PgPool);

impl ProviderRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Provider>> {
        sqlx::query_as(
            "select id, org_id, name, slug, kind, api_base, api_key_env, egress_proxy, egress_proxies, created_at
             from providers where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Provider> {
        sqlx::query_as(
            "select id, org_id, name, slug, kind, api_base, api_key_env, egress_proxy, egress_proxies, created_at
             from providers where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("provider {id}")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        kind: &str,
        api_base: &str,
        api_key_env: Option<&str>,
        egress_proxy: Option<&str>,
        egress_proxies: &[String],
    ) -> Result<Provider> {
        sqlx::query_as(
            "insert into providers (org_id, name, slug, kind, api_base, api_key_env, egress_proxy, egress_proxies)
             values ($1, $2, $3, $4, $5, $6, $7, $8)
             returning id, org_id, name, slug, kind, api_base, api_key_env, egress_proxy, egress_proxies, created_at",
        )
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(kind)
        .bind(api_base)
        .bind(api_key_env)
        .bind(egress_proxy)
        .bind(serde_json::json!(egress_proxies))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Partially update a provider. `None` leaves a field unchanged; the
    /// nullable fields take `Some(None)` to clear. `slug` is immutable by
    /// default — callers must only pass `Some` after an explicit override
    /// (the control API gates this); the charset constraint is enforced by the
    /// database.
    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: Uuid,
        slug: Option<&str>,
        kind: Option<&str>,
        api_base: Option<&str>,
        api_key_env: Option<Option<&str>>,
        egress_proxy: Option<Option<&str>>,
        egress_proxies: Option<&[String]>,
    ) -> Result<Provider> {
        sqlx::query_as(
            "update providers set
                 slug = coalesce($2, slug),
                 kind = coalesce($3, kind),
                 api_base = coalesce($4, api_base),
                 api_key_env = case when $5 then $6 else api_key_env end,
                 egress_proxy = case when $7 then $8 else egress_proxy end,
                 egress_proxies = case when $9 then $10 else egress_proxies end
             where id = $1
             returning id, org_id, name, slug, kind, api_base, api_key_env, egress_proxy, egress_proxies, created_at",
        )
        .bind(id)
        .bind(slug)
        .bind(kind)
        .bind(api_base)
        .bind(api_key_env.is_some())
        .bind(api_key_env.flatten())
        .bind(egress_proxy.is_some())
        .bind(egress_proxy.flatten())
        .bind(egress_proxies.is_some())
        .bind(egress_proxies.map(|v| serde_json::json!(v)))
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("provider {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from providers where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("provider {id}")));
        }
        Ok(())
    }
}

/// Runtime provider credentials, sealed with AES-256-GCM (see
/// [`super::crypto`]). One active key per provider; setting a new one
/// replaces the old in place.
pub struct ProviderKeyRepo<'a>(pub &'a PgPool);

impl ProviderKeyRepo<'_> {
    /// Store (or rotate) the sealed credential for `provider_id`.
    pub async fn set(&self, provider_id: Uuid, ciphertext: &[u8], nonce: &[u8]) -> Result<()> {
        sqlx::query(
            "insert into provider_keys (provider_id, ciphertext, nonce)
             values ($1, $2, $3)
             on conflict (provider_id)
             do update set ciphertext = excluded.ciphertext, nonce = excluded.nonce,
                           created_at = now()",
        )
        .bind(provider_id)
        .bind(ciphertext)
        .bind(nonce)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Remove the stored credential for `provider_id` (no-op when absent).
    pub async fn clear(&self, provider_id: Uuid) -> Result<()> {
        sqlx::query("delete from provider_keys where provider_id = $1")
            .bind(provider_id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// Whether a credential is stored for `provider_id`.
    pub async fn exists(&self, provider_id: Uuid) -> Result<bool> {
        sqlx::query_scalar("select exists(select 1 from provider_keys where provider_id = $1)")
            .bind(provider_id)
            .fetch_one(self.0)
            .await
            .map_err(store_err)
    }
}

/// Routes, scoped to a project.
pub struct RouteRepo<'a>(pub &'a PgPool);

impl RouteRepo<'_> {
    pub async fn list(&self, project_id: Uuid) -> Result<Vec<Route>> {
        sqlx::query_as(
            "select id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at
             from routes where project_id = $1 order by model",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Route> {
        sqlx::query_as(
            "select id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at from routes where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route {id}")))
    }

    pub async fn create(&self, project_id: Uuid, model: &str, strategy: &str) -> Result<Route> {
        sqlx::query_as(
            "insert into routes (project_id, model, strategy) values ($1, $2, $3)
             returning id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at",
        )
        .bind(project_id)
        .bind(model)
        .bind(strategy)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn set_enabled(&self, id: Uuid, enabled: bool) -> Result<Route> {
        sqlx::query_as(
            "update routes set enabled = $2 where id = $1
             returning id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at",
        )
        .bind(id)
        .bind(enabled)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route {id}")))
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
        sqlx::query_as(
            "update routes set params = $2, param_policy = $3 where id = $1
             returning id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at",
        )
        .bind(id)
        .bind(params)
        .bind(param_policy)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route {id}")))
    }

    /// Set the model-catalog configuration projected into gateway snapshots.
    pub async fn set_advanced(&self, id: Uuid, advanced: &serde_json::Value) -> Result<Route> {
        sqlx::query_as(
            "update routes set advanced = $2 where id = $1
             returning id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at",
        )
        .bind(id)
        .bind(advanced)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from routes where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("route {id}")));
        }
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
    pub async fn list(&self, route_id: Uuid) -> Result<Vec<RouteTarget>> {
        sqlx::query_as(
            "select id, route_id, provider_id, upstream_model, weight, created_at
             from route_targets where route_id = $1 order by created_at",
        )
        .bind(route_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<RouteTarget> {
        sqlx::query_as(
            "select id, route_id, provider_id, upstream_model, weight, created_at
             from route_targets where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route target {id}")))
    }

    pub async fn create(
        &self,
        route_id: Uuid,
        provider_id: Uuid,
        upstream_model: Option<&str>,
        weight: i32,
    ) -> Result<RouteTarget> {
        sqlx::query_as(
            "insert into route_targets (route_id, provider_id, upstream_model, weight)
             values ($1, $2, $3, $4)
             returning id, route_id, provider_id, upstream_model, weight, created_at",
        )
        .bind(route_id)
        .bind(provider_id)
        .bind(upstream_model)
        .bind(weight)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from route_targets where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("route target {id}")));
        }
        Ok(())
    }
}

/// Virtual keys, scoped to a project. Callers are responsible for hashing
/// the presented key before it reaches `key_hash` — this repository never
/// sees or stores plaintext keys.
pub struct VirtualKeyRepo<'a>(pub &'a PgPool);

impl VirtualKeyRepo<'_> {
    pub async fn list(&self, project_id: Uuid) -> Result<Vec<VirtualKey>> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at
             from virtual_keys where project_id = $1 order by created_at",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn find_by_hash(&self, key_hash: &str) -> Result<Option<VirtualKey>> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at
             from virtual_keys where key_hash = $1",
        )
        .bind(key_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<VirtualKey> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at
             from virtual_keys where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("virtual key {id}")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        project_id: Uuid,
        key_hash: &str,
        key_prefix: &str,
        name: Option<&str>,
        models: &[String],
        providers: &[String],
        cache_enabled: Option<bool>,
        created_by: Option<Uuid>,
    ) -> Result<VirtualKey> {
        sqlx::query_as(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name, models, providers, cache_enabled, created_by)
             values ($1, $2, $3, $4, $5, $6, $7, $8)
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at",
        )
        .bind(project_id)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(name)
        .bind(models)
        .bind(providers)
        .bind(cache_enabled)
        .bind(created_by)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// every key minted by `user_id` via the self-service panel, newest first,
    /// enriched with the owning project + org names. omits the key hash.
    pub async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<OwnedVirtualKey>> {
        sqlx::query_as(
            "select vk.id, vk.project_id, p.name as project_name, o.name as org_name,
                    vk.key_prefix, vk.name, vk.models, vk.disabled, vk.expires_at, vk.created_at
             from virtual_keys vk
             join projects p on p.id = vk.project_id
             join teams t on t.id = p.team_id
             join orgs o on o.id = t.org_id
             where vk.created_by = $1
             order by vk.created_at desc",
        )
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn set_disabled(&self, id: Uuid, disabled: bool) -> Result<VirtualKey> {
        sqlx::query_as(
            "update virtual_keys set disabled = $2 where id = $1
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at",
        )
        .bind(id)
        .bind(disabled)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("virtual key {id}")))
    }

    /// Set (or clear) the per-key response-cache override. `None` restores the
    /// inherit-the-route default; `Some(bool)` forces caching off/on for the key.
    pub async fn set_cache(&self, id: Uuid, cache_enabled: Option<bool>) -> Result<VirtualKey> {
        sqlx::query_as(
            "update virtual_keys set cache_enabled = $2 where id = $1
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at",
        )
        .bind(id)
        .bind(cache_enabled)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("virtual key {id}")))
    }

    /// Replace the key's provider allow-list. An empty list restores the
    /// permissive default while leaving the model allow-list unchanged.
    pub async fn set_providers(&self, id: Uuid, providers: &[String]) -> Result<VirtualKey> {
        sqlx::query_as(
            "update virtual_keys set providers = $2 where id = $1
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, created_at",
        )
        .bind(id)
        .bind(providers)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("virtual key {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from virtual_keys where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("virtual key {id}")));
        }
        Ok(())
    }
}

/// Budgets, attachable at any scope (org/team/project/virtual_key).
pub struct BudgetRepo<'a>(pub &'a PgPool);

impl BudgetRepo<'_> {
    pub async fn list_for_scope(&self, scope_type: &str, scope_id: Uuid) -> Result<Vec<Budget>> {
        sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, created_at
             from budgets where scope_type = $1 and scope_id = $2 order by created_at",
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Budget> {
        sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, created_at
             from budgets where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("budget {id}")))
    }

    pub async fn create(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        limit_usd: &str,
        period: &str,
    ) -> Result<Budget> {
        sqlx::query_as(
            "insert into budgets (scope_type, scope_id, limit_usd, period)
             values ($1, $2, $3::numeric, $4)
             returning id, scope_type, scope_id, limit_usd::text as limit_usd, period, created_at",
        )
        .bind(scope_type)
        .bind(scope_id)
        .bind(limit_usd)
        .bind(period)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from budgets where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("budget {id}")));
        }
        Ok(())
    }
}

/// Rate limits, attachable at any scope (org/team/project/virtual_key).
pub struct RateLimitRepo<'a>(pub &'a PgPool);

impl RateLimitRepo<'_> {
    pub async fn list_for_scope(&self, scope_type: &str, scope_id: Uuid) -> Result<Vec<RateLimit>> {
        sqlx::query_as(
            "select id, scope_type, scope_id, rpm, tpm, created_at
             from rate_limits where scope_type = $1 and scope_id = $2 order by created_at",
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<RateLimit> {
        sqlx::query_as(
            "select id, scope_type, scope_id, rpm, tpm, created_at
             from rate_limits where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("rate limit {id}")))
    }

    pub async fn create(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        rpm: Option<i32>,
        tpm: Option<i32>,
    ) -> Result<RateLimit> {
        sqlx::query_as(
            "insert into rate_limits (scope_type, scope_id, rpm, tpm)
             values ($1, $2, $3, $4)
             returning id, scope_type, scope_id, rpm, tpm, created_at",
        )
        .bind(scope_type)
        .bind(scope_id)
        .bind(rpm)
        .bind(tpm)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from rate_limits where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("rate limit {id}")));
        }
        Ok(())
    }
}

/// The pricing catalog (usd per million tokens), keyed by public model name.
pub struct ModelPriceRepo<'a>(pub &'a PgPool);

impl ModelPriceRepo<'_> {
    pub async fn list(&self) -> Result<Vec<ModelPrice>> {
        sqlx::query_as(
            "select id, model,
                    input_per_mtok::text as input_per_mtok,
                    output_per_mtok::text as output_per_mtok,
                    cached_input_per_mtok::text as cached_input_per_mtok,
                    currency, created_at
             from model_prices order by model",
        )
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn upsert(
        &self,
        model: &str,
        input_per_mtok: &str,
        output_per_mtok: &str,
        cached_input_per_mtok: Option<&str>,
        currency: &str,
    ) -> Result<ModelPrice> {
        sqlx::query_as(
            "insert into model_prices (model, input_per_mtok, output_per_mtok, cached_input_per_mtok, currency)
             values ($1, $2::numeric, $3::numeric, $4::numeric, $5)
             on conflict (model) do update
                set input_per_mtok = excluded.input_per_mtok,
                    output_per_mtok = excluded.output_per_mtok,
                    cached_input_per_mtok = excluded.cached_input_per_mtok,
                    currency = excluded.currency
             returning id, model,
                       input_per_mtok::text as input_per_mtok,
                       output_per_mtok::text as output_per_mtok,
                       cached_input_per_mtok::text as cached_input_per_mtok,
                       currency, created_at",
        )
        .bind(model)
        .bind(input_per_mtok)
        .bind(output_per_mtok)
        .bind(cached_input_per_mtok)
        .bind(currency)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, model: &str) -> Result<()> {
        let res = sqlx::query("delete from model_prices where model = $1")
            .bind(model)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("model price '{model}'")));
        }
        Ok(())
    }
}

/// local accounts. see [`super::models::User`]; `find_by_email` backs login.
pub struct UserRepo<'a>(pub &'a PgPool);

impl UserRepo<'_> {
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        sqlx::query_as(
            "select id, email, password_hash, is_superadmin, deactivated_at, created_at
             from users where email = $1",
        )
        .bind(email)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<User> {
        sqlx::query_as(
            "select id, email, password_hash, is_superadmin, deactivated_at, created_at
             from users where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("user {id}")))
    }

    /// list every user that holds at least one membership anywhere in `org_id`'s
    /// tree (org, its teams, or their projects). this is the admin-visible set of
    /// accounts for an org; superadmins with no org membership are not included.
    pub async fn list_in_org(&self, org_id: Uuid) -> Result<Vec<User>> {
        sqlx::query_as(
            "select distinct u.id, u.email, u.password_hash, u.is_superadmin,
                    u.deactivated_at, u.created_at
             from users u
             join memberships m on m.user_id = u.id
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             where m.org_id = $1 or t.org_id = $1 or pt.org_id = $1
             order by u.email",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// create a local account. `password_hash` is a pre-computed argon2id digest
    /// (the repo never sees plaintext); pass `None` for an sso-only shell account
    pub async fn create(
        &self,
        email: &str,
        password_hash: Option<&str>,
        is_superadmin: bool,
    ) -> Result<User> {
        sqlx::query_as(
            "insert into users (email, password_hash, is_superadmin)
             values ($1, $2, $3)
             returning id, email, password_hash, is_superadmin, deactivated_at, created_at",
        )
        .bind(email)
        .bind(password_hash)
        .bind(is_superadmin)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// update mutable account fields. each `Some` is applied via `coalesce`, so
    /// `None` leaves the stored value untouched. `password_hash` follows the same
    /// rule; there is no way to clear a password back to null through this path.
    pub async fn update(
        &self,
        id: Uuid,
        email: Option<&str>,
        password_hash: Option<&str>,
        is_superadmin: Option<bool>,
    ) -> Result<User> {
        sqlx::query_as(
            "update users set
                 email = coalesce($2, email),
                 password_hash = coalesce($3, password_hash),
                 is_superadmin = coalesce($4, is_superadmin)
             where id = $1
             returning id, email, password_hash, is_superadmin, deactivated_at, created_at",
        )
        .bind(id)
        .bind(email)
        .bind(password_hash)
        .bind(is_superadmin)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("user {id}")))
    }

    /// flip the deactivation flag. `true` stamps `deactivated_at = now()` (login
    /// blocked); `false` clears it back to null (re-enabled). the caller is
    /// responsible for deleting live sessions when deactivating.
    pub async fn set_deactivated(&self, id: Uuid, deactivated: bool) -> Result<User> {
        sqlx::query_as(
            "update users set deactivated_at = case when $2 then now() else null end
             where id = $1
             returning id, email, password_hash, is_superadmin, deactivated_at, created_at",
        )
        .bind(id)
        .bind(deactivated)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("user {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from users where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("user {id}")));
        }
        Ok(())
    }
}

/// role grants at an org/team/project scope. see [`super::models::Membership`].
pub struct MembershipRepo<'a>(pub &'a PgPool);

impl MembershipRepo<'_> {
    pub async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<Membership>> {
        sqlx::query_as(
            "select id, user_id, org_id, team_id, project_id, role, created_at
             from memberships where user_id = $1 order by created_at",
        )
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// every membership whose scope falls within `org_id`'s tree (an org-scoped
    /// grant, a grant on one of its teams, or on one of their projects), so an
    /// org admin can see and manage all role assignments under their org
    pub async fn list_in_org(&self, org_id: Uuid) -> Result<Vec<Membership>> {
        sqlx::query_as(
            "select m.id, m.user_id, m.org_id, m.team_id, m.project_id, m.role, m.created_at
             from memberships m
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             where m.org_id = $1 or t.org_id = $1 or pt.org_id = $1
             order by m.created_at",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Membership> {
        sqlx::query_as(
            "select id, user_id, org_id, team_id, project_id, role, created_at
             from memberships where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("membership {id}")))
    }

    /// grant `role` to `user_id` at exactly one scope. exactly one of
    /// `org_id`/`team_id`/`project_id` should be non-null (the most-specific
    /// scope id); enforcement of that invariant is left to the caller.
    pub async fn create(
        &self,
        user_id: Uuid,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
    ) -> Result<Membership> {
        sqlx::query_as(
            "insert into memberships (user_id, org_id, team_id, project_id, role)
             values ($1, $2, $3, $4, $5)
             returning id, user_id, org_id, team_id, project_id, role, created_at",
        )
        .bind(user_id)
        .bind(org_id)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from memberships where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("membership {id}")));
        }
        Ok(())
    }
}

/// login sessions backing bearer-token auth. see [`super::models::Session`]
/// and the rationale in `migrations/0013_sessions.sql` for why these are
/// stateful (postgres-backed) rather than a stateless jwt.
pub struct SessionRepo<'a>(pub &'a PgPool);

impl SessionRepo<'_> {
    pub async fn create(
        &self,
        user_id: Uuid,
        token_hash: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Session> {
        sqlx::query_as(
            "insert into sessions (user_id, token_hash, expires_at)
             values ($1, $2, $3)
             returning id, user_id, token_hash, created_at, expires_at, last_seen_at",
        )
        .bind(user_id)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// look up a live (unexpired) session by its token digest and bump
    /// `last_seen_at`; returns `None` for a missing, wrong, or expired token
    pub async fn find_active_by_hash(&self, token_hash: &str) -> Result<Option<Session>> {
        sqlx::query_as(
            "update sessions set last_seen_at = now()
             where token_hash = $1 and expires_at > now()
             returning id, user_id, token_hash, created_at, expires_at, last_seen_at",
        )
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// delete a session by its token digest (logout); a no-op if it's
    /// already gone or expired
    pub async fn delete_by_hash(&self, token_hash: &str) -> Result<()> {
        sqlx::query("delete from sessions where token_hash = $1")
            .bind(token_hash)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// revoke every live session for a user (used when an account is
    /// deactivated or deleted so access is cut immediately)
    pub async fn delete_for_user(&self, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from sessions where user_id = $1")
            .bind(user_id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }
}

pub struct AuditLogRepo<'a>(pub &'a PgPool);

/// The immutable boundary for a stable keyset page. Audit records are ordered
/// by `(at desc, id desc)` so records written after a page is read do not shift
/// its older results.
#[derive(Debug, Clone, Copy)]
pub struct AuditLogCursor {
    pub at: DateTime<Utc>,
    pub id: Uuid,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum AuditLogDirection {
    #[default]
    Next,
    Previous,
}

#[derive(Debug, Clone, Default)]
pub struct AuditLogFilter {
    pub actor_user_id: Option<Uuid>,
    pub action: Option<String>,
    pub target_type: Option<String>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub cursor: Option<AuditLogCursor>,
    pub direction: AuditLogDirection,
}

#[derive(Debug)]
pub struct AuditLogPage {
    pub entries: Vec<AuditLogEntry>,
    pub has_more: bool,
}

/// Global ingress policy. The encrypted secret remains write-only; callers
/// receive only whether a managed dashboard credential is configured.
pub struct SecuritySettingsRepo<'a>(pub &'a PgPool);

impl SecuritySettingsRepo<'_> {
    pub async fn get(&self) -> Result<SecuritySettings> {
        sqlx::query_as(
            "select virtual_key_required, allow_direct_provider_keys, allowed_origins, allowed_headers, \
                    required_headers, auth_bypass_routes, dashboard_auth_enabled, dashboard_credential_ref, \
                    dashboard_credential_ciphertext is not null as dashboard_secret_configured, updated_at \
             from security_settings where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        virtual_key_required: bool,
        allow_direct_provider_keys: bool,
        allowed_origins: &[String],
        allowed_headers: &[String],
        required_headers: serde_json::Value,
        auth_bypass_routes: &[String],
        dashboard_auth_enabled: bool,
        dashboard_credential_ref: Option<&str>,
        dashboard_secret: Option<(&[u8], &[u8])>,
    ) -> Result<SecuritySettings> {
        let (ciphertext, nonce) = match dashboard_secret {
            Some((ciphertext, nonce)) => (Some(ciphertext), Some(nonce)),
            None => (None, None),
        };
        sqlx::query_as(
            "update security_settings set \
                virtual_key_required = $1, allow_direct_provider_keys = $2, allowed_origins = $3, \
                allowed_headers = $4, required_headers = $5, auth_bypass_routes = $6, \
                dashboard_auth_enabled = $7, dashboard_credential_ref = $8, \
                dashboard_credential_ciphertext = coalesce($9, dashboard_credential_ciphertext), \
                dashboard_credential_nonce = coalesce($10, dashboard_credential_nonce), updated_at = now() \
             where id = true \
             returning virtual_key_required, allow_direct_provider_keys, allowed_origins, allowed_headers, \
                       required_headers, auth_bypass_routes, dashboard_auth_enabled, dashboard_credential_ref, \
                       dashboard_credential_ciphertext is not null as dashboard_secret_configured, updated_at",
        )
        .bind(virtual_key_required)
        .bind(allow_direct_provider_keys)
        .bind(allowed_origins)
        .bind(allowed_headers)
        .bind(required_headers)
        .bind(auth_bypass_routes)
        .bind(dashboard_auth_enabled)
        .bind(dashboard_credential_ref)
        .bind(ciphertext)
        .bind(nonce)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

impl AuditLogRepo<'_> {
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        action: &str,
        target_type: Option<&str>,
        target_id: Option<Uuid>,
        detail: Option<serde_json::Value>,
    ) -> Result<AuditLogEntry> {
        sqlx::query_as(
            "insert into audit_log (org_id, actor_user_id, action, target_type, target_id, detail)
             values ($1, $2, $3, $4, $5, $6)
             returning id, org_id, actor_user_id, action, target_type, target_id, detail, at",
        )
        .bind(org_id)
        .bind(actor_user_id)
        .bind(action)
        .bind(target_type)
        .bind(target_id)
        .bind(detail)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid, limit: i64) -> Result<Vec<AuditLogEntry>> {
        sqlx::query_as(
            "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
             from audit_log where org_id = $1 order by at desc limit $2",
        )
        .bind(org_id)
        .bind(limit)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Query one cursor page. `Previous` scans newer records in ascending
    /// order, then reverses them before returning so every response remains
    /// newest-first to API clients.
    pub async fn list_page(
        &self,
        org_id: Uuid,
        filter: &AuditLogFilter,
        limit: i64,
    ) -> Result<AuditLogPage> {
        let query = match filter.direction {
            AuditLogDirection::Next => {
                "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
                 from audit_log
                 where org_id = $1
                   and ($2::uuid is null or actor_user_id = $2)
                   and ($3::text is null or action = $3)
                   and ($4::text is null or target_type = $4)
                   and ($5::timestamptz is null or at >= $5)
                   and ($6::timestamptz is null or at <= $6)
                   and ($7::timestamptz is null or (at, id) < ($7, $8))
                 order by at desc, id desc limit $9"
            }
            AuditLogDirection::Previous => {
                "select id, org_id, actor_user_id, action, target_type, target_id, detail, at
                 from audit_log
                 where org_id = $1
                   and ($2::uuid is null or actor_user_id = $2)
                   and ($3::text is null or action = $3)
                   and ($4::text is null or target_type = $4)
                   and ($5::timestamptz is null or at >= $5)
                   and ($6::timestamptz is null or at <= $6)
                   and ($7::timestamptz is null or (at, id) > ($7, $8))
                 order by at asc, id asc limit $9"
            }
        };
        let mut entries: Vec<AuditLogEntry> = sqlx::query_as(query)
            .bind(org_id)
            .bind(filter.actor_user_id)
            .bind(filter.action.as_deref())
            .bind(filter.target_type.as_deref())
            .bind(filter.start_at)
            .bind(filter.end_at)
            .bind(filter.cursor.map(|cursor| cursor.at))
            .bind(filter.cursor.map(|cursor| cursor.id))
            .bind(limit + 1)
            .fetch_all(self.0)
            .await
            .map_err(store_err)?;
        let has_more = entries.len() as i64 > limit;
        if has_more {
            entries.pop();
        }
        if matches!(filter.direction, AuditLogDirection::Previous) {
            entries.reverse();
        }
        Ok(AuditLogPage { entries, has_more })
    }

    /// Count matching records without applying a cursor. Callers opt in to
    /// this extra query because a precise total is not needed for normal
    /// next/previous navigation.
    pub async fn count(&self, org_id: Uuid, filter: &AuditLogFilter) -> Result<i64> {
        sqlx::query_scalar(
            "select count(*) from audit_log
             where org_id = $1
               and ($2::uuid is null or actor_user_id = $2)
               and ($3::text is null or action = $3)
               and ($4::text is null or target_type = $4)
               and ($5::timestamptz is null or at >= $5)
               and ($6::timestamptz is null or at <= $6)",
        )
        .bind(org_id)
        .bind(filter.actor_user_id)
        .bind(filter.action.as_deref())
        .bind(filter.target_type.as_deref())
        .bind(filter.start_at)
        .bind(filter.end_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

/// Provider groups and their membership (ADR-0017 addendum, ADR-0022). A group
/// is org-scoped; its slug shares the provider slug namespace. Members are
/// stored in `provider_group_members`; `set_members` replaces the membership
/// atomically so the group and its members stay consistent.
pub struct ProviderGroupRepo<'a>(pub &'a PgPool);

impl ProviderGroupRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ProviderGroup>> {
        sqlx::query_as(
            "select id, org_id, name, slug, strategy, created_at
             from provider_groups where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ProviderGroup> {
        sqlx::query_as(
            "select id, org_id, name, slug, strategy, created_at
             from provider_groups where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("provider group {id}")))
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        strategy: &str,
    ) -> Result<ProviderGroup> {
        sqlx::query_as(
            "insert into provider_groups (org_id, name, slug, strategy)
             values ($1, $2, $3, $4)
             returning id, org_id, name, slug, strategy, created_at",
        )
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(strategy)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Update the mutable fields of a group. `None` leaves a field unchanged;
    /// `slug` is immutable by default (the control API gates any change).
    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        slug: Option<&str>,
        strategy: Option<&str>,
    ) -> Result<ProviderGroup> {
        sqlx::query_as(
            "update provider_groups set
                 name = coalesce($2, name),
                 slug = coalesce($3, slug),
                 strategy = coalesce($4, strategy)
             where id = $1
             returning id, org_id, name, slug, strategy, created_at",
        )
        .bind(id)
        .bind(name)
        .bind(slug)
        .bind(strategy)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("provider group {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from provider_groups where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("provider group {id}")));
        }
        Ok(())
    }

    /// List a group's members with the provider name joined in, ordered by
    /// `position` for a stable fan-out.
    pub async fn members(&self, group_id: Uuid) -> Result<Vec<ProviderGroupMember>> {
        sqlx::query_as(
            "select m.group_id, m.provider_id, p.name as provider_name,
                    m.upstream_model, m.weight, m.position
             from provider_group_members m
             join providers p on p.id = m.provider_id
             where m.group_id = $1
             order by m.position, p.name",
        )
        .bind(group_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Replace a group's membership atomically. Each tuple is
    /// `(provider_id, upstream_model, weight)`; `position` is the tuple index.
    pub async fn set_members(
        &self,
        group_id: Uuid,
        members: &[(Uuid, Option<String>, i32)],
    ) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from provider_group_members where group_id = $1")
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        for (position, (provider_id, upstream_model, weight)) in members.iter().enumerate() {
            sqlx::query(
                "insert into provider_group_members
                     (group_id, provider_id, upstream_model, weight, position)
                 values ($1, $2, $3, $4, $5)",
            )
            .bind(group_id)
            .bind(provider_id)
            .bind(upstream_model.as_deref())
            .bind(weight)
            .bind(position as i32)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh_pool() -> PgPool {
        let url = std::env::var("ROLTER_TEST_DATABASE_URL")
            .expect("ROLTER_TEST_DATABASE_URL not set; skipping");
        super::super::test_support::fresh_scoped_pool(&url).await
    }

    #[tokio::test]
    async fn crud_roundtrip_across_the_tenancy_and_routing_tables() {
        let Ok(_) = std::env::var("ROLTER_TEST_DATABASE_URL") else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

        let orgs = OrgRepo(&pool);
        let org = orgs.create("acme", "acme").await.unwrap();
        assert_eq!(orgs.get(org.id).await.unwrap().slug, "acme");
        assert_eq!(orgs.list().await.unwrap().len(), 1);

        let teams = TeamRepo(&pool);
        let team = teams.create(org.id, "core").await.unwrap();
        assert_eq!(teams.list(org.id).await.unwrap().len(), 1);

        let projects = ProjectRepo(&pool);
        let project = projects.create(team.id, "default").await.unwrap();
        assert_eq!(projects.list(team.id).await.unwrap().len(), 1);

        let providers = ProviderRepo(&pool);
        let provider = providers
            .create(
                org.id,
                "openai",
                "openai",
                "openai",
                "https://api.openai.com",
                Some("OPENAI_API_KEY"),
                None,
                &[],
            )
            .await
            .unwrap();
        assert_eq!(providers.list(org.id).await.unwrap().len(), 1);

        let routes = RouteRepo(&pool);
        let route = routes
            .create(project.id, "gpt-4o", "power_of_two")
            .await
            .unwrap();
        assert!(route.enabled);
        let disabled = routes.set_enabled(route.id, false).await.unwrap();
        assert!(!disabled.enabled);

        let targets = RouteTargetRepo(&pool);
        let target = targets
            .create(route.id, provider.id, Some("gpt-4o-2024-08-06"), 2)
            .await
            .unwrap();
        assert_eq!(targets.list(route.id).await.unwrap().len(), 1);
        targets.delete(target.id).await.unwrap();
        assert!(targets.list(route.id).await.unwrap().is_empty());

        let keys = VirtualKeyRepo(&pool);
        let vk = keys
            .create(
                project.id,
                "hash123",
                "sk-abc",
                Some("ci key"),
                &["gpt-4o".to_string()],
                &[],
                None,
                None,
            )
            .await
            .unwrap();
        // defaults to inherit-the-route (NULL) on create
        assert_eq!(vk.cache_enabled, None);
        assert_eq!(
            keys.find_by_hash("hash123").await.unwrap().map(|k| k.id),
            Some(vk.id)
        );
        let disabled_key = keys.set_disabled(vk.id, true).await.unwrap();
        assert!(disabled_key.disabled);
        // the cache override round-trips: force off, then clear back to inherit
        let off = keys.set_cache(vk.id, Some(false)).await.unwrap();
        assert_eq!(off.cache_enabled, Some(false));
        let cleared = keys.set_cache(vk.id, None).await.unwrap();
        assert_eq!(cleared.cache_enabled, None);

        let budgets = BudgetRepo(&pool);
        let budget = budgets
            .create("project", project.id, "100.5000", "30d")
            .await
            .unwrap();
        assert_eq!(budget.limit_usd, "100.5000");
        assert_eq!(
            budgets
                .list_for_scope("project", project.id)
                .await
                .unwrap()
                .len(),
            1
        );

        let limits = RateLimitRepo(&pool);
        let limit = limits
            .create("project", project.id, Some(60), Some(100_000))
            .await
            .unwrap();
        assert_eq!(limit.rpm, Some(60));

        let prices = ModelPriceRepo(&pool);
        let price = prices
            .upsert("gpt-4o", "2.500000", "10.000000", None, "USD")
            .await
            .unwrap();
        assert_eq!(price.input_per_mtok, "2.500000");
        let updated = prices
            .upsert("gpt-4o", "3.000000", "10.000000", None, "USD")
            .await
            .unwrap();
        assert_eq!(updated.input_per_mtok, "3.000000");
        assert_eq!(prices.list().await.unwrap().len(), 1);

        // deletes cascade top-down; exercise the not-found error path too
        orgs.delete(org.id).await.unwrap();
        assert!(matches!(orgs.get(org.id).await, Err(Error::NotFound(_))));
    }

    #[tokio::test]
    async fn audit_log_keyset_pages_filter_without_shifting_boundaries() {
        if std::env::var("ROLTER_TEST_DATABASE_URL").is_err() {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        }
        let pool = fresh_pool().await;
        let org = OrgRepo(&pool).create("audit", "audit").await.unwrap();
        let actor = UserRepo(&pool)
            .create("audit@example.com", None, false)
            .await
            .unwrap();
        let repo = AuditLogRepo(&pool);
        let now = Utc::now();
        for (offset, action) in [
            (3, "route.create"),
            (2, "route.create"),
            (1, "route.delete"),
        ] {
            let entry = repo
                .create(
                    Some(org.id),
                    Some(actor.id),
                    action,
                    Some("route"),
                    Some(Uuid::new_v4()),
                    None,
                )
                .await
                .unwrap();
            sqlx::query("update audit_log set at = $2 where id = $1")
                .bind(entry.id)
                .bind(now - chrono::Duration::seconds(offset))
                .execute(&pool)
                .await
                .unwrap();
        }
        let filter = AuditLogFilter {
            actor_user_id: Some(actor.id),
            action: Some("route.create".to_string()),
            target_type: Some("route".to_string()),
            ..Default::default()
        };
        let first = repo.list_page(org.id, &filter, 1).await.unwrap();
        assert_eq!(first.entries.len(), 1);
        assert!(first.has_more);
        let boundary = first.entries[0].clone();
        let second = repo
            .list_page(
                org.id,
                &AuditLogFilter {
                    cursor: Some(AuditLogCursor {
                        at: boundary.at,
                        id: boundary.id,
                    }),
                    ..filter.clone()
                },
                1,
            )
            .await
            .unwrap();
        assert_eq!(second.entries.len(), 1);
        assert_ne!(second.entries[0].id, boundary.id);
        let previous = repo
            .list_page(
                org.id,
                &AuditLogFilter {
                    cursor: Some(AuditLogCursor {
                        at: second.entries[0].at,
                        id: second.entries[0].id,
                    }),
                    direction: AuditLogDirection::Previous,
                    ..filter.clone()
                },
                1,
            )
            .await
            .unwrap();
        assert_eq!(previous.entries[0].id, boundary.id);
        assert_eq!(repo.count(org.id, &filter).await.unwrap(), 2);
    }
}
