//! Custom roles and access profiles.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{
    AccessProfile, AccessProfileAssignment, AccessProfilePolicy, AccessProfileRole, CustomRole,
    CustomRoleGrant, EffectiveGrant,
};
use super::support::{fetch_optional_or_not_found, store_err};

/// Every column of `custom_role_grants` that its row type reads.
const CUSTOM_ROLE_GRANT_COLUMNS: &str = "id, role_id, resource, action";

/// One role a profile confers, and where: `(role, org, team, project)`. Exactly
/// one of the three scopes is `Some`, which the schema enforces.
pub type ProfileRoleAssignment = (Uuid, Option<Uuid>, Option<Uuid>, Option<Uuid>);

const CUSTOM_ROLE_COLUMNS: &str =
    "id, org_id, slug, name, description, base_role, created_at, updated_at";

/// Org-scoped custom roles and their explicit `(resource, action)` grants.
pub struct CustomRoleRepo<'a>(pub &'a PgPool);

impl CustomRoleRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<CustomRole>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CUSTOM_ROLE_COLUMNS} from custom_roles where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<CustomRole> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {CUSTOM_ROLE_COLUMNS} from custom_roles where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("custom role {id}"),
        )
        .await
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        slug: &str,
        name: &str,
        description: Option<&str>,
        base_role: &str,
    ) -> Result<CustomRole> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into custom_roles (org_id, slug, name, description, base_role) \
             values ($1, $2, $3, $4, $5) returning {CUSTOM_ROLE_COLUMNS}"
        )))
        .bind(org_id)
        .bind(slug)
        .bind(name)
        .bind(description)
        .bind(base_role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: Option<&str>,
        base_role: &str,
    ) -> Result<CustomRole> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update custom_roles set name = $2, description = $3, base_role = $4, \
                    updated_at = now() \
             where id = $1 returning {CUSTOM_ROLE_COLUMNS}"
            )))
            .bind(id)
            .bind(name)
            .bind(description)
            .bind(base_role),
            self.0,
            || format!("custom role {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("delete from custom_roles where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    /// How many profile compositions still reference this role. The delete
    /// guard reads this rather than catching a foreign-key violation, so the
    /// API can say *how many* references block the deletion.
    pub async fn reference_count(&self, id: Uuid) -> Result<i64> {
        sqlx::query_scalar("select count(*) from access_profile_roles where role_id = $1")
            .bind(id)
            .fetch_one(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn list_grants(&self, role_id: Uuid) -> Result<Vec<CustomRoleGrant>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CUSTOM_ROLE_GRANT_COLUMNS} from custom_role_grants \
             where role_id = $1 order by resource, action"
        )))
        .bind(role_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every grant belonging to any of `role_ids`, so the matrix can render a
    /// whole org's custom roles without a query per role.
    pub async fn list_grants_for_roles(&self, role_ids: &[Uuid]) -> Result<Vec<CustomRoleGrant>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CUSTOM_ROLE_GRANT_COLUMNS} from custom_role_grants \
             where role_id = any($1) order by resource, action"
        )))
        .bind(role_ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Replace a role's grants wholesale, in one transaction: an update that
    /// deleted and re-inserted outside a transaction would briefly leave the
    /// role granting nothing, and a concurrent request would see it.
    pub async fn set_grants(&self, role_id: Uuid, grants: &[(String, String)]) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from custom_role_grants where role_id = $1")
            .bind(role_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        for (resource, action) in grants {
            sqlx::query(
                "insert into custom_role_grants (role_id, resource, action) values ($1, $2, $3) \
                 on conflict do nothing",
            )
            .bind(role_id)
            .bind(resource)
            .bind(action)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        sqlx::query("update custom_roles set updated_at = now() where id = $1")
            .bind(role_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        tx.commit().await.map_err(store_err)
    }
}

const ACCESS_PROFILE_COLUMNS: &str = "id, org_id, slug, name, description, created_at, updated_at";
const ACCESS_PROFILE_POLICY_COLUMNS: &str = "profile_id, allowed_models, denied_models, \
     allowed_routes, denied_routes, updated_at";

/// Access profiles: the composition of custom roles, who holds them, and the
/// model/route policy they carry.
pub struct AccessProfileRepo<'a>(pub &'a PgPool);

impl AccessProfileRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<AccessProfile>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ACCESS_PROFILE_COLUMNS} from access_profiles where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<AccessProfile> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {ACCESS_PROFILE_COLUMNS} from access_profiles where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("access profile {id}"),
        )
        .await
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        slug: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<AccessProfile> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into access_profiles (org_id, slug, name, description) \
             values ($1, $2, $3, $4) returning {ACCESS_PROFILE_COLUMNS}"
        )))
        .bind(org_id)
        .bind(slug)
        .bind(name)
        .bind(description)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: Option<&str>,
    ) -> Result<AccessProfile> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update access_profiles set name = $2, description = $3, updated_at = now() \
             where id = $1 returning {ACCESS_PROFILE_COLUMNS}"
            )))
            .bind(id)
            .bind(name)
            .bind(description),
            self.0,
            || format!("access profile {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query("delete from access_profiles where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    pub async fn list_roles(&self, profile_id: Uuid) -> Result<Vec<AccessProfileRole>> {
        sqlx::query_as(
            "select id, profile_id, role_id, org_id, team_id, project_id, created_at \
             from access_profile_roles where profile_id = $1 order by created_at",
        )
        .bind(profile_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The composition of every profile in `org_id`, in one read, ordered by
    /// profile and then as [`AccessProfileRepo::list_roles`] orders one profile.
    /// The profiles list embeds it (`?include=roles`) so a screen of N profiles
    /// costs one query rather than N.
    pub async fn list_org_roles(&self, org_id: Uuid) -> Result<Vec<AccessProfileRole>> {
        sqlx::query_as(
            "select r.id, r.profile_id, r.role_id, r.org_id, r.team_id, r.project_id, r.created_at \
             from access_profile_roles r join access_profiles p on p.id = r.profile_id \
             where p.org_id = $1 order by r.profile_id, r.created_at",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The policy of every profile in `org_id` that has one, in one read.
    pub async fn list_org_policies(&self, org_id: Uuid) -> Result<Vec<AccessProfilePolicy>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ACCESS_PROFILE_POLICY_COLUMNS} from access_profile_policies \
             where profile_id in (select id from access_profiles where org_id = $1)"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Replace a profile's composition in one transaction, for the same reason
    /// [`CustomRoleRepo::set_grants`] does.
    pub async fn set_roles(&self, profile_id: Uuid, roles: &[ProfileRoleAssignment]) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from access_profile_roles where profile_id = $1")
            .bind(profile_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        for (role_id, org_id, team_id, project_id) in roles {
            sqlx::query(
                "insert into access_profile_roles (profile_id, role_id, org_id, team_id, project_id) \
                 values ($1, $2, $3, $4, $5) on conflict do nothing",
            )
            .bind(profile_id)
            .bind(role_id)
            .bind(org_id)
            .bind(team_id)
            .bind(project_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        sqlx::query("update access_profiles set updated_at = now() where id = $1")
            .bind(profile_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        tx.commit().await.map_err(store_err)
    }

    pub async fn list_assignments(&self, profile_id: Uuid) -> Result<Vec<AccessProfileAssignment>> {
        sqlx::query_as(
            "select id, profile_id, user_id, team_id, created_at \
             from access_profile_assignments where profile_id = $1 order by created_at",
        )
        .bind(profile_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The assignments of every profile in `org_id`, narrowed to one user or one
    /// team when given. The join is the tenancy bound: a subject that also holds
    /// profiles in another org never leaks them through this org's route.
    pub async fn list_org_assignments(
        &self,
        org_id: Uuid,
        user_id: Option<Uuid>,
        team_id: Option<Uuid>,
    ) -> Result<Vec<AccessProfileAssignment>> {
        sqlx::query_as(
            "select a.id, a.profile_id, a.user_id, a.team_id, a.created_at \
             from access_profile_assignments a \
             join access_profiles p on p.id = a.profile_id \
             where p.org_id = $1 \
               and ($2::uuid is null or a.user_id = $2) \
               and ($3::uuid is null or a.team_id = $3) \
             order by a.created_at, a.id",
        )
        .bind(org_id)
        .bind(user_id)
        .bind(team_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_assignment(&self, id: Uuid) -> Result<AccessProfileAssignment> {
        fetch_optional_or_not_found(
            sqlx::query_as(
                "select id, profile_id, user_id, team_id, created_at \
             from access_profile_assignments where id = $1",
            )
            .bind(id),
            self.0,
            || format!("access profile assignment {id}"),
        )
        .await
    }

    /// Assign the profile to a user or a team. Re-assigning the same subject is
    /// a no-op that returns the existing row, so a retried request does not 409.
    pub async fn assign(
        &self,
        profile_id: Uuid,
        user_id: Option<Uuid>,
        team_id: Option<Uuid>,
    ) -> Result<AccessProfileAssignment> {
        if let Some(existing) = sqlx::query_as::<_, AccessProfileAssignment>(
            "select id, profile_id, user_id, team_id, created_at \
             from access_profile_assignments \
             where profile_id = $1 and user_id is not distinct from $2 \
               and team_id is not distinct from $3",
        )
        .bind(profile_id)
        .bind(user_id)
        .bind(team_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        {
            return Ok(existing);
        }
        sqlx::query_as(
            "insert into access_profile_assignments (profile_id, user_id, team_id) \
             values ($1, $2, $3) returning id, profile_id, user_id, team_id, created_at",
        )
        .bind(profile_id)
        .bind(user_id)
        .bind(team_id)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn unassign(&self, id: Uuid) -> Result<()> {
        sqlx::query("delete from access_profile_assignments where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    pub async fn get_policy(&self, profile_id: Uuid) -> Result<Option<AccessProfilePolicy>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ACCESS_PROFILE_POLICY_COLUMNS} from access_profile_policies \
             where profile_id = $1"
        )))
        .bind(profile_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn set_policy(
        &self,
        profile_id: Uuid,
        allowed_models: &[String],
        denied_models: &[String],
        allowed_routes: &[String],
        denied_routes: &[String],
    ) -> Result<AccessProfilePolicy> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into access_profile_policies \
                 (profile_id, allowed_models, denied_models, allowed_routes, denied_routes) \
             values ($1, $2, $3, $4, $5) \
             on conflict (profile_id) do update set \
                 allowed_models = excluded.allowed_models, \
                 denied_models = excluded.denied_models, \
                 allowed_routes = excluded.allowed_routes, \
                 denied_routes = excluded.denied_routes, \
                 updated_at = now() \
             returning {ACCESS_PROFILE_POLICY_COLUMNS}"
        )))
        .bind(profile_id)
        .bind(allowed_models)
        .bind(denied_models)
        .bind(allowed_routes)
        .bind(denied_routes)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Every `(custom role, scope)` a user holds, flattened across the profiles
    /// assigned to them directly and to the teams they belong to — including a
    /// team reached only through a project membership inside it.
    ///
    /// One round trip, and it returns nothing for a deployment with no profiles,
    /// which is why the authorization guard can afford to call it.
    pub async fn effective_grants_for_user(&self, user_id: Uuid) -> Result<Vec<EffectiveGrant>> {
        sqlx::query_as(
            "select apr.profile_id, cr.id as role_id, cr.slug as role_slug, cr.base_role, \
                    apr.org_id, apr.team_id, apr.project_id, g.resource, g.action \
             from access_profile_assignments a \
             join access_profile_roles apr on apr.profile_id = a.profile_id \
             join custom_roles cr on cr.id = apr.role_id \
             left join custom_role_grants g on g.role_id = cr.id \
             where a.user_id = $1 or a.team_id in ( \
                 select m.team_id from memberships m \
                  where m.user_id = $1 and m.team_id is not null \
                 union \
                 select p.team_id from memberships m \
                   join projects p on p.id = m.project_id \
                  where m.user_id = $1 \
             )",
        )
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The model/route policies carried by every profile that reaches a user.
    pub async fn policies_for_user(&self, user_id: Uuid) -> Result<Vec<AccessProfilePolicy>> {
        sqlx::query_as(
            "select distinct p.profile_id, p.allowed_models, p.denied_models, \
                    p.allowed_routes, p.denied_routes, p.updated_at \
             from access_profile_policies p \
             join access_profile_assignments a on a.profile_id = p.profile_id \
             where a.user_id = $1 or a.team_id in ( \
                 select m.team_id from memberships m \
                  where m.user_id = $1 and m.team_id is not null \
                 union \
                 select pr.team_id from memberships m \
                   join projects pr on pr.id = m.project_id \
                  where m.user_id = $1 \
             )",
        )
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }
}
