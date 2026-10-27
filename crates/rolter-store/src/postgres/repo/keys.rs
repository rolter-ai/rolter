//! Virtual keys.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{OwnedVirtualKey, VirtualKey};
use super::support::store_err;

/// Virtual keys, scoped to a project. Callers are responsible for hashing
/// the presented key before it reaches `key_hash` — this repository never
/// sees or stores plaintext keys.
pub struct VirtualKeyRepo<'a>(pub &'a PgPool);

impl VirtualKeyRepo<'_> {
    pub async fn list(&self, project_id: Uuid) -> Result<Vec<VirtualKey>> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose
             from virtual_keys where project_id = $1 order by created_at",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn find_by_hash(&self, key_hash: &str) -> Result<Option<VirtualKey>> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose
             from virtual_keys where key_hash = $1",
        )
        .bind(key_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<VirtualKey> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose
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
        // when the key stops working; `None` mints an immortal key, which the
        // callers only pass for a deliberate "never expires" choice (#945)
        expires_at: Option<DateTime<Utc>>,
        // why the key exists when a person did not mint it by hand; `None` for
        // every operator-created key (#1640)
        purpose: Option<&str>,
    ) -> Result<VirtualKey> {
        sqlx::query_as(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name, models, providers, cache_enabled, created_by, expires_at, purpose)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose",
        )
        .bind(project_id)
        .bind(key_hash)
        .bind(key_prefix)
        .bind(name)
        .bind(models)
        .bind(providers)
        .bind(cache_enabled)
        .bind(created_by)
        .bind(expires_at)
        .bind(purpose)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// every key minted by `user_id` via the self-service panel, newest first,
    /// enriched with the owning project + org names. omits the key hash.
    pub async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<OwnedVirtualKey>> {
        sqlx::query_as(
            "select vk.id, vk.project_id, p.name as project_name, o.name as org_name,
                    vk.key_prefix, vk.name, vk.models, vk.disabled, vk.expires_at, vk.created_at, vk.purpose
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

    /// How many keys `user_id` minted for themselves. Deactivating the account
    /// takes exactly these out of the snapshot, and deleting it disables them
    /// (#1841), so the count is what an offboarding audit row records.
    pub async fn count_personal(&self, user_id: Uuid) -> Result<i64> {
        sqlx::query_scalar("select count(*) from virtual_keys where created_by = $1")
            .bind(user_id)
            .fetch_one(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn set_disabled(&self, id: Uuid, disabled: bool) -> Result<VirtualKey> {
        sqlx::query_as(
            "update virtual_keys set disabled = $2 where id = $1
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose",
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
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose",
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
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose",
        )
        .bind(id)
        .bind(providers)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("virtual key {id}")))
    }

    /// Point the key's spend at a business unit and/or customer. `None` clears
    /// the dimension, leaving the key attributed to its tenancy chain only.
    /// Callers must verify both ids belong to the key's org.
    pub async fn set_attribution(
        &self,
        id: Uuid,
        business_unit_id: Option<Uuid>,
        customer_id: Option<Uuid>,
    ) -> Result<VirtualKey> {
        sqlx::query_as(
            "update virtual_keys set business_unit_id = $2, customer_id = $3 where id = $1
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at, purpose",
        )
        .bind(id)
        .bind(business_unit_id)
        .bind(customer_id)
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

/// What an in-place edit did to a row (#1285).
///
/// `before` is the row as it stood under the edit's own row lock, so a diff
/// built from the pair describes this edit and not one that overtook it
/// between a caller's earlier read and the write.
#[derive(Debug, Clone)]
pub struct Edited<T> {
    pub before: T,
    pub after: T,
    /// Unset when every field the edit named already held the value sent. No
    /// statement was issued then, so `config_version` did not move and there
    /// is nothing to announce or audit.
    pub changed: bool,
}

impl<T: Clone> Edited<T> {
    pub(super) fn unchanged(row: T) -> Self {
        Self {
            after: row.clone(),
            before: row,
            changed: false,
        }
    }
}

/// Whether a rate limit with these caps limits anything. The snapshot loader
/// reads a cap of zero or below as no cap, so only a positive one counts.
pub(super) fn keeps_a_cap(rpm: Option<i32>, tpm: Option<i32>) -> bool {
    rpm.is_some_and(|cap| cap > 0) || tpm.is_some_and(|cap| cap > 0)
}
