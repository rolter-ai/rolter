//! SCIM tokens, identities, groups and group mappings.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{ScimGroup, ScimGroupMapping, ScimIdentity, ScimToken};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// SCIM provisioning tokens. Lookup is by peppered digest only; the plaintext
/// token never reaches the database.
pub struct ScimTokenRepo<'a>(pub &'a PgPool);

const SCIM_TOKEN_COLUMNS: &str =
    "id, org_id, name, token_hash, created_by, created_at, last_used_at, revoked_at";

impl ScimTokenRepo<'_> {
    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        token_hash: &str,
        created_by: Option<Uuid>,
    ) -> Result<ScimToken> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into scim_tokens (org_id, name, token_hash, created_by) \
             values ($1, $2, $3, $4) returning {SCIM_TOKEN_COLUMNS}"
        )))
        .bind(org_id)
        .bind(name)
        .bind(token_hash)
        .bind(created_by)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimToken>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_TOKEN_COLUMNS} from scim_tokens where org_id = $1 \
             order by created_at desc"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ScimToken> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {SCIM_TOKEN_COLUMNS} from scim_tokens where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("scim token {id}"),
        )
        .await
    }

    /// Resolve a presented token by digest. Revoked tokens never resolve, so a
    /// revocation takes effect on the next request with no cache to invalidate.
    pub async fn find_active_by_hash(&self, token_hash: &str) -> Result<Option<ScimToken>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_TOKEN_COLUMNS} from scim_tokens \
             where token_hash = $1 and revoked_at is null"
        )))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<ScimToken> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update scim_tokens set revoked_at = coalesce(revoked_at, now()) \
             where id = $1 returning {SCIM_TOKEN_COLUMNS}"
            )))
            .bind(id),
            self.0,
            || format!("scim token {id}"),
        )
        .await
    }

    /// Best-effort last-used stamp for the provisioning screen.
    pub async fn touch(&self, id: Uuid) -> Result<()> {
        sqlx::query("update scim_tokens set last_used_at = now() where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }
}

/// The org-scoped SCIM identity of a local user.
pub struct ScimIdentityRepo<'a>(pub &'a PgPool);

const SCIM_IDENTITY_COLUMNS: &str =
    "user_id, org_id, external_id, user_name, display_name, created_at, updated_at";

impl ScimIdentityRepo<'_> {
    /// Create or update the identity for `user_id` in `org_id`. Idempotent by
    /// design: an IdP that replays a create must not produce a second row.
    pub async fn upsert(
        &self,
        user_id: Uuid,
        org_id: Uuid,
        external_id: Option<&str>,
        user_name: &str,
        display_name: &str,
    ) -> Result<ScimIdentity> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into scim_identities (user_id, org_id, external_id, user_name, display_name) \
             values ($1, $2, $3, $4, $5) \
             on conflict (user_id, org_id) do update set \
                 external_id = excluded.external_id, user_name = excluded.user_name, \
                 display_name = excluded.display_name, updated_at = now() \
             returning {SCIM_IDENTITY_COLUMNS}"
        )))
        .bind(user_id)
        .bind(org_id)
        .bind(external_id)
        .bind(user_name)
        .bind(display_name)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, user_id: Uuid, org_id: Uuid) -> Result<Option<ScimIdentity>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_IDENTITY_COLUMNS} from scim_identities \
             where user_id = $1 and org_id = $2"
        )))
        .bind(user_id)
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// whether any org's IdP provisioned this account
    pub async fn exists_for_user(&self, user_id: Uuid) -> Result<bool> {
        sqlx::query_scalar("select exists (select 1 from scim_identities where user_id = $1)")
            .bind(user_id)
            .fetch_one(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn find_by_user_name(
        &self,
        org_id: Uuid,
        user_name: &str,
    ) -> Result<Option<ScimIdentity>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_IDENTITY_COLUMNS} from scim_identities \
             where org_id = $1 and user_name = $2"
        )))
        .bind(org_id)
        .bind(user_name)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Every provisioned identity in the org, oldest first so paging is stable.
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimIdentity>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_IDENTITY_COLUMNS} from scim_identities where org_id = $1 \
             order by created_at, user_id"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Drop the mapping without touching the underlying user, whose row,
    /// memberships and audit trail outlive any one IdP.
    pub async fn delete(&self, user_id: Uuid, org_id: Uuid) -> Result<()> {
        sqlx::query("delete from scim_identities where user_id = $1 and org_id = $2")
            .bind(user_id)
            .bind(org_id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }
}

/// SCIM groups and their membership, org-scoped like every other SCIM row.
pub struct ScimGroupRepo<'a>(pub &'a PgPool);

const SCIM_GROUP_COLUMNS: &str = "id, org_id, external_id, display_name, created_at, updated_at";

impl ScimGroupRepo<'_> {
    pub async fn create(
        &self,
        org_id: Uuid,
        external_id: Option<&str>,
        display_name: &str,
    ) -> Result<ScimGroup> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into scim_groups (org_id, external_id, display_name) \
             values ($1, $2, $3) returning {SCIM_GROUP_COLUMNS}"
        )))
        .bind(org_id)
        .bind(external_id)
        .bind(display_name)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Look a group up inside its org. The org is part of the predicate rather
    /// than checked afterwards, so a foreign id simply does not resolve.
    pub async fn get(&self, id: Uuid, org_id: Uuid) -> Result<Option<ScimGroup>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_GROUP_COLUMNS} from scim_groups where id = $1 and org_id = $2"
        )))
        .bind(id)
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn find_by_display_name(
        &self,
        org_id: Uuid,
        display_name: &str,
    ) -> Result<Option<ScimGroup>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_GROUP_COLUMNS} from scim_groups \
             where org_id = $1 and display_name = $2"
        )))
        .bind(org_id)
        .bind(display_name)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Every group in the org, oldest first so paging is stable.
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimGroup>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_GROUP_COLUMNS} from scim_groups where org_id = $1 \
             order by created_at, id"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        org_id: Uuid,
        external_id: Option<&str>,
        display_name: &str,
    ) -> Result<ScimGroup> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update scim_groups set external_id = $3, display_name = $4, updated_at = now() \
             where id = $1 and org_id = $2 returning {SCIM_GROUP_COLUMNS}"
            )))
            .bind(id)
            .bind(org_id)
            .bind(external_id)
            .bind(display_name),
            self.0,
            || format!("scim group {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid, org_id: Uuid) -> Result<()> {
        sqlx::query("delete from scim_groups where id = $1 and org_id = $2")
            .bind(id)
            .bind(org_id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// The group's members, oldest first.
    pub async fn members(&self, group_id: Uuid) -> Result<Vec<Uuid>> {
        sqlx::query_scalar(
            "select user_id from scim_group_members where group_id = $1 \
             order by created_at, user_id",
        )
        .bind(group_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Add a member. Idempotent: an IdP replaying an `add` must not fail.
    pub async fn add_member(&self, group_id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query(
            "insert into scim_group_members (group_id, user_id) values ($1, $2) \
             on conflict do nothing",
        )
        .bind(group_id)
        .bind(user_id)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Remove a member. Idempotent for the same reason.
    pub async fn remove_member(&self, group_id: Uuid, user_id: Uuid) -> Result<()> {
        sqlx::query("delete from scim_group_members where group_id = $1 and user_id = $2")
            .bind(group_id)
            .bind(user_id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// Make the group's members be exactly `user_ids`. This is the `PUT` and
    /// `replace members` path, and re-sending the same list is a no-op.
    pub async fn set_members(&self, group_id: Uuid, user_ids: &[Uuid]) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query(
            "delete from scim_group_members \
             where group_id = $1 and user_id <> all($2::uuid[])",
        )
        .bind(group_id)
        .bind(user_ids)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        for user_id in user_ids {
            sqlx::query(
                "insert into scim_group_members (group_id, user_id) values ($1, $2) \
                 on conflict do nothing",
            )
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(())
    }

    /// Every group in `org_id` the user belongs to — the input to reconciling
    /// the roles their group membership implies.
    pub async fn groups_for_user(&self, org_id: Uuid, user_id: Uuid) -> Result<Vec<ScimGroup>> {
        sqlx::query_as(
            "select g.id, g.org_id, g.external_id, g.display_name, g.created_at, g.updated_at
             from scim_groups g
             join scim_group_members m on m.group_id = g.id
             where g.org_id = $1 and m.user_id = $2
             order by g.created_at, g.id",
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }
}

/// Group→role mappings, the SCIM counterpart of [`SsoRepo::list_mappings`](super::sso::SsoRepo::list_mappings).
pub struct ScimGroupMappingRepo<'a>(pub &'a PgPool);

const SCIM_GROUP_MAPPING_COLUMNS: &str =
    "id, org_id, group_name, team_id, project_id, role, created_at";

impl ScimGroupMappingRepo<'_> {
    pub async fn create(
        &self,
        org_id: Uuid,
        group_name: &str,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
    ) -> Result<ScimGroupMapping> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into scim_group_mappings (org_id, group_name, team_id, project_id, role) \
             values ($1, $2, $3, $4, $5) returning {SCIM_GROUP_MAPPING_COLUMNS}"
        )))
        .bind(org_id)
        .bind(group_name)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimGroupMapping>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_GROUP_MAPPING_COLUMNS} from scim_group_mappings \
             where org_id = $1 order by group_name, created_at"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The mappings a single group name grants.
    pub async fn list_for_group(
        &self,
        org_id: Uuid,
        group_name: &str,
    ) -> Result<Vec<ScimGroupMapping>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SCIM_GROUP_MAPPING_COLUMNS} from scim_group_mappings \
             where org_id = $1 and group_name = $2 order by created_at"
        )))
        .bind(org_id)
        .bind(group_name)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ScimGroupMapping> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {SCIM_GROUP_MAPPING_COLUMNS} from scim_group_mappings where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("scim group mapping {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from scim_group_mappings where id = $1").bind(id),
            self.0,
            || format!("scim group mapping {id}"),
        )
        .await?;
        Ok(())
    }
}
