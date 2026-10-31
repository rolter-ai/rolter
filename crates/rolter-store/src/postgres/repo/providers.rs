//! Upstream providers, their keys and provider groups.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{Provider, ProviderCacheProbe, ProviderGroup, ProviderGroupMember};
use super::support::{
    fetch_optional_or_not_found, fetch_scalar_or_not_found, require_affected, store_err,
};

/// Every column of `provider_groups` that its row type reads.
const PROVIDER_GROUP_COLUMNS: &str = "id, org_id, name, slug, strategy, project_id, created_at";

/// Every column of `providers` that its row type reads.
const PROVIDER_COLUMNS: &str = "id, org_id, name, slug, kind, api_base, api_key_env, \
    egress_proxy, egress_proxies, kv_events, lmcache, project_id, allow_custom_api_base, \
    created_at";

/// What [`ProviderRepo::delete`] did with a provider that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum ProviderDeletion {
    /// The provider is gone.
    Deleted,
    /// The provider was left in place because something still uses it. Each
    /// entry names one referrer for the refusal, as `route '<model>'` or
    /// `provider group '<slug>'`, routes first.
    InUse(Vec<String>),
}

/// Upstream providers, scoped to an org.
pub struct ProviderRepo<'a>(pub &'a PgPool);

/// A provider's optional telemetry setting as the json the column holds.
fn setting_json<T: serde::Serialize>(setting: Option<&T>) -> Result<Option<serde_json::Value>> {
    setting
        .map(serde_json::to_value)
        .transpose()
        .map_err(|err| Error::Store(format!("provider setting is not representable: {err}")))
}

/// The settings a provider is created with. A struct rather than twelve
/// positional arguments, so a caller cannot transpose `kind` and `api_base` or
/// the optional env and proxy names.
#[derive(Debug, Clone, Copy)]
pub struct NewProvider<'a> {
    pub name: &'a str,
    pub slug: &'a str,
    pub kind: &'a str,
    pub api_base: &'a str,
    pub api_key_env: Option<&'a str>,
    pub egress_proxy: Option<&'a str>,
    pub egress_proxies: &'a [String],
    pub kv_events: Option<&'a rolter_core::KvEventsConfig>,
    pub lmcache: Option<&'a rolter_core::LmCacheConfig>,
    pub project_id: Option<Uuid>,
    pub allow_custom_api_base: bool,
}

/// The fields of a provider an update may change. `None` leaves a field
/// unchanged and the nullable fields take `Some(None)` to clear, as on
/// [`ProviderRepo::update`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderUpdate<'a> {
    pub slug: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub api_base: Option<&'a str>,
    pub api_key_env: Option<Option<&'a str>>,
    pub egress_proxy: Option<Option<&'a str>>,
    pub egress_proxies: Option<&'a [String]>,
    pub kv_events: Option<Option<&'a rolter_core::KvEventsConfig>>,
    pub lmcache: Option<Option<&'a rolter_core::LmCacheConfig>>,
    pub project_id: Option<Option<Uuid>>,
    pub allow_custom_api_base: Option<bool>,
}

impl ProviderRepo<'_> {
    /// Whether a provider of `org_id` already has `name`. Names are per org
    /// (#1857, ADR-0040): another org may hold the same one. `None` asks
    /// across every org.
    pub async fn name_in_use(&self, name: &str, org_id: Option<Uuid>) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from providers where name = $1 \
             and ($2::uuid is null or org_id = $2))",
        )
        .bind(name)
        .bind(org_id)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether a provider other than `except` already has `slug`, in any org.
    /// `provider-slug/model` addressing resolves a slug across the deployment.
    pub async fn slug_in_use(&self, slug: &str, except: Option<Uuid>) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from providers where slug = $1 \
             and ($2::uuid is null or id <> $2))",
        )
        .bind(slug)
        .bind(except)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether a provider or a provider group of `org_id` other than `except`
    /// already answers `slug`.
    ///
    /// The gateway resolves `provider-slug/model` and `group-slug/model` in one
    /// namespace per org (#1857, ADR-0040), and a group whose slug a provider
    /// of its org holds is not addressable at all. So a new or renamed slug has
    /// to be free in both tables of the org, or the second holder silently
    /// takes the address from the first; another org may hold the same slug.
    /// Row ids are unique across the two tables, so one `except` covers a
    /// rename of either kind. `None` asks across every org.
    pub async fn address_slug_in_use(
        &self,
        slug: &str,
        org_id: Option<Uuid>,
        except: Option<Uuid>,
    ) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from providers where slug = $1 \
                            and ($2::uuid is null or org_id = $2) \
                            and ($3::uuid is null or id <> $3)) \
                 or exists(select 1 from provider_groups where slug = $1 \
                            and ($2::uuid is null or org_id = $2) \
                            and ($3::uuid is null or id <> $3))",
        )
        .bind(slug)
        .bind(org_id)
        .bind(except)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Whether a provider or provider group outside `org_id` answers `slug`.
    ///
    /// A route named `slug/…` in `org_id` would sit on that holder's
    /// `provider-slug/model` or `group-slug/model` address, and a key with no
    /// org (the operator's) would reach the route instead of the address.
    /// `None` treats every org as another one.
    pub async fn address_slug_outside_org(&self, slug: &str, org_id: Option<Uuid>) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from providers where slug = $1 \
                            and ($2::uuid is null or org_id <> $2)) \
                 or exists(select 1 from provider_groups where slug = $1 \
                            and ($2::uuid is null or org_id <> $2))",
        )
        .bind(slug)
        .bind(org_id)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Names of the providers in `provider_ids` that are scoped to a project
    /// other than `owner`: the ones a route of project `owner`, or a group
    /// scoped to it, may not use (#1919). `owner` is `None` for an org-wide
    /// group, which may hold no scoped provider at all. An org-wide provider
    /// is never listed, so every project can use it.
    pub async fn scoped_outside(
        &self,
        provider_ids: &[Uuid],
        owner: Option<Uuid>,
    ) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "select name from providers
             where id = any($1) and project_id is not null
               and project_id is distinct from $2
             order by name",
        )
        .bind(provider_ids)
        .bind(owner)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// What would stop using `provider_id` if it were scoped to `scope`: the
    /// routes of other projects that target it and the groups not scoped to
    /// `scope` that hold it, each named for the error that refuses the change.
    /// Empty for `None`, since org-wide is reachable from everywhere.
    pub async fn dependents_outside(
        &self,
        provider_id: Uuid,
        scope: Option<Uuid>,
    ) -> Result<Vec<String>> {
        let Some(scope) = scope else {
            return Ok(Vec::new());
        };
        let routes: Vec<String> = sqlx::query_scalar(
            "select model from (
                 select r.model from route_targets rt
                 join routes r on r.id = rt.route_id
                 where rt.provider_id = $1 and r.project_id <> $2
                 union
                 select r.model from route_variant_targets vt
                 join route_variants v on v.id = vt.variant_id
                 join routes r on r.id = v.route_id
                 where vt.provider_id = $1 and r.project_id <> $2
             ) dependents order by model",
        )
        .bind(provider_id)
        .bind(scope)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        let groups: Vec<String> = sqlx::query_scalar(
            "select g.slug from provider_group_members m
             join provider_groups g on g.id = m.group_id
             where m.provider_id = $1 and g.project_id is distinct from $2
             order by g.slug",
        )
        .bind(provider_id)
        .bind(scope)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(routes
            .into_iter()
            .map(|model| format!("route '{model}'"))
            .chain(
                groups
                    .into_iter()
                    .map(|slug| format!("provider group '{slug}'")),
            )
            .collect())
    }

    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Provider>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PROVIDER_COLUMNS}
             from providers where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Provider> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {PROVIDER_COLUMNS}
             from providers where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("provider {id}"),
        )
        .await
    }

    pub async fn create(&self, org_id: Uuid, values: NewProvider<'_>) -> Result<Provider> {
        let NewProvider {
            name,
            slug,
            kind,
            api_base,
            api_key_env,
            egress_proxy,
            egress_proxies,
            kv_events,
            lmcache,
            project_id,
            allow_custom_api_base,
        } = values;
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into providers (org_id, name, slug, kind, \
                api_base, api_key_env, egress_proxy, egress_proxies, project_id, kv_events, \
                lmcache, allow_custom_api_base)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             returning {PROVIDER_COLUMNS}"
        )))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(kind)
        .bind(api_base)
        .bind(api_key_env)
        .bind(egress_proxy)
        .bind(serde_json::json!(egress_proxies))
        .bind(project_id)
        .bind(setting_json(kv_events)?)
        .bind(setting_json(lmcache)?)
        .bind(allow_custom_api_base)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Partially update a provider. `None` leaves a field unchanged; the
    /// nullable fields take `Some(None)` to clear. `slug` is immutable by
    /// default — callers must only pass `Some` after an explicit override
    /// (the control API gates this); the charset constraint is enforced by the
    /// database.
    pub async fn update(&self, id: Uuid, values: ProviderUpdate<'_>) -> Result<Provider> {
        let ProviderUpdate {
            slug,
            kind,
            api_base,
            api_key_env,
            egress_proxy,
            egress_proxies,
            kv_events,
            lmcache,
            project_id,
            allow_custom_api_base,
        } = values;
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update providers set
                 slug = coalesce($2, slug),
                 kind = coalesce($3, kind),
                 api_base = coalesce($4, api_base),
                 api_key_env = case when $5 then $6 else api_key_env end,
                 egress_proxy = case when $7 then $8 else egress_proxy end,
                 egress_proxies = case when $9 then $10 else egress_proxies end,
                 project_id = case when $11 then $12 else project_id end,
                 kv_events = case when $13 then $14 else kv_events end,
                 lmcache = case when $15 then $16 else lmcache end,
                 allow_custom_api_base = coalesce($17, allow_custom_api_base)
             where id = $1
             returning {PROVIDER_COLUMNS}"
            )))
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
            .bind(project_id.is_some())
            .bind(project_id.flatten())
            .bind(kv_events.is_some())
            .bind(setting_json(kv_events.flatten())?)
            .bind(lmcache.is_some())
            .bind(setting_json(lmcache.flatten())?)
            .bind(allow_custom_api_base),
            self.0,
            || format!("provider {id}"),
        )
        .await
    }

    /// Delete provider `id` unless a route target, a route variant's target
    /// (#391) or a provider-group member still references it (#2438).
    ///
    /// Both references are `on delete restrict`, so the database would refuse
    /// anyway, but only with a foreign-key error that names nothing the caller
    /// can act on. The check and the delete share one transaction that holds
    /// the provider row `for update`: inserting a target or a member takes a
    /// `for key share` lock on the provider it references, which conflicts
    /// with that, so no reference can land between the check and the delete.
    pub async fn delete(&self, id: Uuid) -> Result<ProviderDeletion> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let _: Uuid = fetch_scalar_or_not_found(
            sqlx::query_scalar("select id from providers where id = $1 for update").bind(id),
            &mut *tx,
            || format!("provider {id}"),
        )
        .await?;
        let routes: Vec<String> = sqlx::query_scalar(
            "select model from (
                 select r.model from route_targets rt
                 join routes r on r.id = rt.route_id
                 where rt.provider_id = $1
                 union
                 select r.model from route_variant_targets vt
                 join route_variants v on v.id = vt.variant_id
                 join routes r on r.id = v.route_id
                 where vt.provider_id = $1
             ) dependents order by model",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(store_err)?;
        let groups: Vec<String> = sqlx::query_scalar(
            "select g.slug from provider_group_members m
             join provider_groups g on g.id = m.group_id
             where m.provider_id = $1
             order by g.slug",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await
        .map_err(store_err)?;
        if !routes.is_empty() || !groups.is_empty() {
            // nothing was written, so dropping the transaction just releases the lock
            return Ok(ProviderDeletion::InUse(
                routes
                    .into_iter()
                    .map(|model| format!("route '{model}'"))
                    .chain(
                        groups
                            .into_iter()
                            .map(|slug| format!("provider group '{slug}'")),
                    )
                    .collect(),
            ));
        }
        sqlx::query("delete from providers where id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(ProviderDeletion::Deleted)
    }
}

/// Runtime provider credentials, sealed with AES-256-GCM (see
/// [`super::super::crypto`]). One active key per provider; setting a new one
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

    /// Which of `provider_ids` have a stored credential, in one query so a
    /// listing never asks per row. Reads only the key column, never the
    /// ciphertext or nonce.
    pub async fn stored_among(
        &self,
        provider_ids: &[Uuid],
    ) -> Result<std::collections::HashSet<Uuid>> {
        let ids: Vec<Uuid> =
            sqlx::query_scalar("select provider_id from provider_keys where provider_id = any($1)")
                .bind(provider_ids)
                .fetch_all(self.0)
                .await
                .map_err(store_err)?;
        Ok(ids.into_iter().collect())
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

/// The prompt-cache reporting verdicts of providers (#984), written by the
/// control plane's probe and read by the dashboard. The gateway does not read
/// them.
pub struct ProviderCacheProbeRepo<'a>(pub &'a PgPool);

impl ProviderCacheProbeRepo<'_> {
    /// Record what a probe of `provider_id` observed, replacing the previous
    /// verdict and stamping the time.
    pub async fn record(
        &self,
        provider_id: Uuid,
        reports: bool,
        spelling: Option<&str>,
        model: &str,
    ) -> Result<ProviderCacheProbe> {
        sqlx::query_as(
            "insert into provider_cache_probes (provider_id, reports, spelling, model, probed_at)
             values ($1, $2, $3, $4, now())
             on conflict (provider_id) do update
             set reports = excluded.reports, spelling = excluded.spelling,
                 model = excluded.model, probed_at = excluded.probed_at
             returning provider_id, reports, spelling, model, probed_at",
        )
        .bind(provider_id)
        .bind(reports)
        .bind(spelling)
        .bind(model)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// The verdicts of the providers of `org_id`; a provider never probed has
    /// no entry.
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ProviderCacheProbe>> {
        sqlx::query_as(
            "select c.provider_id, c.reports, c.spelling, c.model, c.probed_at
             from provider_cache_probes c join providers p on p.id = c.provider_id
             where p.org_id = $1 order by p.name",
        )
        .bind(org_id)
        .fetch_all(self.0)
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
    /// Whether a group other than `except` already has `slug`, in any org.
    /// `group-slug/model` addressing resolves a slug across the deployment.
    pub async fn slug_in_use(&self, slug: &str, except: Option<Uuid>) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from provider_groups where slug = $1 \
             and ($2::uuid is null or id <> $2))",
        )
        .bind(slug)
        .bind(except)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ProviderGroup>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PROVIDER_GROUP_COLUMNS}
             from provider_groups where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ProviderGroup> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {PROVIDER_GROUP_COLUMNS}
             from provider_groups where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("provider group {id}"),
        )
        .await
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        strategy: &str,
        project_id: Option<Uuid>,
    ) -> Result<ProviderGroup> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into provider_groups (org_id, name, slug, strategy, project_id)
             values ($1, $2, $3, $4, $5)
             returning {PROVIDER_GROUP_COLUMNS}"
        )))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(strategy)
        .bind(project_id)
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
        project_id: Option<Option<Uuid>>,
    ) -> Result<ProviderGroup> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update provider_groups set
                 name = coalesce($2, name),
                 slug = coalesce($3, slug),
                 strategy = coalesce($4, strategy),
                 project_id = case when $5 then $6 else project_id end
             where id = $1
             returning {PROVIDER_GROUP_COLUMNS}"
            )))
            .bind(id)
            .bind(name)
            .bind(slug)
            .bind(strategy)
            .bind(project_id.is_some())
            .bind(project_id.flatten()),
            self.0,
            || format!("provider group {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from provider_groups where id = $1").bind(id),
            self.0,
            || format!("provider group {id}"),
        )
        .await?;
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

// ---------------------------------------------------------------------------
// configurable rbac (#534)
// ---------------------------------------------------------------------------
