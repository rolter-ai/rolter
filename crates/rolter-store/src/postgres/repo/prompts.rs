//! Prompt templates, plugins and skills.

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{
    PluginInstance, PromptTemplate, PromptTemplateScope, PromptTemplateVersion, Skill, SkillVersion,
};
use super::support::{
    fetch_optional_or_not_found, fetch_scalar_or_not_found, require_affected, store_err,
};

/// Every column of `skills` that its row type reads.
const SKILL_COLUMNS: &str = "id, org_id, name, slug, description, retired_at, published_version, \
    allowed_team_ids, minimum_role, created_at";

/// Every column of `skill_versions` that its row type reads.
const SKILL_VERSION_COLUMNS: &str = "skill_id, version, content, content_ref, metadata, created_at";

/// Every column of `prompt_templates` that its row type reads.
const PROMPT_TEMPLATE_COLUMNS: &str =
    "id, org_id, name, slug, description, published_version, created_at";

/// Every column of `prompt_template_versions` that its row type reads.
const PROMPT_TEMPLATE_VERSION_COLUMNS: &str =
    "template_id, version, variables, decorators, created_at";

/// Prompt templates with immutable versions and explicit scope assignments.
pub struct PromptTemplateRepo<'a>(pub &'a PgPool);

impl PromptTemplateRepo<'_> {
    pub async fn list_templates(&self, org_id: Uuid) -> Result<Vec<PromptTemplate>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PROMPT_TEMPLATE_COLUMNS}
             from prompt_templates where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_template(&self, id: Uuid) -> Result<PromptTemplate> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {PROMPT_TEMPLATE_COLUMNS}
             from prompt_templates where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("prompt template {id}"),
        )
        .await
    }

    pub async fn create_template(
        &self,
        org_id: Uuid,
        name: &str,
        slug: &str,
        description: Option<&str>,
    ) -> Result<PromptTemplate> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into prompt_templates (org_id, name, slug, description)
             values ($1, $2, $3, $4)
             returning {PROMPT_TEMPLATE_COLUMNS}"
        )))
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
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update prompt_templates set
                 name = coalesce($2, name),
                 description = case when $3::bool then $4 else description end
             where id = $1
             returning {PROMPT_TEMPLATE_COLUMNS}"
            )))
            .bind(id)
            .bind(name)
            .bind(description.is_some())
            .bind(description.unwrap_or("")),
            self.0,
            || format!("prompt template {id}"),
        )
        .await
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
            sqlx::AssertSqlSafe(format!("insert into prompt_template_versions (template_id, \
                version, variables, decorators)
             values (
                 $1,
                 coalesce((select max(version) + 1 from prompt_template_versions where template_id = $1), 1),
                 $2,
                 $3
             )
             returning {PROMPT_TEMPLATE_VERSION_COLUMNS}")),
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
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PROMPT_TEMPLATE_VERSION_COLUMNS}
             from prompt_template_versions where template_id = $1 order by version desc"
        )))
        .bind(template_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn publish_version(&self, template_id: Uuid, version: i32) -> Result<PromptTemplate> {
        let _: i32 = fetch_scalar_or_not_found(
            sqlx::query_scalar(
                "select 1 from prompt_template_versions where template_id = $1 and version = $2",
            )
            .bind(template_id)
            .bind(version),
            self.0,
            || format!("prompt template version {template_id}:{version}"),
        )
        .await?;
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update prompt_templates
             set published_version = $2
             where id = $1
             returning {PROMPT_TEMPLATE_COLUMNS}"
            )))
            .bind(template_id)
            .bind(version),
            self.0,
            || format!("prompt template {template_id}"),
        )
        .await
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
        require_affected(
            sqlx::query("delete from prompt_templates where id = $1").bind(id),
            self.0,
            || format!("prompt template {id}"),
        )
        .await?;
        Ok(())
    }
}

pub struct PluginRepo<'a>(pub &'a PgPool);

const PLUGIN_COLUMNS: &str = "id, org_id, project_id, name, slug, description, kind, stage, \
    enabled, position, failure_mode, endpoint, secret_env, config, created_at, updated_at";

/// The settings a plugin instance is created with. A struct rather than twelve
/// positional arguments, so a caller cannot transpose `kind` and `stage` or
/// `endpoint` and `secret_env`.
#[derive(Debug, Clone, Copy)]
pub struct NewPlugin<'a> {
    pub project_id: Option<Uuid>,
    pub name: &'a str,
    pub slug: &'a str,
    pub description: &'a str,
    pub kind: &'a str,
    pub stage: &'a str,
    pub enabled: bool,
    pub position: i32,
    pub failure_mode: &'a str,
    pub endpoint: &'a str,
    pub secret_env: Option<&'a str>,
    pub config: &'a serde_json::Value,
}

/// The editable settings of a plugin instance; `update` replaces all of them.
/// Kind and slug are fixed at creation.
#[derive(Debug, Clone, Copy)]
pub struct PluginUpdate<'a> {
    pub project_id: Option<Uuid>,
    pub name: &'a str,
    pub description: &'a str,
    pub stage: &'a str,
    pub enabled: bool,
    pub position: i32,
    pub failure_mode: &'a str,
    pub endpoint: &'a str,
    pub secret_env: Option<&'a str>,
    pub config: &'a serde_json::Value,
}

impl PluginRepo<'_> {
    pub async fn list(&self, org_id: Uuid) -> Result<Vec<PluginInstance>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PLUGIN_COLUMNS} from plugin_instances where org_id=$1 order by position, name"
        )))
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
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {PLUGIN_COLUMNS} from plugin_instances \
             where enabled and kind = 'webhook' order by org_id, position, name"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get(&self, id: Uuid) -> Result<PluginInstance> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {PLUGIN_COLUMNS} from plugin_instances where id=$1"
            )))
            .bind(id),
            self.0,
            || format!("plugin {id}"),
        )
        .await
    }

    pub async fn create(&self, org_id: Uuid, values: NewPlugin<'_>) -> Result<PluginInstance> {
        let NewPlugin {
            project_id,
            name,
            slug,
            description,
            kind,
            stage,
            enabled,
            position,
            failure_mode,
            endpoint,
            secret_env,
            config,
        } = values;
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into plugin_instances (org_id, project_id, name, slug, description, kind, \
             stage, enabled, position, failure_mode, endpoint, secret_env, config) values \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) returning {PLUGIN_COLUMNS}"
        )))
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

    pub async fn update(&self, id: Uuid, values: PluginUpdate<'_>) -> Result<PluginInstance> {
        let PluginUpdate {
            project_id,
            name,
            description,
            stage,
            enabled,
            position,
            failure_mode,
            endpoint,
            secret_env,
            config,
        } = values;
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update plugin_instances set project_id=$2, name=$3, description=$4, stage=$5, \
             enabled=$6, position=$7, failure_mode=$8, endpoint=$9, secret_env=$10, config=$11, \
             updated_at=now() where id=$1 returning {PLUGIN_COLUMNS}"
            )))
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
            .bind(config),
            self.0,
            || format!("plugin {id}"),
        )
        .await
    }

    pub async fn delete(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from plugin_instances where id=$1").bind(id),
            self.0,
            || format!("plugin {id}"),
        )
        .await?;
        Ok(())
    }
}

pub struct SkillRepo<'a>(pub &'a PgPool);

impl SkillRepo<'_> {
    pub async fn list_skills(&self, org_id: Uuid) -> Result<Vec<Skill>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SKILL_COLUMNS}
             from skills where org_id = $1 order by name"
        )))
        .bind(org_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_skill(&self, id: Uuid) -> Result<Skill> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {SKILL_COLUMNS}
             from skills where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("skill {id}"),
        )
        .await
    }

    pub async fn get_by_slug(&self, org_id: Uuid, slug: &str) -> Result<Skill> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {SKILL_COLUMNS}
             from skills where org_id = $1 and slug = $2"
            )))
            .bind(org_id)
            .bind(slug),
            self.0,
            || format!("skill {org_id}:{slug}"),
        )
        .await
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
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into skills (
                 org_id, name, slug, description, allowed_team_ids, minimum_role
             )
             values ($1, $2, $3, $4, $5, $6)
             returning {SKILL_COLUMNS}"
        )))
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
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
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
             returning {SKILL_COLUMNS}"
            )))
            .bind(id)
            .bind(name)
            .bind(description.is_some())
            .bind(description.unwrap_or(""))
            .bind(retired)
            .bind(allowed_team_ids)
            .bind(minimum_role),
            self.0,
            || format!("skill {id}"),
        )
        .await
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
        let version = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into skill_versions (skill_id, version, content, content_ref, metadata)
             values (
                 $1,
                 coalesce((select max(version) + 1 from skill_versions where skill_id = $1), 1),
                 $2,
                 $3,
                 $4
             )
             returning {SKILL_VERSION_COLUMNS}"
        )))
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
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {SKILL_VERSION_COLUMNS}
             from skill_versions where skill_id = $1 order by version desc"
        )))
        .bind(skill_id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn resolve_published(&self, skill_id: Uuid) -> Result<SkillVersion> {
        fetch_optional_or_not_found(
            sqlx::query_as(
                "select v.skill_id, v.version, v.content, v.content_ref, v.metadata, v.created_at
             from skill_versions v
             join skills s
               on s.id = v.skill_id
              and s.published_version = v.version
             where s.id = $1 and s.retired_at is null",
            )
            .bind(skill_id),
            self.0,
            || format!("published skill {skill_id}"),
        )
        .await
    }

    pub async fn publish_version(&self, skill_id: Uuid, version: i32) -> Result<Skill> {
        let _: i32 = fetch_scalar_or_not_found(
            sqlx::query_scalar("select 1 from skill_versions where skill_id = $1 and version = $2")
                .bind(skill_id)
                .bind(version),
            self.0,
            || format!("skill version {skill_id}:{version}"),
        )
        .await?;
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update skills
             set published_version = $2
             where id = $1
             returning {SKILL_COLUMNS}"
            )))
            .bind(skill_id)
            .bind(version),
            self.0,
            || format!("skill {skill_id}"),
        )
        .await
    }

    pub async fn delete_skill(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from skills where id = $1").bind(id),
            self.0,
            || format!("skill {id}"),
        )
        .await?;
        Ok(())
    }
}
