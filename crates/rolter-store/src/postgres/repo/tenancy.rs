//! Orgs, teams, projects, business units and customers: the tenancy hierarchy.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{
    BusinessUnit, BusinessUnitListing, Customer, CustomerListing, Org, OrgProject, Project, Team,
};
use super::support::{
    fetch_optional_or_not_found, fetch_scalar_or_not_found, require_affected, store_err,
};

/// Every column of `customers` that its row type reads.
const CUSTOMER_COLUMNS: &str = "id, org_id, business_unit_id, name, slug, retired_at, created_at";

/// Every column of `business_units` that its row type reads.
const BUSINESS_UNIT_COLUMNS: &str = "id, org_id, name, slug, retired_at, created_at";

/// Every column of `projects` that its row type reads.
const PROJECT_COLUMNS: &str = "id, team_id, name, created_at";

/// Every column of `teams` that its row type reads.
const TEAM_COLUMNS: &str = "id, org_id, name, created_at";

/// Every column of `orgs` that its row type reads.
const ORG_COLUMNS: &str = "id, name, slug, created_at";

/// Orgs: the top of the org → team → project tenancy hierarchy.
pub struct OrgRepo<'a>(pub &'a PgPool);

impl OrgRepo<'_> {
    pub async fn list(&self) -> Result<Vec<Org>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ORG_COLUMNS} from orgs order by name"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Org> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {ORG_COLUMNS} from orgs where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("org {id}"),
        )
        .await
    }

    pub async fn create(&self, name: &str, slug: &str) -> Result<Org> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into orgs (name, slug) values ($1, $2)
             returning {ORG_COLUMNS}"
        )))
        .bind(name)
        .bind(slug)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from orgs where id = $1").bind(id),
            self.0,
            || format!("org {id}"),
        )
        .await?;
        Ok(())
    }
}

/// Teams, scoped to an org.
pub struct TeamRepo<'a>(pub &'a PgPool);

impl TeamRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Team>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {TEAM_COLUMNS} from teams where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Team> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {TEAM_COLUMNS} from teams where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("team {id}"),
        )
        .await
    }

    pub async fn create(&self, org_id: Uuid, name: &str) -> Result<Team> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into teams (org_id, name) values ($1, $2)
             returning {TEAM_COLUMNS}"
        )))
        .bind(org_id)
        .bind(name)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Rename a team. A name another team of the org holds is `AlreadyExists`.
    pub async fn rename(&self, id: Uuid, name: &str) -> Result<Team> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update teams set name = $2 where id = $1
             returning {TEAM_COLUMNS}"
            )))
            .bind(id)
            .bind(name),
            self.0,
            || format!("team {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from teams where id = $1").bind(id),
            self.0,
            || format!("team {id}"),
        )
        .await?;
        Ok(())
    }
}

/// Projects, scoped to a team.
pub struct ProjectRepo<'a>(pub &'a PgPool);
pub struct BusinessUnitRepo<'a>(pub &'a PgPool);
pub struct CustomerRepo<'a>(pub &'a PgPool);

impl ProjectRepo<'_> {
    pub async fn list(&self, team_id: Uuid) -> Result<Vec<Project>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PROJECT_COLUMNS} from projects where team_id = $1 order by name"
        )))
        .bind(team_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every project in `org_id`, across all of its teams, in one query.
    ///
    /// The alternative is one [`Self::list`] per team, which a caller that only
    /// wants to name a project anywhere in the org pays as N+1 round trips
    /// (#1357). Ordered by team then project so the result is already grouped.
    pub async fn list_for_org(&self, org_id: Uuid) -> Result<Vec<OrgProject>> {
        sqlx::query_as(
            "select p.id, p.team_id, t.name as team_name, p.name, p.created_at
             from projects p join teams t on t.id = p.team_id
             where t.org_id = $1 order by t.name, p.name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Project> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {PROJECT_COLUMNS} from projects where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("project {id}"),
        )
        .await
    }

    /// Whether `project_id` belongs to `org_id`, through its team.
    pub async fn in_org(&self, project_id: Uuid, org_id: Uuid) -> Result<bool> {
        sqlx::query_scalar(
            "select exists(select 1 from projects p join teams t on t.id = p.team_id
                            where p.id = $1 and t.org_id = $2)",
        )
        .bind(project_id)
        .bind(org_id)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// The providers and groups scoped to any of `project_ids` (#1919), named
    /// for the refusal. Deleting a project that still holds one would either
    /// widen it to the whole org or destroy it, so the delete is refused.
    pub async fn scoped_resources(&self, project_ids: &[Uuid]) -> Result<Vec<String>> {
        let providers: Vec<String> = sqlx::query_scalar(
            "select name from providers where project_id = any($1) order by name",
        )
        .bind(project_ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        let groups: Vec<String> = sqlx::query_scalar(
            "select slug from provider_groups where project_id = any($1) order by slug",
        )
        .bind(project_ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(providers
            .into_iter()
            .map(|name| format!("provider '{name}'"))
            .chain(
                groups
                    .into_iter()
                    .map(|slug| format!("provider group '{slug}'")),
            )
            .collect())
    }

    /// The owning team of each of `ids`, in one query.
    ///
    /// Replaces a per-id [`Self::get`] on the authorization path, where the
    /// caller holds a user's project memberships and needs only which team each
    /// belongs to. Unknown ids are simply absent from the result rather than an
    /// error: the caller is asking which of these are in scope, and a project
    /// that no longer exists is not (#1048).
    pub async fn team_ids_for(&self, ids: &[Uuid]) -> Result<Vec<(Uuid, Uuid)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_as("select id, team_id from projects where id = any($1)")
            .bind(ids)
            .fetch_all(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn create(&self, team_id: Uuid, name: &str) -> Result<Project> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into projects (team_id, name) values ($1, $2)
             returning {PROJECT_COLUMNS}"
        )))
        .bind(team_id)
        .bind(name)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from projects where id = $1").bind(id),
            self.0,
            || format!("project {id}"),
        )
        .await?;
        Ok(())
    }

    /// The lowest built-in role that may read the captured request and response
    /// bodies of this project's traffic: `member` unless an admin lowered it to
    /// `viewer` (#1820).
    pub async fn payload_min_role(&self, id: Uuid) -> Result<String> {
        fetch_scalar_or_not_found(
            sqlx::query_scalar("select payload_min_role from projects where id = $1").bind(id),
            self.0,
            || format!("project {id}"),
        )
        .await
    }

    /// Set [`Self::payload_min_role`]. The column's check constraint admits only
    /// `member` and `viewer`, so an unknown role is refused by the database even
    /// if a caller skipped its own validation.
    pub async fn set_payload_min_role(&self, id: Uuid, role: &str) -> Result<()> {
        require_affected(
            sqlx::query("update projects set payload_min_role = $2 where id = $1")
                .bind(id)
                .bind(role),
            self.0,
            || format!("project {id}"),
        )
        .await?;
        Ok(())
    }

    /// The projects under any of these scopes whose admin let viewers read
    /// captured bodies, in one query.
    ///
    /// Bounded by the scopes a caller holds a role in rather than listing every
    /// such project in the deployment, so the answer grows with one user's
    /// reach and not with the number of tenants (#1820).
    pub async fn viewer_payload_projects(
        &self,
        org_ids: &[Uuid],
        team_ids: &[Uuid],
        project_ids: &[Uuid],
    ) -> Result<Vec<Uuid>> {
        if org_ids.is_empty() && team_ids.is_empty() && project_ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_scalar(
            "select p.id from projects p join teams t on t.id = p.team_id
             where p.payload_min_role = 'viewer'
               and (t.org_id = any($1) or p.team_id = any($2) or p.id = any($3))",
        )
        .bind(org_ids)
        .bind(team_ids)
        .bind(project_ids)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }
}

impl BusinessUnitRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<BusinessUnit>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {BUSINESS_UNIT_COLUMNS}
             from business_units where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The org's business units with the count of live virtual keys
    /// attributed to each, in one query: a grouped count joined onto the
    /// units rather than one count per unit.
    pub async fn list_with_key_counts(&self, org_id: Uuid) -> Result<Vec<BusinessUnitListing>> {
        sqlx::query_as(
            "select bu.id, bu.org_id, bu.name, bu.slug, bu.retired_at, bu.created_at,
                    coalesce(k.live_key_count, 0) as live_key_count
             from business_units bu
             left join (
                 select vk.business_unit_id, count(*) as live_key_count
                 from virtual_keys vk
                 join business_units owner on owner.id = vk.business_unit_id
                 where owner.org_id = $1
                   and not vk.disabled
                   and (vk.expires_at is null or vk.expires_at > now())
                 group by vk.business_unit_id
             ) k on k.business_unit_id = bu.id
             where bu.org_id = $1
             order by bu.name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<BusinessUnit> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {BUSINESS_UNIT_COLUMNS}
             from business_units where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("business unit {id}"),
        )
        .await
    }

    pub async fn create(&self, org_id: Uuid, name: &str, slug: &str) -> Result<BusinessUnit> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into business_units (org_id, name, slug) values ($1, $2, $3)
             returning {BUSINESS_UNIT_COLUMNS}"
        )))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        slug: Option<&str>,
        retired: Option<bool>,
    ) -> Result<BusinessUnit> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update business_units set
                 name = coalesce($2, name),
                 slug = coalesce($3, slug),
                 retired_at = case
                    when $4::bool is null then retired_at
                    when $4 then coalesce(retired_at, now())
                    else null
                 end
             where id = $1
             returning {BUSINESS_UNIT_COLUMNS}"
            )))
            .bind(id)
            .bind(name)
            .bind(slug)
            .bind(retired),
            self.0,
            || format!("business unit {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from business_units where id = $1").bind(id),
            self.0,
            || format!("business unit {id}"),
        )
        .await?;
        Ok(())
    }
}

impl CustomerRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Customer>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CUSTOMER_COLUMNS}
             from customers where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// The org's customers with the count of live virtual keys attributed to
    /// each, in one query: a grouped count joined onto the customers rather
    /// than one count per customer.
    pub async fn list_with_key_counts(&self, org_id: Uuid) -> Result<Vec<CustomerListing>> {
        sqlx::query_as(
            "select c.id, c.org_id, c.business_unit_id, c.name, c.slug, c.retired_at,
                    c.created_at, coalesce(k.live_key_count, 0) as live_key_count
             from customers c
             left join (
                 select vk.customer_id, count(*) as live_key_count
                 from virtual_keys vk
                 join customers owner on owner.id = vk.customer_id
                 where owner.org_id = $1
                   and not vk.disabled
                   and (vk.expires_at is null or vk.expires_at > now())
                 group by vk.customer_id
             ) k on k.customer_id = c.id
             where c.org_id = $1
             order by c.name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Customer> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {CUSTOMER_COLUMNS}
             from customers where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("customer {id}"),
        )
        .await
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        business_unit_id: Option<Uuid>,
        name: &str,
        slug: &str,
    ) -> Result<Customer> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into customers (org_id, business_unit_id, name, slug)
             values ($1, $2, $3, $4)
             returning {CUSTOMER_COLUMNS}"
        )))
        .bind(org_id)
        .bind(business_unit_id)
        .bind(name)
        .bind(slug)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        business_unit_id: Option<Option<Uuid>>,
        name: Option<&str>,
        slug: Option<&str>,
        retired: Option<bool>,
    ) -> Result<Customer> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update customers set
                 business_unit_id = case when $2 then $3 else business_unit_id end,
                 name = coalesce($4, name),
                 slug = coalesce($5, slug),
                 retired_at = case
                    when $6::bool is null then retired_at
                    when $6 then coalesce(retired_at, now())
                    else null
                 end
             where id = $1
             returning {CUSTOMER_COLUMNS}"
            )))
            .bind(id)
            .bind(business_unit_id.is_some())
            .bind(business_unit_id.flatten())
            .bind(name)
            .bind(slug)
            .bind(retired),
            self.0,
            || format!("customer {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from customers where id = $1").bind(id),
            self.0,
            || format!("customer {id}"),
        )
        .await?;
        Ok(())
    }
}
