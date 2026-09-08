//! Thin CRUD repositories over the `postgres` feature's schema. Each
//! repository wraps a [`PgPool`] and exposes `list`/`get`/`create`/`delete`
//! (plus narrow `update`s where a resource has mutable fields worth editing
//! independently). Domain interpretation of row contents (e.g. balancer
//! strategy parsing) is left to callers; see [`super::PostgresConfigStore`]
//! for the read path the gateway uses.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::models::{
    AccessProfile, AccessProfileAssignment, AccessProfilePolicy, AccessProfileRole,
    AdaptiveRoutingPolicy, AdaptiveRoutingTelemetry, AuditLogEntry, Budget, BusinessUnit,
    ClientSettings, ClusterNode, CompatibilityPolicy, CustomRole, CustomRoleGrant, Customer,
    EffectiveGrant, FeatureFlags, GuardrailProvider, GuardrailRule, Invitation, LoggingSettings,
    McpGatewaySettings, McpLoginState, McpOAuthGrant, McpOAuthSession, McpServer, McpToolGroup,
    Membership, ModelDefaults, ModelPrice, Org, OrgAuthPolicy, OwnedVirtualKey, PluginInstance,
    Project, PromptTemplate, PromptTemplateScope, PromptTemplateVersion, Provider, ProviderGroup,
    ProviderGroupMember, RateLimit, Route, RouteTarget, RuntimePolicy, ScimGroup, ScimGroupMapping,
    ScimIdentity, ScimToken, SecuritySettings, Session, Skill, SkillVersion, SsoGroupMapping,
    SsoLoginState, SsoProvider, Team, User, VirtualKey,
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

impl CompatibilityPolicyRepo<'_> {
    pub async fn get(&self) -> Result<CompatibilityPolicy> {
        sqlx::query_as(
            "select anthropic_version, default_max_tokens, updated_at \
             from compatibility_policy where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        anthropic_version: &str,
        default_max_tokens: i32,
    ) -> Result<CompatibilityPolicy> {
        sqlx::query_as(
            "update compatibility_policy set \
                anthropic_version = $1, default_max_tokens = $2, updated_at = now() \
             where id = true \
             returning anthropic_version, default_max_tokens, updated_at",
        )
        .bind(anthropic_version)
        .bind(default_max_tokens)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound("compatibility policy".to_string()))
    }
}

const CLIENT_SETTINGS_COLUMNS: &str = "public_base_url, forwarded_headers, injected_headers, \
     request_id_header, updated_at";

impl ClientSettingsRepo<'_> {
    pub async fn get(&self) -> Result<ClientSettings> {
        sqlx::query_as(&format!(
            "select {CLIENT_SETTINGS_COLUMNS} from client_settings where id = true"
        ))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        public_base_url: Option<&str>,
        forwarded_headers: &[String],
        injected_headers: &serde_json::Value,
        request_id_header: &str,
    ) -> Result<ClientSettings> {
        sqlx::query_as(&format!(
            "update client_settings set \
                public_base_url = $1, forwarded_headers = $2, injected_headers = $3, \
                request_id_header = $4, updated_at = now() \
             where id = true \
             returning {CLIENT_SETTINGS_COLUMNS}"
        ))
        .bind(public_base_url)
        .bind(forwarded_headers)
        .bind(injected_headers)
        .bind(request_id_header)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound("client settings".to_string()))
    }
}

const MODEL_DEFAULTS_COLUMNS: &str = "enabled, default_model, default_temperature, \
     default_top_p, default_max_tokens, updated_at";

impl ModelDefaultsRepo<'_> {
    pub async fn get(&self) -> Result<ModelDefaults> {
        sqlx::query_as(&format!(
            "select {MODEL_DEFAULTS_COLUMNS} from model_defaults where id = true"
        ))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        enabled: bool,
        default_model: Option<&str>,
        default_temperature: Option<f64>,
        default_top_p: Option<f64>,
        default_max_tokens: Option<i32>,
    ) -> Result<ModelDefaults> {
        sqlx::query_as(&format!(
            "update model_defaults set \
                enabled = $1, default_model = $2, default_temperature = $3, \
                default_top_p = $4, default_max_tokens = $5, updated_at = now() \
             where id = true \
             returning {MODEL_DEFAULTS_COLUMNS}"
        ))
        .bind(enabled)
        .bind(default_model)
        .bind(default_temperature)
        .bind(default_top_p)
        .bind(default_max_tokens)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound("model defaults".to_string()))
    }
}

const ADAPTIVE_POLICY_COLUMNS: &str = "enabled, latency_weight, cost_weight, load_weight, \
     exploration_ratio, min_samples, updated_at";

impl AdaptiveRoutingPolicyRepo<'_> {
    pub async fn get(&self) -> Result<AdaptiveRoutingPolicy> {
        sqlx::query_as(&format!(
            "select {ADAPTIVE_POLICY_COLUMNS} from adaptive_routing_policy where id = true"
        ))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        enabled: bool,
        latency_weight: f32,
        cost_weight: f32,
        load_weight: f32,
        exploration_ratio: f32,
        min_samples: i32,
    ) -> Result<AdaptiveRoutingPolicy> {
        sqlx::query_as(&format!(
            "update adaptive_routing_policy set \
                enabled = $1, latency_weight = $2, cost_weight = $3, load_weight = $4, \
                exploration_ratio = $5, min_samples = $6, updated_at = now() \
             where id = true returning {ADAPTIVE_POLICY_COLUMNS}"
        ))
        .bind(enabled)
        .bind(latency_weight)
        .bind(cost_weight)
        .bind(load_weight)
        .bind(exploration_ratio)
        .bind(min_samples)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound("adaptive routing policy".to_string()))
    }
}

impl RuntimePolicyRepo<'_> {
    pub async fn get(&self) -> Result<RuntimePolicy> {
        sqlx::query_as(
            "select retry_max_retries, retry_base_ms, retry_max_ms, timeout_connect_s, \
                    timeout_request_s, queue_enabled, queue_capacity, queue_workers, \
                    queue_backpressure, queue_block_ms, updated_at \
             from runtime_policy where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        retry_max_retries: i32,
        retry_base_ms: i32,
        retry_max_ms: i32,
        timeout_connect_s: i32,
        timeout_request_s: i32,
        queue_enabled: bool,
        queue_capacity: i32,
        queue_workers: i32,
        queue_backpressure: &str,
        queue_block_ms: i32,
    ) -> Result<RuntimePolicy> {
        sqlx::query_as(
            "update runtime_policy set \
                retry_max_retries = $1, retry_base_ms = $2, retry_max_ms = $3, \
                timeout_connect_s = $4, timeout_request_s = $5, queue_enabled = $6, \
                queue_capacity = $7, queue_workers = $8, queue_backpressure = $9, \
                queue_block_ms = $10, updated_at = now() \
             where id = true \
             returning retry_max_retries, retry_base_ms, retry_max_ms, timeout_connect_s, \
                       timeout_request_s, queue_enabled, queue_capacity, queue_workers, \
                       queue_backpressure, queue_block_ms, updated_at",
        )
        .bind(retry_max_retries)
        .bind(retry_base_ms)
        .bind(retry_max_ms)
        .bind(timeout_connect_s)
        .bind(timeout_request_s)
        .bind(queue_enabled)
        .bind(queue_capacity)
        .bind(queue_workers)
        .bind(queue_backpressure)
        .bind(queue_block_ms)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
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
pub struct BusinessUnitRepo<'a>(pub &'a PgPool);
pub struct CustomerRepo<'a>(pub &'a PgPool);

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

impl BusinessUnitRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<BusinessUnit>> {
        sqlx::query_as(
            "select id, org_id, name, slug, retired_at, created_at
             from business_units where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<BusinessUnit> {
        sqlx::query_as(
            "select id, org_id, name, slug, retired_at, created_at
             from business_units where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("business unit {id}")))
    }

    pub async fn create(&self, org_id: Uuid, name: &str, slug: &str) -> Result<BusinessUnit> {
        sqlx::query_as(
            "insert into business_units (org_id, name, slug) values ($1, $2, $3)
             returning id, org_id, name, slug, retired_at, created_at",
        )
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
        sqlx::query_as(
            "update business_units set
                 name = coalesce($2, name),
                 slug = coalesce($3, slug),
                 retired_at = case
                    when $4::bool is null then retired_at
                    when $4 then coalesce(retired_at, now())
                    else null
                 end
             where id = $1
             returning id, org_id, name, slug, retired_at, created_at",
        )
        .bind(id)
        .bind(name)
        .bind(slug)
        .bind(retired)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("business unit {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from business_units where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("business unit {id}")));
        }
        Ok(())
    }
}

impl CustomerRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Customer>> {
        sqlx::query_as(
            "select id, org_id, business_unit_id, name, slug, retired_at, created_at
             from customers where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<Customer> {
        sqlx::query_as(
            "select id, org_id, business_unit_id, name, slug, retired_at, created_at
             from customers where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("customer {id}")))
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        business_unit_id: Option<Uuid>,
        name: &str,
        slug: &str,
    ) -> Result<Customer> {
        sqlx::query_as(
            "insert into customers (org_id, business_unit_id, name, slug)
             values ($1, $2, $3, $4)
             returning id, org_id, business_unit_id, name, slug, retired_at, created_at",
        )
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
        sqlx::query_as(
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
             returning id, org_id, business_unit_id, name, slug, retired_at, created_at",
        )
        .bind(id)
        .bind(business_unit_id.is_some())
        .bind(business_unit_id.flatten())
        .bind(name)
        .bind(slug)
        .bind(retired)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("customer {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from customers where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("customer {id}")));
        }
        Ok(())
    }
}

/// Prompt templates with immutable versions and explicit scope assignments.
pub struct PromptTemplateRepo<'a>(pub &'a PgPool);

impl PromptTemplateRepo<'_> {
    pub async fn list_templates(&self, org_id: Uuid) -> Result<Vec<PromptTemplate>> {
        sqlx::query_as(
            "select id, org_id, name, slug, description, published_version, created_at
             from prompt_templates where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_template(&self, id: Uuid) -> Result<PromptTemplate> {
        sqlx::query_as(
            "select id, org_id, name, slug, description, published_version, created_at
             from prompt_templates where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("prompt template {id}")))
    }

    pub async fn create_template(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        description: Option<&str>,
    ) -> Result<PromptTemplate> {
        sqlx::query_as(
            "insert into prompt_templates (org_id, name, slug, description)
             values ($1, $2, $3, $4)
             returning id, org_id, name, slug, description, published_version, created_at",
        )
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(description.unwrap_or(""))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update_template(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<PromptTemplate> {
        sqlx::query_as(
            "update prompt_templates set
                 name = coalesce($2, name),
                 description = case when $3::bool then $4 else description end
             where id = $1
             returning id, org_id, name, slug, description, published_version, created_at",
        )
        .bind(id)
        .bind(name)
        .bind(description.is_some())
        .bind(description.unwrap_or(""))
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("prompt template {id}")))
    }

    pub async fn create_version(
        &self,
        template_id: Uuid,
        variables: &serde_json::Value,
        decorators: &serde_json::Value,
    ) -> Result<PromptTemplateVersion> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("select pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(template_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        let version = sqlx::query_as(
            "insert into prompt_template_versions (template_id, version, variables, decorators)
             values (
                 $1,
                 coalesce((select max(version) + 1 from prompt_template_versions where template_id = $1), 1),
                 $2,
                 $3
             )
             returning template_id, version, variables, decorators, created_at",
        )
        .bind(template_id)
        .bind(variables)
        .bind(decorators)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(version)
    }

    pub async fn list_versions(&self, template_id: Uuid) -> Result<Vec<PromptTemplateVersion>> {
        sqlx::query_as(
            "select template_id, version, variables, decorators, created_at
             from prompt_template_versions where template_id = $1 order by version desc",
        )
        .bind(template_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn publish_version(&self, template_id: Uuid, version: i32) -> Result<PromptTemplate> {
        let exists: Option<i32> = sqlx::query_scalar(
            "select 1 from prompt_template_versions where template_id = $1 and version = $2",
        )
        .bind(template_id)
        .bind(version)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        if exists.is_none() {
            return Err(Error::NotFound(format!(
                "prompt template version {template_id}:{version}"
            )));
        }
        sqlx::query_as(
            "update prompt_templates
             set published_version = $2
             where id = $1
             returning id, org_id, name, slug, description, published_version, created_at",
        )
        .bind(template_id)
        .bind(version)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("prompt template {template_id}")))
    }

    /// Replace all scope bindings for a template version.
    pub async fn set_scopes(
        &self,
        template_id: Uuid,
        version: i32,
        scopes: &[(String, Uuid)],
    ) -> Result<()> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("delete from prompt_template_scopes where template_id = $1 and version = $2")
            .bind(template_id)
            .bind(version)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        for (scope_type, scope_id) in scopes {
            sqlx::query(
                "insert into prompt_template_scopes (
                     template_id, version, scope_type, scope_id,
                     org_id, project_id, route_id, virtual_key_id
                 )
                 values (
                     $1, $2, $3, $4,
                     case when $3 = 'org' then $4 end,
                     case when $3 = 'project' then $4 end,
                     case when $3 = 'route' then $4 end,
                     case when $3 = 'virtual_key' then $4 end
                 )",
            )
            .bind(template_id)
            .bind(version)
            .bind(scope_type)
            .bind(scope_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        tx.commit().await.map_err(store_err)?;
        Ok(())
    }

    pub async fn list_scopes(
        &self,
        template_id: Uuid,
        version: i32,
    ) -> Result<Vec<PromptTemplateScope>> {
        sqlx::query_as(
            "select template_id, version, scope_type, scope_id, created_at
             from prompt_template_scopes
             where template_id = $1 and version = $2
             order by scope_type, scope_id",
        )
        .bind(template_id)
        .bind(version)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete_template(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from prompt_templates where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("prompt template {id}")));
        }
        Ok(())
    }
}

pub struct PluginRepo<'a>(pub &'a PgPool);

const PLUGIN_COLUMNS: &str = "id, org_id, project_id, name, slug, description, kind, stage, \
    enabled, position, failure_mode, endpoint, secret_env, config, created_at, updated_at";

impl PluginRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<PluginInstance>> {
        sqlx::query_as(&format!(
            "select {PLUGIN_COLUMNS} from plugin_instances where org_id=$1 order by position, name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every enabled instance across every org, for the gateway snapshot
    /// (#509). `kind = 'webhook'` is the only kind the registry accepts today,
    /// but the filter is explicit so a future non-webhook kind does not
    /// silently reach a dispatcher that only knows how to call a URL.
    pub async fn list_all_enabled(&self) -> Result<Vec<PluginInstance>> {
        sqlx::query_as(&format!(
            "select {PLUGIN_COLUMNS} from plugin_instances \
             where enabled and kind = 'webhook' order by org_id, position, name"
        ))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<PluginInstance> {
        sqlx::query_as(&format!(
            "select {PLUGIN_COLUMNS} from plugin_instances where id=$1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("plugin {id}")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: Uuid,
        project_id: Option<Uuid>,
        name: &str,
        slug: &str,
        description: &str,
        kind: &str,
        stage: &str,
        enabled: bool,
        position: i32,
        failure_mode: &str,
        endpoint: &str,
        secret_env: Option<&str>,
        config: &serde_json::Value,
    ) -> Result<PluginInstance> {
        sqlx::query_as(&format!(
            "insert into plugin_instances (org_id, project_id, name, slug, description, kind, \
             stage, enabled, position, failure_mode, endpoint, secret_env, config) values \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) returning {PLUGIN_COLUMNS}"
        ))
        .bind(org_id)
        .bind(project_id)
        .bind(name)
        .bind(slug)
        .bind(description)
        .bind(kind)
        .bind(stage)
        .bind(enabled)
        .bind(position)
        .bind(failure_mode)
        .bind(endpoint)
        .bind(secret_env)
        .bind(config)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: Uuid,
        project_id: Option<Uuid>,
        name: &str,
        description: &str,
        stage: &str,
        enabled: bool,
        position: i32,
        failure_mode: &str,
        endpoint: &str,
        secret_env: Option<&str>,
        config: &serde_json::Value,
    ) -> Result<PluginInstance> {
        sqlx::query_as(&format!(
            "update plugin_instances set project_id=$2, name=$3, description=$4, stage=$5, \
             enabled=$6, position=$7, failure_mode=$8, endpoint=$9, secret_env=$10, config=$11, \
             updated_at=now() where id=$1 returning {PLUGIN_COLUMNS}"
        ))
        .bind(id)
        .bind(project_id)
        .bind(name)
        .bind(description)
        .bind(stage)
        .bind(enabled)
        .bind(position)
        .bind(failure_mode)
        .bind(endpoint)
        .bind(secret_env)
        .bind(config)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("plugin {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("delete from plugin_instances where id=$1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("plugin {id}")));
        }
        Ok(())
    }
}

pub struct SkillRepo<'a>(pub &'a PgPool);

impl SkillRepo<'_> {
    pub async fn list_skills(&self, org_id: Uuid) -> Result<Vec<Skill>> {
        sqlx::query_as(
            "select id, org_id, name, slug, description, retired_at, published_version,
                    allowed_team_ids, minimum_role, created_at
             from skills where org_id = $1 order by name",
        )
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_skill(&self, id: Uuid) -> Result<Skill> {
        sqlx::query_as(
            "select id, org_id, name, slug, description, retired_at, published_version,
                    allowed_team_ids, minimum_role, created_at
             from skills where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("skill {id}")))
    }

    pub async fn get_by_slug(&self, org_id: Uuid, slug: &str) -> Result<Skill> {
        sqlx::query_as(
            "select id, org_id, name, slug, description, retired_at, published_version,
                    allowed_team_ids, minimum_role, created_at
             from skills where org_id = $1 and slug = $2",
        )
        .bind(org_id)
        .bind(slug)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("skill {org_id}:{slug}")))
    }

    pub async fn create_skill(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        description: Option<&str>,
        allowed_team_ids: &[Uuid],
        minimum_role: &str,
    ) -> Result<Skill> {
        sqlx::query_as(
            "insert into skills (
                 org_id, name, slug, description, allowed_team_ids, minimum_role
             )
             values ($1, $2, $3, $4, $5, $6)
             returning id, org_id, name, slug, description, retired_at, published_version,
                       allowed_team_ids, minimum_role, created_at",
        )
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(description.unwrap_or(""))
        .bind(allowed_team_ids)
        .bind(minimum_role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update_skill(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        retired: Option<bool>,
        allowed_team_ids: Option<&[Uuid]>,
        minimum_role: Option<&str>,
    ) -> Result<Skill> {
        sqlx::query_as(
            "update skills set
                 name = coalesce($2, name),
                 description = case when $3::bool then $4 else description end,
                 retired_at = case
                    when $5::bool is null then retired_at
                    when $5 then coalesce(retired_at, now())
                    else null
                 end,
                 allowed_team_ids = coalesce($6, allowed_team_ids),
                 minimum_role = coalesce($7, minimum_role)
             where id = $1
             returning id, org_id, name, slug, description, retired_at, published_version,
                       allowed_team_ids, minimum_role, created_at",
        )
        .bind(id)
        .bind(name)
        .bind(description.is_some())
        .bind(description.unwrap_or(""))
        .bind(retired)
        .bind(allowed_team_ids)
        .bind(minimum_role)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("skill {id}")))
    }

    pub async fn create_version(
        &self,
        skill_id: Uuid,
        content: Option<&str>,
        content_ref: Option<&str>,
        metadata: &serde_json::Value,
    ) -> Result<SkillVersion> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query("select pg_advisory_xact_lock(hashtextextended($1::text, 0))")
            .bind(skill_id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        let version = sqlx::query_as(
            "insert into skill_versions (skill_id, version, content, content_ref, metadata)
             values (
                 $1,
                 coalesce((select max(version) + 1 from skill_versions where skill_id = $1), 1),
                 $2,
                 $3,
                 $4
             )
             returning skill_id, version, content, content_ref, metadata, created_at",
        )
        .bind(skill_id)
        .bind(content)
        .bind(content_ref)
        .bind(metadata)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(version)
    }

    pub async fn list_versions(&self, skill_id: Uuid) -> Result<Vec<SkillVersion>> {
        sqlx::query_as(
            "select skill_id, version, content, content_ref, metadata, created_at
             from skill_versions where skill_id = $1 order by version desc",
        )
        .bind(skill_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn resolve_published(&self, skill_id: Uuid) -> Result<SkillVersion> {
        sqlx::query_as(
            "select v.skill_id, v.version, v.content, v.content_ref, v.metadata, v.created_at
             from skill_versions v
             join skills s
               on s.id = v.skill_id
              and s.published_version = v.version
             where s.id = $1 and s.retired_at is null",
        )
        .bind(skill_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("published skill {skill_id}")))
    }

    pub async fn publish_version(&self, skill_id: Uuid, version: i32) -> Result<Skill> {
        let exists: Option<i32> =
            sqlx::query_scalar("select 1 from skill_versions where skill_id = $1 and version = $2")
                .bind(skill_id)
                .bind(version)
                .fetch_optional(self.0)
                .await
                .map_err(store_err)?;
        if exists.is_none() {
            return Err(Error::NotFound(format!(
                "skill version {skill_id}:{version}"
            )));
        }
        sqlx::query_as(
            "update skills
             set published_version = $2
             where id = $1
             returning id, org_id, name, slug, description, retired_at, published_version,
                       allowed_team_ids, minimum_role, created_at",
        )
        .bind(skill_id)
        .bind(version)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("skill {skill_id}")))
    }

    pub async fn delete_skill(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from skills where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("skill {id}")));
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

/// OIDC identity providers, their group→role mappings, and the short-lived
/// state rows that make the authorization-code flow replay-safe.
pub struct SsoRepo<'a>(pub &'a PgPool);

const SSO_PROVIDER_COLUMNS: &str = "id, org_id, name, slug, issuer, client_id, secret_ciphertext, \
     secret_nonce, scopes, group_claim, default_role, enabled, created_at";

/// What to do with the sealed client secret on an update: leave the stored one
/// alone, clear it (a provider that becomes a public PKCE client), or replace
/// it with freshly sealed bytes (a rotation at the IdP).
#[derive(Debug, Clone, Copy)]
pub enum SecretUpdate<'a> {
    Keep,
    Clear,
    Set(&'a [u8], &'a [u8]),
}

/// The mutable half of an `sso_providers` row.
///
/// A struct rather than an argument list because the row has eight editable
/// columns and four of them are strings - positional arguments there are a
/// silent swap waiting to happen.
#[derive(Debug, Clone, Copy)]
pub struct SsoProviderUpdate<'a> {
    pub name: &'a str,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub secret: SecretUpdate<'a>,
    pub scopes: &'a [String],
    pub group_claim: &'a str,
    pub default_role: Option<&'a str>,
    pub enabled: bool,
}

impl SsoRepo<'_> {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_provider(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        issuer: &str,
        client_id: &str,
        secret: Option<(&[u8], &[u8])>,
        scopes: &[String],
        group_claim: &str,
        default_role: Option<&str>,
    ) -> Result<SsoProvider> {
        let (ciphertext, nonce) = match secret {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        sqlx::query_as(&format!(
            "insert into sso_providers (org_id, name, slug, issuer, client_id, \
                    secret_ciphertext, secret_nonce, scopes, group_claim, default_role) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             returning {SSO_PROVIDER_COLUMNS}"
        ))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(issuer)
        .bind(client_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(scopes)
        .bind(group_claim)
        .bind(default_role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// Update the mutable half of a provider row in place.
    ///
    /// `slug` is deliberately not updatable: it is in the login URL, so
    /// changing it would break every bookmark, IdP redirect and org runbook
    /// already pointing at it (#1233).
    ///
    /// The sealed secret is three-valued - kept, cleared, or replaced - which
    /// the `case when $5` pair expresses in one statement, so a rotation can
    /// never leave the row with a ciphertext from one write and a nonce from
    /// another.
    pub async fn update_provider(
        &self,
        id: Uuid,
        update: SsoProviderUpdate<'_>,
    ) -> Result<SsoProvider> {
        let (replace, ciphertext, nonce) = match update.secret {
            SecretUpdate::Keep => (false, None, None),
            SecretUpdate::Clear => (true, None, None),
            SecretUpdate::Set(c, n) => (true, Some(c), Some(n)),
        };
        sqlx::query_as(&format!(
            "update sso_providers set \
                    name = $2, issuer = $3, client_id = $4, \
                    secret_ciphertext = case when $5 then $6 else secret_ciphertext end, \
                    secret_nonce = case when $5 then $7 else secret_nonce end, \
                    scopes = $8, group_claim = $9, default_role = $10, enabled = $11 \
             where id = $1 \
             returning {SSO_PROVIDER_COLUMNS}"
        ))
        .bind(id)
        .bind(update.name)
        .bind(update.issuer)
        .bind(update.client_id)
        .bind(replace)
        .bind(ciphertext)
        .bind(nonce)
        .bind(update.scopes)
        .bind(update.group_claim)
        .bind(update.default_role)
        .bind(update.enabled)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("sso provider {id}")))
    }

    pub async fn list_providers(&self, org_id: Uuid) -> Result<Vec<SsoProvider>> {
        sqlx::query_as(&format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_provider(&self, id: Uuid) -> Result<SsoProvider> {
        sqlx::query_as(&format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("sso provider {id}")))
    }

    /// Resolve the provider a login URL names. Slugs are unique per org, and a
    /// login URL carries no org, so a duplicate slug across orgs is ambiguous
    /// and rejected rather than resolved arbitrarily.
    pub async fn find_provider_by_slug(&self, slug: &str) -> Result<Option<SsoProvider>> {
        let mut rows: Vec<SsoProvider> = sqlx::query_as(&format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers where slug = $1 and enabled limit 2"
        ))
        .bind(slug)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        if rows.len() > 1 {
            return Err(Error::Store(format!(
                "sso slug '{slug}' is registered by more than one org; log in through the \
                 org-specific url"
            )));
        }
        Ok(rows.pop())
    }

    /// every enabled provider across all orgs, for the login screen. Returns
    /// names and slugs the login URL already exposes; never secrets.
    pub async fn list_enabled_providers(&self) -> Result<Vec<SsoProvider>> {
        sqlx::query_as(&format!(
            "select {SSO_PROVIDER_COLUMNS} from sso_providers where enabled order by name"
        ))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete_provider(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from sso_providers where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("sso provider {id}")));
        }
        Ok(())
    }

    /// Open the sealed client secret. Returns `None` for a public client.
    pub async fn client_secret(
        &self,
        kek: &super::crypto::Kek,
        provider: &SsoProvider,
    ) -> Result<Option<String>> {
        match (&provider.secret_ciphertext, &provider.secret_nonce) {
            (Some(c), Some(n)) => Ok(Some(kek.decrypt(c, n)?)),
            _ => Ok(None),
        }
    }

    pub async fn add_mapping(
        &self,
        provider_id: Uuid,
        group_name: &str,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
    ) -> Result<SsoGroupMapping> {
        sqlx::query_as(
            "insert into sso_group_mappings (provider_id, group_name, org_id, team_id, \
                    project_id, role) \
             values ($1, $2, $3, $4, $5, $6) \
             returning id, provider_id, group_name, org_id, team_id, project_id, role, created_at",
        )
        .bind(provider_id)
        .bind(group_name)
        .bind(org_id)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list_mappings(&self, provider_id: Uuid) -> Result<Vec<SsoGroupMapping>> {
        sqlx::query_as(
            "select id, provider_id, group_name, org_id, team_id, project_id, role, created_at \
             from sso_group_mappings where provider_id = $1 order by group_name",
        )
        .bind(provider_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn delete_mapping(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from sso_group_mappings where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("sso group mapping {id}")));
        }
        Ok(())
    }

    pub async fn start_login(
        &self,
        state: &str,
        provider_id: Uuid,
        code_verifier: &str,
        nonce: &str,
        redirect_uri: &str,
    ) -> Result<()> {
        sqlx::query(
            "insert into sso_login_states (state, provider_id, code_verifier, nonce, redirect_uri) \
             values ($1, $2, $3, $4, $5)",
        )
        .bind(state)
        .bind(provider_id)
        .bind(code_verifier)
        .bind(nonce)
        .bind(redirect_uri)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Consume a login state exactly once. A replayed callback finds nothing,
    /// so an intercepted `code`+`state` pair cannot be redeemed twice. States
    /// older than `max_age_secs` are treated as absent and swept.
    pub async fn consume_login(
        &self,
        state: &str,
        max_age_secs: i64,
    ) -> Result<Option<SsoLoginState>> {
        // opportunistic sweep: expired rows are worthless and unbounded growth
        // would be the only other outcome
        let _ = sqlx::query(
            "delete from sso_login_states where created_at < now() - ($1 || ' seconds')::interval",
        )
        .bind(max_age_secs.to_string())
        .execute(self.0)
        .await;
        sqlx::query_as(
            "delete from sso_login_states where state = $1 \
             returning state, provider_id, code_verifier, nonce, redirect_uri, created_at",
        )
        .bind(state)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }
}

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
        sqlx::query_as(&format!(
            "insert into scim_tokens (org_id, name, token_hash, created_by) \
             values ($1, $2, $3, $4) returning {SCIM_TOKEN_COLUMNS}"
        ))
        .bind(org_id)
        .bind(name)
        .bind(token_hash)
        .bind(created_by)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimToken>> {
        sqlx::query_as(&format!(
            "select {SCIM_TOKEN_COLUMNS} from scim_tokens where org_id = $1 \
             order by created_at desc"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ScimToken> {
        sqlx::query_as(&format!(
            "select {SCIM_TOKEN_COLUMNS} from scim_tokens where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("scim token {id}")))
    }

    /// Resolve a presented token by digest. Revoked tokens never resolve, so a
    /// revocation takes effect on the next request with no cache to invalidate.
    pub async fn find_active_by_hash(&self, token_hash: &str) -> Result<Option<ScimToken>> {
        sqlx::query_as(&format!(
            "select {SCIM_TOKEN_COLUMNS} from scim_tokens \
             where token_hash = $1 and revoked_at is null"
        ))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<ScimToken> {
        sqlx::query_as(&format!(
            "update scim_tokens set revoked_at = coalesce(revoked_at, now()) \
             where id = $1 returning {SCIM_TOKEN_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("scim token {id}")))
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
        sqlx::query_as(&format!(
            "insert into scim_identities (user_id, org_id, external_id, user_name, display_name) \
             values ($1, $2, $3, $4, $5) \
             on conflict (user_id, org_id) do update set \
                 external_id = excluded.external_id, user_name = excluded.user_name, \
                 display_name = excluded.display_name, updated_at = now() \
             returning {SCIM_IDENTITY_COLUMNS}"
        ))
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
        sqlx::query_as(&format!(
            "select {SCIM_IDENTITY_COLUMNS} from scim_identities \
             where user_id = $1 and org_id = $2"
        ))
        .bind(user_id)
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn find_by_user_name(
        &self,
        org_id: Uuid,
        user_name: &str,
    ) -> Result<Option<ScimIdentity>> {
        sqlx::query_as(&format!(
            "select {SCIM_IDENTITY_COLUMNS} from scim_identities \
             where org_id = $1 and user_name = $2"
        ))
        .bind(org_id)
        .bind(user_name)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Every provisioned identity in the org, oldest first so paging is stable.
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimIdentity>> {
        sqlx::query_as(&format!(
            "select {SCIM_IDENTITY_COLUMNS} from scim_identities where org_id = $1 \
             order by created_at, user_id"
        ))
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
        sqlx::query_as(&format!(
            "insert into scim_groups (org_id, external_id, display_name) \
             values ($1, $2, $3) returning {SCIM_GROUP_COLUMNS}"
        ))
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
        sqlx::query_as(&format!(
            "select {SCIM_GROUP_COLUMNS} from scim_groups where id = $1 and org_id = $2"
        ))
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
        sqlx::query_as(&format!(
            "select {SCIM_GROUP_COLUMNS} from scim_groups \
             where org_id = $1 and display_name = $2"
        ))
        .bind(org_id)
        .bind(display_name)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Every group in the org, oldest first so paging is stable.
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<ScimGroup>> {
        sqlx::query_as(&format!(
            "select {SCIM_GROUP_COLUMNS} from scim_groups where org_id = $1 \
             order by created_at, id"
        ))
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
        sqlx::query_as(&format!(
            "update scim_groups set external_id = $3, display_name = $4, updated_at = now() \
             where id = $1 and org_id = $2 returning {SCIM_GROUP_COLUMNS}"
        ))
        .bind(id)
        .bind(org_id)
        .bind(external_id)
        .bind(display_name)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("scim group {id}")))
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

/// Group→role mappings, the SCIM counterpart of [`SsoRepo::list_mappings`].
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
        sqlx::query_as(&format!(
            "insert into scim_group_mappings (org_id, group_name, team_id, project_id, role) \
             values ($1, $2, $3, $4, $5) returning {SCIM_GROUP_MAPPING_COLUMNS}"
        ))
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
        sqlx::query_as(&format!(
            "select {SCIM_GROUP_MAPPING_COLUMNS} from scim_group_mappings \
             where org_id = $1 order by group_name, created_at"
        ))
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
        sqlx::query_as(&format!(
            "select {SCIM_GROUP_MAPPING_COLUMNS} from scim_group_mappings \
             where org_id = $1 and group_name = $2 order by created_at"
        ))
        .bind(org_id)
        .bind(group_name)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<ScimGroupMapping> {
        sqlx::query_as(&format!(
            "select {SCIM_GROUP_MAPPING_COLUMNS} from scim_group_mappings where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("scim group mapping {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from scim_group_mappings where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("scim group mapping {id}")));
        }
        Ok(())
    }
}

/// MCP servers an org has registered.
pub struct McpServerRepo<'a>(pub &'a PgPool);

/// Columns of `mcp_servers` that may leave the store. The sealed client secret
/// is absent by construction: `has_client_secret` is projected instead, so no
/// query in this file can hand the ciphertext to a serializer by accident.
const MCP_SERVER_COLUMNS: &str = "id, org_id, name, slug, url, transport, description, enabled, \
     tools, source, required_scopes, created_at, \
     authorize_url, token_url, client_id, default_scopes, \
     (client_secret_ciphertext is not null) as has_client_secret";

impl McpServerRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<McpServer>> {
        sqlx::query_as(&format!(
            "select {MCP_SERVER_COLUMNS} from mcp_servers where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<McpServer> {
        sqlx::query_as(&format!(
            "select {MCP_SERVER_COLUMNS} from mcp_servers where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp server {id}")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        url: &str,
        transport: &str,
        description: &str,
        enabled: bool,
        tools: &[String],
        source: &str,
        required_scopes: &[String],
    ) -> Result<McpServer> {
        sqlx::query_as(&format!(
            "insert into mcp_servers \
             (org_id, name, slug, url, transport, description, enabled, tools, source, required_scopes) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             returning {MCP_SERVER_COLUMNS}"
        ))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(url)
        .bind(transport)
        .bind(description)
        .bind(enabled)
        .bind(tools)
        .bind(source)
        .bind(required_scopes)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        url: &str,
        transport: &str,
        description: &str,
        enabled: bool,
        tools: &[String],
        required_scopes: &[String],
    ) -> Result<McpServer> {
        sqlx::query_as(&format!(
            "update mcp_servers set name = $2, url = $3, transport = $4, description = $5, \
             enabled = $6, tools = $7, required_scopes = $8 \
             where id = $1 returning {MCP_SERVER_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(url)
        .bind(transport)
        .bind(description)
        .bind(enabled)
        .bind(tools)
        .bind(required_scopes)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp server {id}")))
    }

    /// Register (or replace) the OAuth client rolter presents to this server's
    /// authorization server. `client_secret` is sealed before it is written;
    /// `None` leaves the stored secret alone, `Some("")` clears it, which is
    /// how a confidential client is downgraded to a public one.
    #[allow(clippy::too_many_arguments)]
    pub async fn set_oauth_client(
        &self,
        kek: &super::crypto::Kek,
        id: Uuid,
        authorize_url: &str,
        token_url: &str,
        client_id: &str,
        client_secret: Option<&str>,
        default_scopes: &[String],
    ) -> Result<McpServer> {
        // three shapes: leave the secret, clear it, or replace it
        let sealed = match client_secret {
            None => None,
            Some("") => Some((None, None)),
            Some(secret) => {
                let (c, n) = kek.encrypt(secret)?;
                Some((Some(c), Some(n)))
            }
        };
        let (touch_secret, ciphertext, nonce) = match sealed {
            None => (false, None, None),
            Some((c, n)) => (true, c, n),
        };
        sqlx::query_as(&format!(
            "update mcp_servers set authorize_url = $2, token_url = $3, client_id = $4, \
                    default_scopes = $5, \
                    client_secret_ciphertext = case when $6 then $7 else client_secret_ciphertext end, \
                    client_secret_nonce = case when $6 then $8 else client_secret_nonce end \
             where id = $1 returning {MCP_SERVER_COLUMNS}"
        ))
        .bind(id)
        .bind(authorize_url)
        .bind(token_url)
        .bind(client_id)
        .bind(default_scopes)
        .bind(touch_secret)
        .bind(ciphertext)
        .bind(nonce)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp server {id}")))
    }

    /// Open the sealed client secret for `id`, or `None` when the client is
    /// public. The only way ciphertext leaves this table.
    pub async fn client_secret(
        &self,
        kek: &super::crypto::Kek,
        id: Uuid,
    ) -> Result<Option<String>> {
        let row: Option<SealedSecretRow> = sqlx::query_as(
            "select client_secret_ciphertext, client_secret_nonce from mcp_servers where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((Some(ciphertext), Some(nonce))) = row else {
            return Ok(None);
        };
        Ok(Some(kek.decrypt(&ciphertext, &nonce)?))
    }

    /// Delete a server. Its grants and sessions cascade, which is the intended
    /// blast radius: removing the server withdraws access to it entirely.
    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let res = sqlx::query("delete from mcp_servers where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("mcp server {id}")));
        }
        Ok(())
    }
}

/// Organization-owned tool-group policy manifests.
pub struct McpToolGroupRepo<'a>(pub &'a PgPool);

const MCP_TOOL_GROUP_COLUMNS: &str =
    "id, org_id, name, slug, description, enabled, tools, created_at, updated_at";

impl McpToolGroupRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<McpToolGroup>> {
        sqlx::query_as(&format!(
            "select {MCP_TOOL_GROUP_COLUMNS} from mcp_tool_groups \
             where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<McpToolGroup> {
        sqlx::query_as(&format!(
            "select {MCP_TOOL_GROUP_COLUMNS} from mcp_tool_groups where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp tool group {id}")))
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        description: &str,
        enabled: bool,
        tools: &serde_json::Value,
    ) -> Result<McpToolGroup> {
        sqlx::query_as(&format!(
            "insert into mcp_tool_groups (org_id, name, slug, description, enabled, tools) \
             values ($1, $2, $3, $4, $5, $6) returning {MCP_TOOL_GROUP_COLUMNS}"
        ))
        .bind(org_id)
        .bind(name)
        .bind(slug)
        .bind(description)
        .bind(enabled)
        .bind(tools)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        id: Uuid,
        name: &str,
        description: &str,
        enabled: bool,
        tools: &serde_json::Value,
    ) -> Result<McpToolGroup> {
        sqlx::query_as(&format!(
            "update mcp_tool_groups set name = $2, description = $3, enabled = $4, \
             tools = $5, updated_at = now() where id = $1 returning {MCP_TOOL_GROUP_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(enabled)
        .bind(tools)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp tool group {id}")))
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("delete from mcp_tool_groups where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("mcp tool group {id}")));
        }
        Ok(())
    }
}

/// Per-organization MCP gateway defaults.
pub struct McpGatewaySettingsRepo<'a>(pub &'a PgPool);

impl McpGatewaySettingsRepo<'_> {
    pub async fn get(&self, org_id: Uuid) -> Result<McpGatewaySettings> {
        sqlx::query_as(
            "select org_id, default_transport, connect_timeout_ms, request_timeout_ms, \
             max_retries, default_failure_mode, allow_unlisted_tools, updated_at \
             from mcp_gateway_settings where org_id = $1",
        )
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
        .map(|stored| {
            stored.unwrap_or_else(|| McpGatewaySettings {
                org_id,
                default_transport: "streamable_http".to_string(),
                connect_timeout_ms: 5_000,
                request_timeout_ms: 30_000,
                max_retries: 1,
                default_failure_mode: "fail_closed".to_string(),
                allow_unlisted_tools: false,
                updated_at: Utc::now(),
            })
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        org_id: Uuid,
        default_transport: &str,
        connect_timeout_ms: i32,
        request_timeout_ms: i32,
        max_retries: i32,
        default_failure_mode: &str,
        allow_unlisted_tools: bool,
    ) -> Result<McpGatewaySettings> {
        sqlx::query_as(
            "insert into mcp_gateway_settings \
             (org_id, default_transport, connect_timeout_ms, request_timeout_ms, max_retries, \
              default_failure_mode, allow_unlisted_tools, updated_at) \
             values ($1, $2, $3, $4, $5, $6, $7, now()) \
             on conflict (org_id) do update set default_transport = excluded.default_transport, \
             connect_timeout_ms = excluded.connect_timeout_ms, request_timeout_ms = excluded.request_timeout_ms, \
             max_retries = excluded.max_retries, default_failure_mode = excluded.default_failure_mode, \
             allow_unlisted_tools = excluded.allow_unlisted_tools, updated_at = now() \
             returning org_id, default_transport, connect_timeout_ms, request_timeout_ms, \
             max_retries, default_failure_mode, allow_unlisted_tools, updated_at",
        )
        .bind(org_id)
        .bind(default_transport)
        .bind(connect_timeout_ms)
        .bind(request_timeout_ms)
        .bind(max_retries)
        .bind(default_failure_mode)
        .bind(allow_unlisted_tools)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

/// Sealed client-secret columns of an `mcp_servers` row.
type SealedSecretRow = (Option<Vec<u8>>, Option<Vec<u8>>);

/// One `mcp_oauth_login_states` row as stored: `(state, server, user, verifier
/// ciphertext, verifier nonce, scopes, redirect uri, created at)`.
type SealedLoginRow = (
    String,
    Uuid,
    Uuid,
    Vec<u8>,
    Vec<u8>,
    Vec<String>,
    String,
    DateTime<Utc>,
);

/// Refresh material of one session: `(grant, server, refresh ciphertext,
/// refresh nonce, scopes)`.
type SealedRefreshRow = (Uuid, Uuid, Option<Vec<u8>>, Option<Vec<u8>>, Vec<String>);

/// Who a session belongs to: `(session, grant, user, server, grant scopes,
/// session scopes)`.
type SessionContextRow = (Uuid, Uuid, Uuid, Uuid, Vec<String>, Vec<String>);

/// Sealed columns of one session row: `(access ciphertext, access nonce,
/// refresh ciphertext, refresh nonce, scopes)`.
type SealedSessionRow = (
    Vec<u8>,
    Vec<u8>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Vec<String>,
);

/// OAuth consent grants and token sessions for MCP servers.
///
/// Token material is sealed with the deployment KEK ([`super::crypto::Kek`])
/// before it is written and is only ever returned by [`Self::open_session`],
/// which the future MCP proxy calls on the request path. Every other method
/// returns metadata only, so an API handler cannot leak a token by reaching
/// for the wrong function.
pub struct McpOAuthRepo<'a>(pub &'a PgPool);

const GRANT_COLUMNS: &str = "id, server_id, user_id, scopes, granted_at, revoked_at, revoked_by";
const SESSION_COLUMNS: &str = "id, grant_id, scopes, expires_at, refresh_expires_at, revoked_at, \
     created_at, last_used_at, (refresh_ciphertext is not null) as has_refresh_token";

impl McpOAuthRepo<'_> {
    /// Record (or refresh) a user's consent for a server. Re-consenting
    /// updates the scope set on the live grant rather than accumulating rows.
    pub async fn upsert_grant(
        &self,
        server_id: Uuid,
        user_id: Uuid,
        scopes: &[String],
    ) -> Result<McpOAuthGrant> {
        sqlx::query_as(&format!(
            "insert into mcp_oauth_grants (server_id, user_id, scopes) values ($1, $2, $3) \
             on conflict (server_id, user_id) where revoked_at is null \
             do update set scopes = excluded.scopes, granted_at = now() \
             returning {GRANT_COLUMNS}"
        ))
        .bind(server_id)
        .bind(user_id)
        .bind(scopes)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_grant(&self, id: Uuid) -> Result<McpOAuthGrant> {
        sqlx::query_as(&format!(
            "select {GRANT_COLUMNS} from mcp_oauth_grants where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp oauth grant {id}")))
    }

    /// Grants across an org, newest first. `user_id` narrows the listing to one
    /// owner, which is how a non-admin caller sees only their own.
    pub async fn list_grants(
        &self,
        org_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<Vec<McpOAuthGrant>> {
        sqlx::query_as(
            "select g.id, g.server_id, g.user_id, g.scopes, g.granted_at, g.revoked_at, \
                    g.revoked_by \
             from mcp_oauth_grants g join mcp_servers s on s.id = g.server_id \
             where s.org_id = $1 and ($2::uuid is null or g.user_id = $2) \
             order by g.granted_at desc",
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Revoke a grant and every session under it in one transaction, so a
    /// revoked consent can never leave a live token behind.
    pub async fn revoke_grant(&self, id: Uuid, revoked_by: Option<Uuid>) -> Result<McpOAuthGrant> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let grant: Option<McpOAuthGrant> = sqlx::query_as(&format!(
            "update mcp_oauth_grants set revoked_at = coalesce(revoked_at, now()), \
                    revoked_by = coalesce(revoked_by, $2) \
             where id = $1 returning {GRANT_COLUMNS}"
        ))
        .bind(id)
        .bind(revoked_by)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let grant = grant.ok_or_else(|| Error::NotFound(format!("mcp oauth grant {id}")))?;
        sqlx::query(
            "update mcp_oauth_sessions set revoked_at = coalesce(revoked_at, now()) \
             where grant_id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(grant)
    }

    /// Seal and store a token session for a live grant.
    #[allow(clippy::too_many_arguments)]
    pub async fn store_session(
        &self,
        kek: &super::crypto::Kek,
        grant_id: Uuid,
        access_token: &str,
        refresh_token: Option<&str>,
        scopes: &[String],
        expires_at: DateTime<Utc>,
        refresh_expires_at: Option<DateTime<Utc>>,
    ) -> Result<McpOAuthSession> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let grant_scopes: Option<Vec<String>> = sqlx::query_scalar(
            "select scopes from mcp_oauth_grants \
             where id = $1 and revoked_at is null for share",
        )
        .bind(grant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?;
        let grant_scopes = grant_scopes
            .ok_or_else(|| Error::Config("MCP OAuth grant is revoked or missing".to_string()))?;
        if scopes.iter().any(|scope| !grant_scopes.contains(scope)) {
            return Err(Error::Config(
                "MCP OAuth session scopes exceed the consent grant".to_string(),
            ));
        }
        let (access_ciphertext, access_nonce) = kek.encrypt(access_token)?;
        let refresh = refresh_token.map(|t| kek.encrypt(t)).transpose()?;
        let (refresh_ciphertext, refresh_nonce) = match refresh {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        let session = sqlx::query_as(&format!(
            "insert into mcp_oauth_sessions (grant_id, access_ciphertext, access_nonce, \
                    refresh_ciphertext, refresh_nonce, scopes, expires_at, refresh_expires_at) \
             values ($1, $2, $3, $4, $5, $6, $7, $8) returning {SESSION_COLUMNS}"
        ))
        .bind(grant_id)
        .bind(access_ciphertext)
        .bind(access_nonce)
        .bind(refresh_ciphertext)
        .bind(refresh_nonce)
        .bind(scopes)
        .bind(expires_at)
        .bind(refresh_expires_at)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(session)
    }

    /// Sessions across an org, newest first; `user_id` narrows to one owner.
    pub async fn list_sessions(
        &self,
        org_id: Uuid,
        user_id: Option<Uuid>,
    ) -> Result<Vec<McpOAuthSession>> {
        sqlx::query_as(
            "select s.id, s.grant_id, s.scopes, s.expires_at, s.refresh_expires_at, \
                    s.revoked_at, s.created_at, s.last_used_at, \
                    (s.refresh_ciphertext is not null) as has_refresh_token \
             from mcp_oauth_sessions s \
             join mcp_oauth_grants g on g.id = s.grant_id \
             join mcp_servers srv on srv.id = g.server_id \
             where srv.org_id = $1 and ($2::uuid is null or g.user_id = $2) \
             order by s.created_at desc",
        )
        .bind(org_id)
        .bind(user_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_session(&self, id: Uuid) -> Result<McpOAuthSession> {
        sqlx::query_as(&format!(
            "select {SESSION_COLUMNS} from mcp_oauth_sessions where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp oauth session {id}")))
    }

    pub async fn revoke_session(&self, id: Uuid) -> Result<McpOAuthSession> {
        sqlx::query_as(&format!(
            "update mcp_oauth_sessions set revoked_at = coalesce(revoked_at, now()) \
             where id = $1 returning {SESSION_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("mcp oauth session {id}")))
    }

    /// Open the sealed tokens for a session that is live *and* whose grant is
    /// live. Returns `None` for a revoked or expired session, or one whose
    /// consent was withdrawn — the caller cannot accidentally use a token the
    /// user has taken back.
    pub async fn open_session(
        &self,
        kek: &super::crypto::Kek,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<McpSessionTokens>> {
        let row: Option<SealedSessionRow> = sqlx::query_as(
            "select s.access_ciphertext, s.access_nonce, s.refresh_ciphertext, \
                        s.refresh_nonce, s.scopes \
                 from mcp_oauth_sessions s join mcp_oauth_grants g on g.id = s.grant_id \
                 where s.id = $1 and s.revoked_at is null and g.revoked_at is null \
                   and s.expires_at > $2",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((access_c, access_n, refresh_c, refresh_n, scopes)) = row else {
            return Ok(None);
        };
        let access_token = kek.decrypt(&access_c, &access_n)?;
        let refresh_token = match (refresh_c, refresh_n) {
            (Some(c), Some(n)) => Some(kek.decrypt(&c, &n)?),
            _ => None,
        };
        Ok(Some(McpSessionTokens {
            access_token,
            refresh_token,
            scopes,
        }))
    }

    /// Stamp a session as used. Best-effort bookkeeping for the sessions
    /// screen; a failure must never fail the MCP call it belongs to.
    pub async fn touch_session(&self, id: Uuid) -> Result<()> {
        sqlx::query("update mcp_oauth_sessions set last_used_at = now() where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        Ok(())
    }

    // -- authorization-code exchange (#707) ---------------------------------

    /// Record an in-flight consent. The PKCE verifier is sealed on the way in,
    /// so a database read alone cannot redeem a stolen authorization code.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_login(
        &self,
        kek: &super::crypto::Kek,
        state: &str,
        server_id: Uuid,
        user_id: Uuid,
        code_verifier: &str,
        scopes: &[String],
        redirect_uri: &str,
    ) -> Result<()> {
        let (ciphertext, nonce) = kek.encrypt(code_verifier)?;
        sqlx::query(
            "insert into mcp_oauth_login_states \
                 (state, server_id, user_id, verifier_ciphertext, verifier_nonce, scopes, \
                  redirect_uri) \
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(state)
        .bind(server_id)
        .bind(user_id)
        .bind(ciphertext)
        .bind(nonce)
        .bind(scopes)
        .bind(redirect_uri)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(())
    }

    /// Consume an in-flight consent: the row is deleted as it is read, so a
    /// replayed `code`+`state` pair finds nothing. Rows older than `max_age`
    /// are treated as absent (and swept), which bounds how long a leaked
    /// `state` is worth anything.
    pub async fn consume_login(
        &self,
        kek: &super::crypto::Kek,
        state: &str,
        now: DateTime<Utc>,
        max_age: Duration,
    ) -> Result<Option<McpLoginState>> {
        let cutoff = now - max_age;
        // sweep first: expired rows are garbage whether or not this call
        // matches one, and this is the only traffic the table sees
        sqlx::query("delete from mcp_oauth_login_states where created_at < $1")
            .bind(cutoff)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        let row: Option<SealedLoginRow> = sqlx::query_as(
            "delete from mcp_oauth_login_states where state = $1 and created_at >= $2 \
                 returning state, server_id, user_id, verifier_ciphertext, verifier_nonce, \
                           scopes, redirect_uri, created_at",
        )
        .bind(state)
        .bind(cutoff)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((state, server_id, user_id, ciphertext, nonce, scopes, redirect_uri, created_at)) =
            row
        else {
            return Ok(None);
        };
        Ok(Some(McpLoginState {
            state,
            server_id,
            user_id,
            code_verifier: kek.decrypt(&ciphertext, &nonce)?,
            scopes,
            redirect_uri,
            created_at,
        }))
    }

    /// Open the sealed *refresh* token of a session whose access token may
    /// already have expired — the one case [`Self::open_session`] deliberately
    /// refuses. Consent is still checked: a revoked session or a withdrawn
    /// grant yields `None`, as does an expired refresh token, so a renewal can
    /// never outlive the consent it hangs off.
    pub async fn open_refresh(
        &self,
        kek: &super::crypto::Kek,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Option<McpRefreshMaterial>> {
        let row: Option<SealedRefreshRow> = sqlx::query_as(
            "select g.id, g.server_id, s.refresh_ciphertext, s.refresh_nonce, s.scopes \
                 from mcp_oauth_sessions s join mcp_oauth_grants g on g.id = s.grant_id \
                 where s.id = $1 and s.revoked_at is null and g.revoked_at is null \
                   and (s.refresh_expires_at is null or s.refresh_expires_at > $2)",
        )
        .bind(id)
        .bind(now)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        let Some((grant_id, server_id, Some(ciphertext), Some(nonce), scopes)) = row else {
            return Ok(None);
        };
        Ok(Some(McpRefreshMaterial {
            grant_id,
            server_id,
            refresh_token: kek.decrypt(&ciphertext, &nonce)?,
            scopes,
        }))
    }

    /// Replace the token material on a live session in place. Rotation is the
    /// point: an authorization server that hands back a new refresh token must
    /// not leave the old one readable, and one that omits it must not leave the
    /// session unrenewable, so `refresh_token` is written unconditionally.
    ///
    /// The row id is stable across a renewal on purpose — a session is the
    /// user's mental unit of "this app is connected", and churning its id on
    /// every silent refresh would make the sessions screen unreadable.
    #[allow(clippy::too_many_arguments)]
    pub async fn rotate_session(
        &self,
        kek: &super::crypto::Kek,
        id: Uuid,
        access_token: &str,
        refresh_token: Option<&str>,
        scopes: &[String],
        expires_at: DateTime<Utc>,
        refresh_expires_at: Option<DateTime<Utc>>,
    ) -> Result<McpOAuthSession> {
        let (access_ciphertext, access_nonce) = kek.encrypt(access_token)?;
        let refresh = refresh_token.map(|t| kek.encrypt(t)).transpose()?;
        let (refresh_ciphertext, refresh_nonce) = match refresh {
            Some((c, n)) => (Some(c), Some(n)),
            None => (None, None),
        };
        sqlx::query_as(&format!(
            "update mcp_oauth_sessions set access_ciphertext = $2, access_nonce = $3, \
                    refresh_ciphertext = $4, refresh_nonce = $5, scopes = $6, \
                    expires_at = $7, refresh_expires_at = $8 \
             where id = $1 and revoked_at is null returning {SESSION_COLUMNS}"
        ))
        .bind(id)
        .bind(access_ciphertext)
        .bind(access_nonce)
        .bind(refresh_ciphertext)
        .bind(refresh_nonce)
        .bind(scopes)
        .bind(expires_at)
        .bind(refresh_expires_at)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("live mcp oauth session {id}")))
    }

    /// Live sessions whose access token expires before `before` and that hold a
    /// refresh token — the work list for the background renewer. Sessions
    /// without one are skipped: there is nothing to renew them with, and they
    /// simply lapse.
    pub async fn sessions_due_for_refresh(
        &self,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<Uuid>> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "select s.id from mcp_oauth_sessions s \
                 join mcp_oauth_grants g on g.id = s.grant_id \
             where s.revoked_at is null and g.revoked_at is null \
               and s.refresh_ciphertext is not null and s.expires_at < $1 \
               and (s.refresh_expires_at is null or s.refresh_expires_at > now()) \
             order by s.expires_at limit $2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(self.0)
        .await
        .map_err(store_err)?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// The grant a session hangs off together with its server and owner, in one
    /// round trip. Used by the authorization guard on the MCP call path, which
    /// runs per request and cannot afford three.
    pub async fn session_context(&self, id: Uuid) -> Result<Option<McpSessionContext>> {
        let row: Option<SessionContextRow> = sqlx::query_as(
            "select s.id, g.id, g.user_id, srv.id, g.scopes, s.scopes \
             from mcp_oauth_sessions s \
             join mcp_oauth_grants g on g.id = s.grant_id \
             join mcp_servers srv on srv.id = g.server_id \
             where s.id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(row.map(
            |(session_id, grant_id, user_id, server_id, grant_scopes, session_scopes)| {
                McpSessionContext {
                    session_id,
                    grant_id,
                    user_id,
                    server_id,
                    grant_scopes,
                    session_scopes,
                }
            },
        ))
    }
}

/// The refresh token of a session plus the consent it belongs to. Not
/// `Serialize`: the token may not reach an API response or a log line.
#[derive(Debug, Clone)]
pub struct McpRefreshMaterial {
    pub grant_id: Uuid,
    pub server_id: Uuid,
    pub refresh_token: String,
    pub scopes: Vec<String>,
}

/// Who a session belongs to and what it is allowed to ask for. Carries no
/// token material, so it is safe to hold across an authorization decision.
#[derive(Debug, Clone)]
pub struct McpSessionContext {
    pub session_id: Uuid,
    pub grant_id: Uuid,
    pub user_id: Uuid,
    pub server_id: Uuid,
    /// what the user consented to — the ceiling for everything below
    pub grant_scopes: Vec<String>,
    /// what this particular session actually holds
    pub session_scopes: Vec<String>,
}

/// Opened token material. Deliberately not `Serialize`: nothing in this struct
/// may reach an API response or a log line.
#[derive(Debug, Clone)]
pub struct McpSessionTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
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

    /// Every enabled route on `strategy`, across projects. Used by the global
    /// routing policies to report which routes a change actually affects.
    pub async fn list_by_strategy(&self, strategy: &str) -> Result<Vec<Route>> {
        sqlx::query_as(
            "select id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at
             from routes where strategy = $1 and enabled order by model",
        )
        .bind(strategy)
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

    /// Change the balancing strategy of an existing route.
    ///
    /// The bootstrap import needs this to make a re-imported `rolter.toml` the
    /// desired state rather than a first-write-wins snapshot (#927); the value
    /// is constrained by the database to the strategies a migration allows.
    pub async fn set_strategy(&self, id: Uuid, strategy: &str) -> Result<Route> {
        sqlx::query_as(
            "update routes set strategy = $2 where id = $1
             returning id, project_id, model, strategy, enabled, params, param_policy, advanced, created_at",
        )
        .bind(id)
        .bind(strategy)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route {id}")))
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

    /// Change the weight of an existing target.
    ///
    /// The bootstrap import needs this so a re-imported `rolter.toml` whose
    /// weights were edited actually applies them (#927), instead of matching
    /// the target as "already there" and moving on.
    pub async fn set_weight(&self, id: Uuid, weight: i32) -> Result<RouteTarget> {
        sqlx::query_as(
            "update route_targets set weight = $2 where id = $1
             returning id, route_id, provider_id, upstream_model, weight, created_at",
        )
        .bind(id)
        .bind(weight)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("route target {id}")))
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
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at
             from virtual_keys where project_id = $1 order by created_at",
        )
        .bind(project_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn find_by_hash(&self, key_hash: &str) -> Result<Option<VirtualKey>> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at
             from virtual_keys where key_hash = $1",
        )
        .bind(key_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<VirtualKey> {
        sqlx::query_as(
            "select id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at
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
    ) -> Result<VirtualKey> {
        sqlx::query_as(
            "insert into virtual_keys (project_id, key_hash, key_prefix, name, models, providers, cache_enabled, created_by, expires_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at",
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
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at",
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
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at",
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
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at",
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
             returning id, project_id, key_hash, key_prefix, name, models, providers, disabled, expires_at, cache_enabled, created_by, business_unit_id, customer_id, created_at",
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

/// Budgets, attachable at any scope (org/team/project/virtual_key).
pub struct BudgetRepo<'a>(pub &'a PgPool);

impl BudgetRepo<'_> {
    pub async fn list_for_scope(&self, scope_type: &str, scope_id: Uuid) -> Result<Vec<Budget>> {
        sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, unpriced_policy, created_at
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
            "select id, scope_type, scope_id, limit_usd::text as limit_usd, period, unpriced_policy, created_at
             from budgets where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("budget {id}")))
    }

    /// `unpriced_policy` is `None` for a budget that inherits the
    /// deployment-wide setting, which is every budget created before #996.
    pub async fn create(
        &self,
        scope_type: &str,
        scope_id: Uuid,
        limit_usd: &str,
        period: &str,
        unpriced_policy: Option<&str>,
    ) -> Result<Budget> {
        sqlx::query_as(
            "insert into budgets (scope_type, scope_id, limit_usd, period, unpriced_policy)
             values ($1, $2, $3::numeric, $4, $5)
             returning id, scope_type, scope_id, limit_usd::text as limit_usd, period,              unpriced_policy, created_at",
        )
        .bind(scope_type)
        .bind(scope_id)
        .bind(limit_usd)
        .bind(period)
        .bind(unpriced_policy)
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
            "select id, user_id, org_id, team_id, project_id, role, source, created_at
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
            "select m.id, m.user_id, m.org_id, m.team_id, m.project_id, m.role, m.source, m.created_at
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
            "select id, user_id, org_id, team_id, project_id, role, source, created_at
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
        self.create_with_source(user_id, org_id, team_id, project_id, role, "manual")
            .await
    }

    /// grant a role and record where the grant came from. `source` is
    /// `manual` for anything an operator did (admin API, invitation, seed) and
    /// `sso` for a grant an IdP group mapping produced; only `sso` rows are
    /// ever reconciled away by a later login.
    pub async fn create_with_source(
        &self,
        user_id: Uuid,
        org_id: Option<Uuid>,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        role: &str,
        source: &str,
    ) -> Result<Membership> {
        sqlx::query_as(
            "insert into memberships (user_id, org_id, team_id, project_id, role, source)
             values ($1, $2, $3, $4, $5, $6)
             returning id, user_id, org_id, team_id, project_id, role, source, created_at",
        )
        .bind(user_id)
        .bind(org_id)
        .bind(team_id)
        .bind(project_id)
        .bind(role)
        .bind(source)
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

/// invitations: one-time links that let an invitee create their own account.
pub struct InvitationRepo<'a>(pub &'a PgPool);

const INVITATION_COLUMNS: &str = "id, org_id, email, role, team_id, project_id, token_hash, \
     invited_by, expires_at, accepted_at, revoked_at, created_at";

impl InvitationRepo<'_> {
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        org_id: Uuid,
        email: &str,
        role: &str,
        team_id: Option<Uuid>,
        project_id: Option<Uuid>,
        token_hash: &str,
        invited_by: Option<Uuid>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Invitation> {
        sqlx::query_as(&format!(
            "insert into invitations (org_id, email, role, team_id, project_id, token_hash, \
                    invited_by, expires_at) \
             values ($1, $2, $3, $4, $5, $6, $7, $8) \
             returning {INVITATION_COLUMNS}"
        ))
        .bind(org_id)
        .bind(email)
        .bind(role)
        .bind(team_id)
        .bind(project_id)
        .bind(token_hash)
        .bind(invited_by)
        .bind(expires_at)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self, org_id: Uuid) -> Result<Vec<Invitation>> {
        sqlx::query_as(&format!(
            "select {INVITATION_COLUMNS} from invitations \
             where org_id = $1 order by created_at desc"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Look an invitation up by token digest, but only while it is live:
    /// unaccepted, unrevoked and unexpired. Spent invitations are
    /// indistinguishable from wrong ones to the caller, which is the point.
    pub async fn find_live_by_hash(&self, token_hash: &str) -> Result<Option<Invitation>> {
        sqlx::query_as(&format!(
            "select {INVITATION_COLUMNS} from invitations \
             where token_hash = $1 and accepted_at is null and revoked_at is null \
               and expires_at > now()"
        ))
        .bind(token_hash)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)
    }

    /// Mark an invitation accepted, but only if it is still live. Returns
    /// `false` when another request got there first, so acceptance is
    /// single-use even under a race.
    pub async fn mark_accepted(&self, id: Uuid) -> Result<bool> {
        let res = sqlx::query(
            "update invitations set accepted_at = now() \
             where id = $1 and accepted_at is null and revoked_at is null and expires_at > now()",
        )
        .bind(id)
        .execute(self.0)
        .await
        .map_err(store_err)?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn revoke(&self, id: Uuid) -> Result<Invitation> {
        sqlx::query_as(&format!(
            "update invitations set revoked_at = now() \
             where id = $1 and accepted_at is null \
             returning {INVITATION_COLUMNS}"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("pending invitation {id}")))
    }

    pub async fn get(&self, id: Uuid) -> Result<Invitation> {
        sqlx::query_as(&format!(
            "select {INVITATION_COLUMNS} from invitations where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("invitation {id}")))
    }
}

/// per-org login policy: whether members may use a local password, and
/// whether SSO callbacks for the org's providers are honoured.
pub struct OrgAuthPolicyRepo<'a>(pub &'a PgPool);

impl OrgAuthPolicyRepo<'_> {
    /// The policy for `org_id`, or the permissive default when no row exists.
    /// Orgs created before single sign-on existed have no row, and must keep
    /// logging in exactly as they did.
    pub async fn get(&self, org_id: Uuid) -> Result<OrgAuthPolicy> {
        let found: Option<OrgAuthPolicy> = sqlx::query_as(
            "select org_id, allow_password_login, allow_sso, updated_at
             from org_auth_policies where org_id = $1",
        )
        .bind(org_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(found.unwrap_or(OrgAuthPolicy {
            org_id,
            allow_password_login: true,
            allow_sso: true,
            updated_at: chrono::Utc::now(),
        }))
    }

    pub async fn set(
        &self,
        org_id: Uuid,
        allow_password_login: bool,
        allow_sso: bool,
    ) -> Result<OrgAuthPolicy> {
        sqlx::query_as(
            "insert into org_auth_policies (org_id, allow_password_login, allow_sso)
             values ($1, $2, $3)
             on conflict (org_id) do update
                 set allow_password_login = excluded.allow_password_login,
                     allow_sso = excluded.allow_sso,
                     updated_at = now()
             returning org_id, allow_password_login, allow_sso, updated_at",
        )
        .bind(org_id)
        .bind(allow_password_login)
        .bind(allow_sso)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// True when at least one org this user belongs to forbids password login.
    /// Membership scopes are org/team/project, so the team's and project's
    /// owning org counts too.
    pub async fn password_login_blocked_for_user(&self, user_id: Uuid) -> Result<bool> {
        let blocked: Option<(bool,)> = sqlx::query_as(
            "select true from memberships m
             left join teams t on t.id = m.team_id
             left join projects p on p.id = m.project_id
             left join teams pt on pt.id = p.team_id
             join org_auth_policies ap
               on ap.org_id = coalesce(m.org_id, t.org_id, pt.org_id)
             where m.user_id = $1 and not ap.allow_password_login
             limit 1",
        )
        .bind(user_id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(blocked.is_some())
    }

    /// True when any org allows password login, or when no org has a policy
    /// row at all. Drives the login page: a deployment that turned password
    /// login off everywhere should not render a password form.
    pub async fn any_password_login_allowed(&self) -> Result<bool> {
        let row: Option<(bool,)> = sqlx::query_as(
            "select true from orgs o
             left join org_auth_policies ap on ap.org_id = o.id
             where ap.allow_password_login is not false
             limit 1",
        )
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?;
        Ok(row.is_some())
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
pub struct FeatureFlagsRepo<'a>(pub &'a PgPool);
pub struct LoggingSettingsRepo<'a>(pub &'a PgPool);
pub struct RuntimePolicyRepo<'a>(pub &'a PgPool);
pub struct CompatibilityPolicyRepo<'a>(pub &'a PgPool);
pub struct ClientSettingsRepo<'a>(pub &'a PgPool);
pub struct ModelDefaultsRepo<'a>(pub &'a PgPool);
pub struct AdaptiveRoutingPolicyRepo<'a>(pub &'a PgPool);
pub struct GuardrailRepo<'a>(pub &'a PgPool);

const GUARDRAIL_RULE_COLUMNS: &str = "id, name, enabled, source_type, builtin, pattern, stage, \
    action, replacement, include_system, position, created_at, updated_at";
const GUARDRAIL_PROVIDER_COLUMNS: &str = "id, name, enabled, url, stage, timeout_ms, max_retries, \
    failure_mode, max_body_bytes, auth_kind, auth_env, created_at, updated_at";

impl GuardrailRepo<'_> {
    pub async fn list_rules(&self) -> Result<Vec<GuardrailRule>> {
        sqlx::query_as(&format!(
            "select {GUARDRAIL_RULE_COLUMNS} from guardrail_rules order by position, name"
        ))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_rule(&self, id: Uuid) -> Result<GuardrailRule> {
        sqlx::query_as(&format!(
            "select {GUARDRAIL_RULE_COLUMNS} from guardrail_rules where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("guardrail rule {id}")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_rule(
        &self,
        name: &str,
        enabled: bool,
        source_type: &str,
        builtin: Option<&str>,
        pattern: Option<&str>,
        stage: &str,
        action: &str,
        replacement: Option<&str>,
        include_system: bool,
        position: i32,
    ) -> Result<GuardrailRule> {
        sqlx::query_as(&format!(
            "insert into guardrail_rules (name, enabled, source_type, builtin, pattern, stage, \
             action, replacement, include_system, position) values \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) returning {GUARDRAIL_RULE_COLUMNS}"
        ))
        .bind(name)
        .bind(enabled)
        .bind(source_type)
        .bind(builtin)
        .bind(pattern)
        .bind(stage)
        .bind(action)
        .bind(replacement)
        .bind(include_system)
        .bind(position)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_rule(
        &self,
        id: Uuid,
        name: &str,
        enabled: bool,
        source_type: &str,
        builtin: Option<&str>,
        pattern: Option<&str>,
        stage: &str,
        action: &str,
        replacement: Option<&str>,
        include_system: bool,
        position: i32,
    ) -> Result<GuardrailRule> {
        sqlx::query_as(&format!(
            "update guardrail_rules set name=$2, enabled=$3, source_type=$4, builtin=$5, \
             pattern=$6, stage=$7, action=$8, replacement=$9, include_system=$10, \
             position=$11, updated_at=now() where id=$1 returning {GUARDRAIL_RULE_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(enabled)
        .bind(source_type)
        .bind(builtin)
        .bind(pattern)
        .bind(stage)
        .bind(action)
        .bind(replacement)
        .bind(include_system)
        .bind(position)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("guardrail rule {id}")))
    }

    pub async fn delete_rule(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("delete from guardrail_rules where id=$1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("guardrail rule {id}")));
        }
        Ok(())
    }

    pub async fn list_providers(&self) -> Result<Vec<GuardrailProvider>> {
        sqlx::query_as(&format!(
            "select {GUARDRAIL_PROVIDER_COLUMNS} from guardrail_providers order by enabled desc, name"
        ))
        .fetch_all(self.0).await.map_err(store_err)
    }

    pub async fn get_provider(&self, id: Uuid) -> Result<GuardrailProvider> {
        sqlx::query_as(&format!(
            "select {GUARDRAIL_PROVIDER_COLUMNS} from guardrail_providers where id=$1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("guardrail provider {id}")))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_provider(
        &self,
        name: &str,
        enabled: bool,
        url: &str,
        stage: &str,
        timeout_ms: i32,
        max_retries: i32,
        failure_mode: &str,
        max_body_bytes: i32,
        auth_kind: &str,
        auth_env: Option<&str>,
    ) -> Result<GuardrailProvider> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if enabled {
            sqlx::query(
                "update guardrail_providers set enabled=false, updated_at=now() where enabled",
            )
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        }
        let row = sqlx::query_as(&format!(
            "insert into guardrail_providers (name, enabled, url, stage, timeout_ms, max_retries, \
             failure_mode, max_body_bytes, auth_kind, auth_env) values \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) returning {GUARDRAIL_PROVIDER_COLUMNS}"
        ))
        .bind(name)
        .bind(enabled)
        .bind(url)
        .bind(stage)
        .bind(timeout_ms)
        .bind(max_retries)
        .bind(failure_mode)
        .bind(max_body_bytes)
        .bind(auth_kind)
        .bind(auth_env)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(row)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_provider(
        &self,
        id: Uuid,
        name: &str,
        enabled: bool,
        url: &str,
        stage: &str,
        timeout_ms: i32,
        max_retries: i32,
        failure_mode: &str,
        max_body_bytes: i32,
        auth_kind: &str,
        auth_env: Option<&str>,
    ) -> Result<GuardrailProvider> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if enabled {
            sqlx::query("update guardrail_providers set enabled=false, updated_at=now() where enabled and id<>$1")
                .bind(id).execute(&mut *tx).await.map_err(store_err)?;
        }
        let row = sqlx::query_as(&format!(
            "update guardrail_providers set name=$2, enabled=$3, url=$4, stage=$5, timeout_ms=$6, \
             max_retries=$7, failure_mode=$8, max_body_bytes=$9, auth_kind=$10, auth_env=$11, \
             updated_at=now() where id=$1 returning {GUARDRAIL_PROVIDER_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(enabled)
        .bind(url)
        .bind(stage)
        .bind(timeout_ms)
        .bind(max_retries)
        .bind(failure_mode)
        .bind(max_body_bytes)
        .bind(auth_kind)
        .bind(auth_env)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("guardrail provider {id}")))?;
        tx.commit().await.map_err(store_err)?;
        Ok(row)
    }

    pub async fn delete_provider(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("delete from guardrail_providers where id=$1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("guardrail provider {id}")));
        }
        Ok(())
    }
}

/// Cluster inventory: one row per gateway/control node, upserted from the
/// snapshot poll each node already performs.
pub struct ClusterNodeRepo<'a>(pub &'a PgPool);

impl ClusterNodeRepo<'_> {
    /// Record a node sighting. `first_seen_at` is preserved across restarts of
    /// the same node id so an operator can tell a flapping node from a new one.
    pub async fn heartbeat(
        &self,
        id: &str,
        role: &str,
        build_version: &str,
        config_version: i64,
    ) -> Result<ClusterNode> {
        sqlx::query_as(
            "insert into cluster_nodes (id, role, build_version, config_version) \
             values ($1, $2, $3, $4) \
             on conflict (id) do update set \
                role = excluded.role, build_version = excluded.build_version, \
                config_version = excluded.config_version, last_seen_at = now() \
             returning id, role, build_version, config_version, desired_state, \
                state_changed_at, first_seen_at, last_seen_at",
        )
        .bind(id)
        .bind(role)
        .bind(build_version)
        .bind(config_version)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn list(&self) -> Result<Vec<ClusterNode>> {
        sqlx::query_as(
            "select id, role, build_version, config_version, desired_state, \
                    state_changed_at, first_seen_at, last_seen_at \
             from cluster_nodes order by role, id",
        )
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Set the operator-requested state for a node. Returns the updated row so
    /// the caller can audit what actually changed.
    pub async fn set_desired_state(&self, id: &str, desired_state: &str) -> Result<ClusterNode> {
        sqlx::query_as(
            "update cluster_nodes set desired_state = $2, state_changed_at = now() \
             where id = $1 \
             returning id, role, build_version, config_version, desired_state, \
                       state_changed_at, first_seen_at, last_seen_at",
        )
        .bind(id)
        .bind(desired_state)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("cluster node {id}")))
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
        let res = sqlx::query("delete from cluster_nodes where id = $1")
            .bind(id)
            .execute(self.0)
            .await
            .map_err(store_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("cluster node {id}")));
        }
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
        let models: Vec<String> = samples.iter().map(|s| s.model.to_string()).collect();
        let mut tx = self.0.begin().await.map_err(store_err)?;
        sqlx::query(
            "delete from adaptive_routing_telemetry \
             where node_id = $1 and model <> all($2)",
        )
        .bind(node_id)
        .bind(&models)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        for sample in samples {
            sqlx::query(
                "insert into adaptive_routing_telemetry \
                    (node_id, model, engaged, observed, blend_picks, exploration_picks, \
                     fallback_picks, policy, targets, reported_at) \
                 values ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
                 on conflict (node_id, model) do update set \
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
            "select node_id, model, engaged, observed, blend_picks, exploration_picks, \
                    fallback_picks, policy, targets, reported_at \
             from adaptive_routing_telemetry \
             where reported_at > now() - make_interval(secs => $1::double precision) \
             order by model, node_id",
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

impl SecuritySettingsRepo<'_> {
    pub async fn get(&self) -> Result<SecuritySettings> {
        sqlx::query_as(
            "select virtual_key_required, allowed_origins, allowed_headers, \
                    required_headers, auth_bypass_routes, dashboard_auth_enabled, dashboard_credential_ref, \
                    dashboard_credential_ciphertext is not null as dashboard_secret_configured, updated_at \
             from security_settings where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// `allow_direct_provider_keys` is pinned to `false` in the statement
    /// rather than taken as an argument: the gateway has no
    /// direct-provider-key passthrough, so the column never controlled
    /// anything and is no longer offered by the API (#1162).
    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        virtual_key_required: bool,
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
                virtual_key_required = $1, allowed_origins = $2, \
                allowed_headers = $3, required_headers = $4, auth_bypass_routes = $5, \
                dashboard_auth_enabled = $6, dashboard_credential_ref = $7, \
                dashboard_credential_ciphertext = coalesce($8, dashboard_credential_ciphertext), \
                dashboard_credential_nonce = coalesce($9, dashboard_credential_nonce), \
                allow_direct_provider_keys = false, updated_at = now() \
             where id = true \
             returning virtual_key_required, allowed_origins, allowed_headers, \
                       required_headers, auth_bypass_routes, dashboard_auth_enabled, dashboard_credential_ref, \
                       dashboard_credential_ciphertext is not null as dashboard_secret_configured, updated_at",
        )
        .bind(virtual_key_required)
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

impl FeatureFlagsRepo<'_> {
    pub async fn get(&self) -> Result<FeatureFlags> {
        sqlx::query_as(
            "select response_cache, cache_aware_routing, circuit_breaker, active_health_checks, \
                    complexity_routing, guardrails, updated_at \
             from feature_flags where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        response_cache: bool,
        cache_aware_routing: bool,
        circuit_breaker: bool,
        active_health_checks: bool,
        complexity_routing: bool,
        guardrails: bool,
    ) -> Result<FeatureFlags> {
        sqlx::query_as(
            "update feature_flags set \
                response_cache = $1, cache_aware_routing = $2, circuit_breaker = $3, \
                active_health_checks = $4, complexity_routing = $5, guardrails = $6, updated_at = now() \
             where id = true \
             returning response_cache, cache_aware_routing, circuit_breaker, active_health_checks, \
                       complexity_routing, guardrails, updated_at",
        )
        .bind(response_cache)
        .bind(cache_aware_routing)
        .bind(circuit_breaker)
        .bind(active_health_checks)
        .bind(complexity_routing)
        .bind(guardrails)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

impl LoggingSettingsRepo<'_> {
    pub async fn get(&self) -> Result<LoggingSettings> {
        sqlx::query_as(
            "select sample_rate, payload_capture_enabled, payload_capture_max_bytes, \
                    payload_capture_redact_fields, payload_capture_models, payload_capture_virtual_key_ids, \
                    retention_days, payload_retention_hours, updated_at \
             from logging_settings where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        sample_rate: f64,
        payload_capture_enabled: bool,
        payload_capture_max_bytes: i32,
        payload_capture_redact_fields: &[String],
        payload_capture_models: &[String],
        payload_capture_virtual_key_ids: &[String],
        retention_days: i32,
        payload_retention_hours: i32,
    ) -> Result<LoggingSettings> {
        sqlx::query_as(
            "update logging_settings set \
                sample_rate = $1, payload_capture_enabled = $2, payload_capture_max_bytes = $3, \
                payload_capture_redact_fields = $4, payload_capture_models = $5, \
                payload_capture_virtual_key_ids = $6, retention_days = $7, \
                payload_retention_hours = $8, updated_at = now() \
             where id = true \
             returning sample_rate, payload_capture_enabled, payload_capture_max_bytes, \
                       payload_capture_redact_fields, payload_capture_models, \
                       payload_capture_virtual_key_ids, retention_days, \
                       payload_retention_hours, updated_at",
        )
        .bind(sample_rate)
        .bind(payload_capture_enabled)
        .bind(payload_capture_max_bytes)
        .bind(payload_capture_redact_fields)
        .bind(payload_capture_models)
        .bind(payload_capture_virtual_key_ids)
        .bind(retention_days)
        .bind(payload_retention_hours)
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

// ---------------------------------------------------------------------------
// configurable rbac (#534)
// ---------------------------------------------------------------------------

/// One role a profile confers, and where: `(role, org, team, project)`. Exactly
/// one of the three scopes is `Some`, which the schema enforces.
pub type ProfileRoleAssignment = (Uuid, Option<Uuid>, Option<Uuid>, Option<Uuid>);

const CUSTOM_ROLE_COLUMNS: &str =
    "id, org_id, slug, name, description, base_role, created_at, updated_at";

/// Org-scoped custom roles and their explicit `(resource, action)` grants.
pub struct CustomRoleRepo<'a>(pub &'a PgPool);

impl CustomRoleRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<CustomRole>> {
        sqlx::query_as(&format!(
            "select {CUSTOM_ROLE_COLUMNS} from custom_roles where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<CustomRole> {
        sqlx::query_as(&format!(
            "select {CUSTOM_ROLE_COLUMNS} from custom_roles where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("custom role {id}")))
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        slug: &str,
        name: &str,
        description: Option<&str>,
        base_role: &str,
    ) -> Result<CustomRole> {
        sqlx::query_as(&format!(
            "insert into custom_roles (org_id, slug, name, description, base_role) \
             values ($1, $2, $3, $4, $5) returning {CUSTOM_ROLE_COLUMNS}"
        ))
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
        sqlx::query_as(&format!(
            "update custom_roles set name = $2, description = $3, base_role = $4, \
                    updated_at = now() \
             where id = $1 returning {CUSTOM_ROLE_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(base_role)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("custom role {id}")))
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
        sqlx::query_as(
            "select id, role_id, resource, action from custom_role_grants \
             where role_id = $1 order by resource, action",
        )
        .bind(role_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Every grant belonging to any of `role_ids`, so the matrix can render a
    /// whole org's custom roles without a query per role.
    pub async fn list_grants_for_roles(&self, role_ids: &[Uuid]) -> Result<Vec<CustomRoleGrant>> {
        sqlx::query_as(
            "select id, role_id, resource, action from custom_role_grants \
             where role_id = any($1) order by resource, action",
        )
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
        sqlx::query_as(&format!(
            "select {ACCESS_PROFILE_COLUMNS} from access_profiles where org_id = $1 order by name"
        ))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<AccessProfile> {
        sqlx::query_as(&format!(
            "select {ACCESS_PROFILE_COLUMNS} from access_profiles where id = $1"
        ))
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("access profile {id}")))
    }

    pub async fn create(
        &self,
        org_id: Uuid,
        slug: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<AccessProfile> {
        sqlx::query_as(&format!(
            "insert into access_profiles (org_id, slug, name, description) \
             values ($1, $2, $3, $4) returning {ACCESS_PROFILE_COLUMNS}"
        ))
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
        sqlx::query_as(&format!(
            "update access_profiles set name = $2, description = $3, updated_at = now() \
             where id = $1 returning {ACCESS_PROFILE_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(description)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("access profile {id}")))
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

    pub async fn get_assignment(&self, id: Uuid) -> Result<AccessProfileAssignment> {
        sqlx::query_as(
            "select id, profile_id, user_id, team_id, created_at \
             from access_profile_assignments where id = $1",
        )
        .bind(id)
        .fetch_optional(self.0)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("access profile assignment {id}")))
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
        sqlx::query_as(&format!(
            "select {ACCESS_PROFILE_POLICY_COLUMNS} from access_profile_policies \
             where profile_id = $1"
        ))
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
        sqlx::query_as(&format!(
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
        ))
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
                Some(Utc::now() + Duration::days(30)),
            )
            .await
            .unwrap();
        // defaults to inherit-the-route (NULL) on create
        assert_eq!(vk.cache_enabled, None);
        // the ttl the caller chose reaches the column the gateway reads, so a
        // minted key is not immortal by construction (#945)
        assert!(vk.expires_at.is_some());
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
            .create("project", project.id, "100.5000", "30d", None)
            .await
            .unwrap();
        assert_eq!(budget.limit_usd, "100.5000");
        // no override means "inherit the deployment-wide unpriced policy", and
        // it round-trips as null rather than as a guessed default (#996)
        assert_eq!(budget.unpriced_policy, None);
        let strict = budgets
            .create("project", project.id, "10.0000", "30d", Some("block"))
            .await
            .unwrap();
        assert_eq!(strict.unpriced_policy, Some("block".to_string()));
        assert_eq!(
            budgets.get(strict.id).await.unwrap().unpriced_policy,
            Some("block".to_string())
        );
        // both budgets belong to the project scope, the inherited one and the
        // strict one, so the scope lists two
        assert_eq!(
            budgets
                .list_for_scope("project", project.id)
                .await
                .unwrap()
                .len(),
            2
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
    async fn prompt_template_versions_publish_and_scope_round_trip() {
        let Ok(_) = std::env::var("ROLTER_TEST_DATABASE_URL") else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
        let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();

        let repo = PromptTemplateRepo(&pool);
        let template = repo
            .create_template(
                org.id,
                "support assistant",
                "support-assistant",
                Some("tier 1 support guardrails"),
            )
            .await
            .unwrap();
        assert_eq!(template.published_version, None);
        assert_eq!(repo.list_templates(org.id).await.unwrap().len(), 1);

        let v1 = repo
            .create_version(template.id, &serde_json::json!([]), &serde_json::json!([]))
            .await
            .unwrap();
        let v2 = repo
            .create_version(
                template.id,
                &serde_json::json!([{"name":"tier","required":true}]),
                &serde_json::json!([{"role":"system","position":"prepend","content":"{{ tier }} mode"}]),
            )
            .await
            .unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v2.version, 2);
        let empty_variables = serde_json::json!([]);
        let empty_decorators = serde_json::json!([]);
        let (v3, v4) = tokio::join!(
            repo.create_version(template.id, &empty_variables, &empty_decorators),
            repo.create_version(template.id, &empty_variables, &empty_decorators)
        );
        let mut concurrent_versions = [v3.unwrap().version, v4.unwrap().version];
        concurrent_versions.sort();
        assert_eq!(concurrent_versions, [3, 4]);
        let versions = repo.list_versions(template.id).await.unwrap();
        assert_eq!(versions.len(), 4);
        assert_eq!(versions[0].version, 4);
        assert_eq!(versions[3].version, 1);

        let project = ProjectRepo(&pool)
            .create(
                TeamRepo(&pool).create(org.id, "core").await.unwrap().id,
                "gateway",
            )
            .await
            .unwrap();
        repo.set_scopes(
            template.id,
            2,
            &[
                ("org".to_string(), org.id),
                ("project".to_string(), project.id),
            ],
        )
        .await
        .unwrap();
        let scopes = repo.list_scopes(template.id, 2).await.unwrap();
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].scope_type, "org");
        assert_eq!(scopes[1].scope_type, "project");

        let published = repo.publish_version(template.id, v2.version).await.unwrap();
        assert_eq!(published.published_version, Some(2));
        assert!(repo
            .set_scopes(template.id, 2, &[("org".to_string(), org.id)])
            .await
            .is_err());

        repo.delete_template(template.id).await.unwrap();
        assert!(matches!(
            repo.get_template(template.id).await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn skill_versions_publish_and_retire_round_trip() {
        let Ok(_) = std::env::var("ROLTER_TEST_DATABASE_URL") else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
        let org = OrgRepo(&pool).create("acme", "acme").await.unwrap();

        let repo = SkillRepo(&pool);
        let skill = repo
            .create_skill(
                org.id,
                "classification baseline",
                "classification-baseline",
                Some("first skill"),
                &[],
                "viewer",
            )
            .await
            .unwrap();
        assert_eq!(skill.published_version, None);
        assert_eq!(repo.list_skills(org.id).await.unwrap().len(), 1);

        let v1 = repo
            .create_version(
                skill.id,
                Some("content-v1"),
                None,
                &serde_json::json!({"author":"ops"}),
            )
            .await
            .unwrap();
        let v2 = repo
            .create_version(
                skill.id,
                None,
                Some("oci://registry.example/skills/classification@sha256:abc"),
                &serde_json::json!({"author":"ops"}),
            )
            .await
            .unwrap();
        assert_eq!(v1.version, 1);
        assert_eq!(v2.version, 2);
        let metadata = serde_json::json!({"author":"ops"});
        let (v3, v4) = tokio::join!(
            repo.create_version(skill.id, Some("content-v3"), None, &metadata),
            repo.create_version(skill.id, Some("content-v4"), None, &metadata)
        );
        let mut concurrent_versions = [v3.unwrap().version, v4.unwrap().version];
        concurrent_versions.sort();
        assert_eq!(concurrent_versions, [3, 4]);
        let versions = repo.list_versions(skill.id).await.unwrap();
        assert_eq!(versions.len(), 4);
        assert_eq!(versions[0].version, 4);
        assert_eq!(versions[3].version, 1);

        let published = repo.publish_version(skill.id, 2).await.unwrap();
        assert_eq!(published.published_version, Some(2));
        let resolved = repo.resolve_published(skill.id).await.unwrap();
        assert_eq!(resolved.version, 2);
        assert!(resolved.content.is_none());
        assert_eq!(
            resolved.content_ref.as_deref(),
            Some("oci://registry.example/skills/classification@sha256:abc")
        );

        let retired = repo
            .update_skill(
                skill.id,
                Some("classification baseline"),
                None,
                Some(true),
                None,
                None,
            )
            .await
            .unwrap();
        assert!(retired.retired_at.is_some());
        assert!(repo.resolve_published(skill.id).await.is_err());

        repo.delete_skill(skill.id).await.unwrap();
        assert!(matches!(
            repo.get_skill(skill.id).await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn plugin_instances_round_trip_at_org_and_project_scope() {
        let Ok(_) = std::env::var("ROLTER_TEST_DATABASE_URL") else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
        let org = OrgRepo(&pool).create("plugins", "plugins").await.unwrap();
        let team = TeamRepo(&pool).create(org.id, "platform").await.unwrap();
        let project = ProjectRepo(&pool).create(team.id, "gateway").await.unwrap();
        let repo = PluginRepo(&pool);
        let org_plugin = repo
            .create(
                org.id,
                None,
                "audit",
                "audit",
                "org audit",
                "webhook",
                "post_response",
                false,
                20,
                "fail_open",
                "https://plugins.internal/audit",
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        let project_plugin = repo
            .create(
                org.id,
                Some(project.id),
                "audit override",
                "audit",
                "project audit",
                "webhook",
                "pre_upstream",
                true,
                10,
                "fail_closed",
                "https://plugins.internal/project-audit",
                Some("PLUGIN_TOKEN"),
                &serde_json::json!({"sample": 1.0}),
            )
            .await
            .unwrap();
        assert_eq!(repo.list(org.id).await.unwrap().len(), 2);
        let updated = repo
            .update(
                project_plugin.id,
                Some(project.id),
                "audit override",
                "updated",
                "pre_route",
                false,
                5,
                "fail_open",
                "https://plugins.internal/project-audit",
                None,
                &serde_json::json!({"sample": 0.5}),
            )
            .await
            .unwrap();
        assert_eq!(updated.stage, "pre_route");
        assert_eq!(updated.position, 5);
        repo.delete(org_plugin.id).await.unwrap();
        assert_eq!(repo.list(org.id).await.unwrap().len(), 1);
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
