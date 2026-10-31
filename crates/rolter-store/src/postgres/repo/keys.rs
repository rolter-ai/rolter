//! Virtual keys.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{OwnedVirtualKey, VirtualKey};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// Every column of `virtual_keys` that its row type reads.
const VIRTUAL_KEY_COLUMNS: &str = "id, project_id, key_hash, key_prefix, name, models, \
    providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, \
    created_at, purpose, previous_key_hash, previous_key_prefix, previous_expires_at";

/// Virtual keys, scoped to a project. Callers are responsible for hashing
/// the presented key before it reaches `key_hash` — this repository never
/// sees or stores plaintext keys.
pub struct VirtualKeyRepo<'a>(pub &'a PgPool);

/// What a virtual key is minted with. A struct rather than ten positional
/// arguments, so a caller cannot transpose `key_hash` and `key_prefix` or the
/// two string lists.
#[derive(Debug, Clone, Copy)]
pub struct NewVirtualKey<'a> {
    pub key_hash: &'a str,
    pub key_prefix: &'a str,
    pub name: Option<&'a str>,
    pub models: &'a [String],
    pub providers: &'a [String],
    pub cache_enabled: Option<bool>,
    pub created_by: Option<Uuid>,
    /// When the key stops working; `None` mints an immortal key, which the
    /// callers only pass for a deliberate "never expires" choice (#945).
    pub expires_at: Option<DateTime<Utc>>,
    /// Why the key exists when a person did not mint it by hand; `None` for
    /// every operator-created key (#1640).
    pub purpose: Option<&'a str>,
}

/// A spend cap minted together with a key (#1171). It is an ordinary `budgets`
/// row scoped to the key, so the gateway enforces it like any other and the
/// budgets API edits it afterwards.
#[derive(Debug, Clone, Copy)]
pub struct NewKeyBudget<'a> {
    pub limit_usd: &'a str,
    pub period: &'a str,
}

/// A throughput cap minted together with a key (#1171); an ordinary
/// `rate_limits` row scoped to the key. At least one of the two is positive.
#[derive(Debug, Clone, Copy)]
pub struct NewKeyRateLimit {
    pub rpm: Option<i32>,
    pub tpm: Option<i32>,
}

/// The caps a key is born with. Written in the same transaction as the key, so
/// a key chosen to be bounded is never visible, or shown to its minter,
/// without its bound.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyCaps<'a> {
    pub budget: Option<NewKeyBudget<'a>>,
    pub rate_limit: Option<NewKeyRateLimit>,
    /// a rotation replaces a secret, not a decision: every cap on this key is
    /// copied onto the new one
    pub inherit_from: Option<Uuid>,
}

impl VirtualKeyRepo<'_> {
    pub async fn list(&self, project_id: Uuid) -> Result<Vec<VirtualKey>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {VIRTUAL_KEY_COLUMNS}
             from virtual_keys where project_id = $1 order by created_at"
        )))
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every key of the given projects, oldest first: what a screen that names
    /// keys across projects reads in one query instead of one per project
    /// (#3467). The caller passes the projects it may read, so the cost follows
    /// what it can see and not the size of the organization.
    pub async fn list_in_projects(&self, project_ids: &[Uuid]) -> Result<Vec<VirtualKey>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {VIRTUAL_KEY_COLUMNS}
             from virtual_keys where project_id = any($1)
             order by created_at, id"
        )))
        .bind(project_ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn find_by_hash(&self, key_hash: &str) -> Result<Option<VirtualKey>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {VIRTUAL_KEY_COLUMNS}
             from virtual_keys where key_hash = $1"
        )))
        .bind(key_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<VirtualKey> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {VIRTUAL_KEY_COLUMNS}
             from virtual_keys where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
    }

    pub async fn create(&self, project_id: Uuid, key: NewVirtualKey<'_>) -> Result<VirtualKey> {
        self.create_with_caps(project_id, key, KeyCaps::default())
            .await
    }

    /// [`create`](Self::create), plus the caps the key is minted with, in one
    /// transaction. The statement triggers on `budgets` and `rate_limits` bump
    /// `config_version`, so a polling gateway sees the key and its caps in the
    /// same snapshot or not at all.
    pub async fn create_with_caps(
        &self,
        project_id: Uuid,
        key: NewVirtualKey<'_>,
        caps: KeyCaps<'_>,
    ) -> Result<VirtualKey> {
        let NewVirtualKey {
            key_hash,
            key_prefix,
            name,
            models,
            providers,
            cache_enabled,
            created_by,
            expires_at,
            purpose,
        } = key;
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let row: VirtualKey = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into virtual_keys (project_id, key_hash, \
                key_prefix, name, models, providers, cache_enabled, created_by, expires_at, \
                purpose)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             returning {VIRTUAL_KEY_COLUMNS}"
        )))
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
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        if let Some(budget) = caps.budget {
            sqlx::query(
                "insert into budgets (scope_type, scope_id, limit_usd, period)
                 values ('virtual_key', $1, $2::numeric, $3)",
            )
            .bind(row.id)
            .bind(budget.limit_usd)
            .bind(budget.period)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        if let Some(limit) = caps.rate_limit {
            sqlx::query(
                "insert into rate_limits (scope_type, scope_id, rpm, tpm)
                 values ('virtual_key', $1, $2, $3)",
            )
            .bind(row.id)
            .bind(limit.rpm)
            .bind(limit.tpm)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        if let Some(old) = caps.inherit_from {
            sqlx::query(
                "insert into budgets (scope_type, scope_id, limit_usd, period, unpriced_policy)
                 select scope_type, $2, limit_usd, period, unpriced_policy
                 from budgets where scope_type = 'virtual_key' and scope_id = $1",
            )
            .bind(old)
            .bind(row.id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
            sqlx::query(
                "insert into rate_limits (scope_type, scope_id, rpm, tpm)
                 select scope_type, $2, rpm, tpm
                 from rate_limits where scope_type = 'virtual_key' and scope_id = $1",
            )
            .bind(old)
            .bind(row.id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(row)
    }

    /// What each of `ids` is called: its name, or its prefix when it has none.
    /// A key that no longer exists is absent from the map (#3128).
    pub async fn names(&self, ids: &[Uuid]) -> Result<std::collections::HashMap<Uuid, String>> {
        if ids.is_empty() {
            return Ok(Default::default());
        }
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "select id, coalesce(name, key_prefix) from virtual_keys where id = any($1)",
        )
        .bind(ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(rows.into_iter().collect())
    }

    /// The names given to each of `ids`, leaving out a key that has none and
    /// one that no longer exists. Unlike [`Self::names`] it never falls back to
    /// the key's prefix, for a reader that may name a key without being
    /// allowed to see any part of its secret (#3499).
    pub async fn given_names(
        &self,
        ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, String>> {
        if ids.is_empty() {
            return Ok(Default::default());
        }
        let rows: Vec<(Uuid, String)> = sqlx::query_as(
            "select id, name from virtual_keys where id = any($1) and name is not null",
        )
        .bind(ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(rows.into_iter().collect())
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

    /// Disable or re-enable a key. Disabling also ends a rotation's overlap:
    /// re-enabling the key later must not bring back a secret that was only
    /// ever meant to work for a window (#1837).
    pub async fn set_disabled(&self, id: Uuid, disabled: bool) -> Result<VirtualKey> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update virtual_keys set disabled = $2,
                    previous_key_hash = case when $2 then null else previous_key_hash end,
                    previous_key_prefix = case when $2 then null else previous_key_prefix end,
                    previous_expires_at = case when $2 then null else previous_expires_at end
             where id = $1
             returning {VIRTUAL_KEY_COLUMNS}"
            )))
            .bind(id)
            .bind(disabled),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
    }

    /// Replace the key's secret on the same row (#1837), so every setting,
    /// attribution and history row carries over.
    ///
    /// With `overlap_until` the secret being replaced keeps authenticating
    /// until then; without it the old secret stops at once. Either way a
    /// previous overlap still running is discarded, because a row holds one
    /// previous secret at most. Only digests are stored.
    pub async fn rotate(
        &self,
        id: Uuid,
        key_hash: &str,
        key_prefix: &str,
        overlap_until: Option<DateTime<Utc>>,
    ) -> Result<VirtualKey> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update virtual_keys set
                    previous_key_hash = case when $4::timestamptz is null then null else key_hash end,
                    previous_key_prefix = case when $4::timestamptz is null then null else key_prefix end,
                    previous_expires_at = $4,
                    key_hash = $2,
                    key_prefix = $3
                 where id = $1
                 returning {VIRTUAL_KEY_COLUMNS}"
            )))
            .bind(id)
            .bind(key_hash)
            .bind(key_prefix)
            .bind(overlap_until),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
    }

    /// End an overlap now: the previous secret stops authenticating at the next
    /// snapshot. A no-op, still returning the row, when none is running.
    pub async fn revoke_previous(&self, id: Uuid) -> Result<VirtualKey> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update virtual_keys set previous_key_hash = null,
                    previous_key_prefix = null, previous_expires_at = null
                 where id = $1
                 returning {VIRTUAL_KEY_COLUMNS}"
            )))
            .bind(id),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
    }

    /// Set (or clear) the per-key response-cache override. `None` restores the
    /// inherit-the-route default; `Some(bool)` forces caching off/on for the key.
    pub async fn set_cache(&self, id: Uuid, cache_enabled: Option<bool>) -> Result<VirtualKey> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update virtual_keys set cache_enabled = $2 where id = $1
             returning {VIRTUAL_KEY_COLUMNS}"
            )))
            .bind(id)
            .bind(cache_enabled),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
    }

    /// Replace the key's provider allow-list. An empty list restores the
    /// permissive default while leaving the model allow-list unchanged.
    pub async fn set_providers(&self, id: Uuid, providers: &[String]) -> Result<VirtualKey> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update virtual_keys set providers = $2 where id = $1
             returning {VIRTUAL_KEY_COLUMNS}"
            )))
            .bind(id)
            .bind(providers),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
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
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update virtual_keys set business_unit_id = $2, customer_id = $3 where id = $1
             returning {VIRTUAL_KEY_COLUMNS}"
            )))
            .bind(id)
            .bind(business_unit_id)
            .bind(customer_id),
            self.0,
            || format!("virtual key {id}"),
        )
        .await
    }

    /// Delete a key and the caps scoped to it. `budgets` and `rate_limits`
    /// carry no foreign key to a scope, so without this a deleted key's caps
    /// would stay behind, listed in the org-wide reads and shipped in every
    /// snapshot for a key that no longer exists.
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        require_affected(
            sqlx::query("delete from virtual_keys where id = $1").bind(id),
            &mut *tx,
            || format!("virtual key {id}"),
        )
        .await?;
        for table in ["budgets", "rate_limits"] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "delete from {table} where scope_type = 'virtual_key' and scope_id = $1"
            )))
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
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
