//! Built-in guardrail rules and the external guardrail providers they consult.
//!
//! Extracted from the monolithic `repo.rs` as the second domain module (#1042).

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Result;

use super::super::models::{GuardrailPolicyRow, GuardrailProvider, GuardrailRule};
use super::support::{fetch_optional_or_not_found, require_affected, store_err};

/// A guardrail rule's editable columns.
///
/// `create_rule` and `update_rule` write exactly the same set, so they share
/// one input type rather than two identical ones; the id an update needs is
/// its own argument.
#[derive(Debug, Clone, Copy)]
pub struct GuardrailRuleInput<'a> {
    pub name: &'a str,
    pub enabled: bool,
    pub source_type: &'a str,
    pub builtin: Option<&'a str>,
    pub pattern: Option<&'a str>,
    pub stage: &'a str,
    pub action: &'a str,
    pub replacement: Option<&'a str>,
    pub include_system: bool,
    pub position: i32,
}

/// A guardrail provider's editable columns, shared by create and update for
/// the same reason as [`GuardrailRuleInput`].
#[derive(Debug, Clone, Copy)]
pub struct GuardrailProviderInput<'a> {
    pub name: &'a str,
    pub enabled: bool,
    /// `webhook` or `local` (#3329)
    pub kind: &'a str,
    pub url: &'a str,
    /// a local provider's weights file and its sha256; `None` for a webhook
    pub model_path: Option<&'a str>,
    pub model_sha256: Option<&'a str>,
    pub stage: &'a str,
    pub timeout_ms: i32,
    pub max_retries: i32,
    pub failure_mode: &'a str,
    pub max_body_bytes: i32,
    pub auth_kind: &'a str,
    pub auth_env: Option<&'a str>,
}

/// A guardrail policy's editable columns plus its ordered provider list,
/// shared by create and update.
#[derive(Debug, Clone, Copy)]
pub struct GuardrailPolicyInput<'a> {
    pub name: &'a str,
    pub enabled: bool,
    pub scope_kind: &'a str,
    pub scope_id: Option<&'a str>,
    pub provider_ids: &'a [Uuid],
    pub thresholds: &'a serde_json::Value,
    pub on_violation: &'a str,
    pub streaming: &'a str,
    pub description: Option<&'a str>,
}

pub struct GuardrailRepo<'a>(pub &'a PgPool);

/// Every policy with its providers aggregated in order, sorted by precedence
/// (narrowest scope first) and then name. `{filter}` is a `where` clause or
/// nothing.
fn policy_select(filter: &str) -> String {
    format!(
        "select p.id, p.name, p.enabled, p.scope_kind, p.scope_id, \
            coalesce(array_agg(pp.provider_id order by pp.position) \
                filter (where pp.provider_id is not null), '{{}}') as provider_ids, \
            p.thresholds, p.on_violation, p.streaming, p.description, p.created_at, p.updated_at \
         from guardrail_policies p \
         left join guardrail_policy_providers pp on pp.policy_id = p.id \
         {filter} \
         group by p.id \
         order by case p.scope_kind when 'key' then 0 when 'route' then 1 \
            when 'project' then 2 when 'org' then 3 else 4 end, p.name"
    )
}

const GUARDRAIL_RULE_COLUMNS: &str = "id, name, enabled, source_type, builtin, pattern, stage, \
    action, replacement, include_system, position, created_at, updated_at";
pub(crate) const GUARDRAIL_PROVIDER_COLUMNS: &str = "id, name, enabled, kind, url, model_path, \
    model_sha256, stage, timeout_ms, max_retries, failure_mode, max_body_bytes, auth_kind, \
    auth_env, created_at, updated_at";

impl GuardrailRepo<'_> {
    pub async fn list_rules(&self) -> Result<Vec<GuardrailRule>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {GUARDRAIL_RULE_COLUMNS} from guardrail_rules order by position, name"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_rule(&self, id: Uuid) -> Result<GuardrailRule> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {GUARDRAIL_RULE_COLUMNS} from guardrail_rules where id = $1"
            )))
            .bind(id),
            self.0,
            || format!("guardrail rule {id}"),
        )
        .await
    }

    pub async fn create_rule(&self, rule: GuardrailRuleInput<'_>) -> Result<GuardrailRule> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into guardrail_rules (name, enabled, source_type, builtin, pattern, stage, \
             action, replacement, include_system, position) values \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) returning {GUARDRAIL_RULE_COLUMNS}"
        )))
        .bind(rule.name)
        .bind(rule.enabled)
        .bind(rule.source_type)
        .bind(rule.builtin)
        .bind(rule.pattern)
        .bind(rule.stage)
        .bind(rule.action)
        .bind(rule.replacement)
        .bind(rule.include_system)
        .bind(rule.position)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update_rule(
        &self,
        id: Uuid,
        rule: GuardrailRuleInput<'_>,
    ) -> Result<GuardrailRule> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update guardrail_rules set name=$2, enabled=$3, source_type=$4, builtin=$5, \
                 pattern=$6, stage=$7, action=$8, replacement=$9, include_system=$10, \
                 position=$11, updated_at=now() where id=$1 returning {GUARDRAIL_RULE_COLUMNS}"
            )))
            .bind(id)
            .bind(rule.name)
            .bind(rule.enabled)
            .bind(rule.source_type)
            .bind(rule.builtin)
            .bind(rule.pattern)
            .bind(rule.stage)
            .bind(rule.action)
            .bind(rule.replacement)
            .bind(rule.include_system)
            .bind(rule.position),
            self.0,
            || format!("guardrail rule {id}"),
        )
        .await
    }

    pub async fn delete_rule(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from guardrail_rules where id=$1").bind(id),
            self.0,
            || format!("guardrail rule {id}"),
        )
        .await
    }

    pub async fn list_providers(&self) -> Result<Vec<GuardrailProvider>> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {GUARDRAIL_PROVIDER_COLUMNS} from guardrail_providers order by enabled desc, name"
        )))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_provider(&self, id: Uuid) -> Result<GuardrailProvider> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "select {GUARDRAIL_PROVIDER_COLUMNS} from guardrail_providers where id=$1"
            )))
            .bind(id),
            self.0,
            || format!("guardrail provider {id}"),
        )
        .await
    }

    pub async fn create_provider(
        &self,
        provider: GuardrailProviderInput<'_>,
    ) -> Result<GuardrailProvider> {
        // several providers may be enabled at once (#459), so saving one never
        // touches another
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "insert into guardrail_providers (name, enabled, url, stage, timeout_ms, max_retries, \
             failure_mode, max_body_bytes, auth_kind, auth_env, kind, model_path, model_sha256) \
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) \
             returning {GUARDRAIL_PROVIDER_COLUMNS}"
        )))
        .bind(provider.name)
        .bind(provider.enabled)
        .bind(provider.url)
        .bind(provider.stage)
        .bind(provider.timeout_ms)
        .bind(provider.max_retries)
        .bind(provider.failure_mode)
        .bind(provider.max_body_bytes)
        .bind(provider.auth_kind)
        .bind(provider.auth_env)
        .bind(provider.kind)
        .bind(provider.model_path)
        .bind(provider.model_sha256)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update_provider(
        &self,
        id: Uuid,
        provider: GuardrailProviderInput<'_>,
    ) -> Result<GuardrailProvider> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update guardrail_providers set name=$2, enabled=$3, url=$4, stage=$5, \
                 timeout_ms=$6, max_retries=$7, failure_mode=$8, max_body_bytes=$9, \
                 auth_kind=$10, auth_env=$11, kind=$12, model_path=$13, model_sha256=$14, \
                 updated_at=now() where id=$1 \
                 returning {GUARDRAIL_PROVIDER_COLUMNS}"
            )))
            .bind(id)
            .bind(provider.name)
            .bind(provider.enabled)
            .bind(provider.url)
            .bind(provider.stage)
            .bind(provider.timeout_ms)
            .bind(provider.max_retries)
            .bind(provider.failure_mode)
            .bind(provider.max_body_bytes)
            .bind(provider.auth_kind)
            .bind(provider.auth_env)
            .bind(provider.kind)
            .bind(provider.model_path)
            .bind(provider.model_sha256),
            self.0,
            || format!("guardrail provider {id}"),
        )
        .await
    }

    pub async fn list_policies(&self) -> Result<Vec<GuardrailPolicyRow>> {
        sqlx::query_as(sqlx::AssertSqlSafe(policy_select("")))
            .fetch_all(self.0)
            .await
            .map_err(store_err)
    }

    pub async fn get_policy(&self, id: Uuid) -> Result<GuardrailPolicyRow> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(policy_select("where p.id = $1"))).bind(id),
            self.0,
            || format!("guardrail policy {id}"),
        )
        .await
    }

    /// The names of the policies that list provider `id`, enabled or not.
    pub async fn policies_listing_provider(&self, id: Uuid) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "select p.name from guardrail_policies p \
             join guardrail_policy_providers pp on pp.policy_id = p.id \
             where pp.provider_id = $1 order by p.name",
        )
        .bind(id)
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    /// Create a policy and its provider list in one transaction, so the
    /// snapshot never sees a policy without its providers.
    pub async fn create_policy(
        &self,
        policy: GuardrailPolicyInput<'_>,
    ) -> Result<GuardrailPolicyRow> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let id: Uuid = sqlx::query_scalar(
            "insert into guardrail_policies (name, enabled, scope_kind, scope_id, thresholds, \
             on_violation, streaming, description) values ($1,$2,$3,$4,$5,$6,$7,$8) returning id",
        )
        .bind(policy.name)
        .bind(policy.enabled)
        .bind(policy.scope_kind)
        .bind(policy.scope_id)
        .bind(policy.thresholds)
        .bind(policy.on_violation)
        .bind(policy.streaming)
        .bind(policy.description)
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        write_policy_providers(&mut tx, id, policy.provider_ids).await?;
        tx.commit().await.map_err(store_err)?;
        self.get_policy(id).await
    }

    /// Replace a policy's columns and provider list in one transaction.
    pub async fn update_policy(
        &self,
        id: Uuid,
        policy: GuardrailPolicyInput<'_>,
    ) -> Result<GuardrailPolicyRow> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        let updated = sqlx::query(
            "update guardrail_policies set name=$2, enabled=$3, scope_kind=$4, scope_id=$5, \
             thresholds=$6, on_violation=$7, streaming=$8, description=$9, updated_at=now() \
             where id=$1",
        )
        .bind(id)
        .bind(policy.name)
        .bind(policy.enabled)
        .bind(policy.scope_kind)
        .bind(policy.scope_id)
        .bind(policy.thresholds)
        .bind(policy.on_violation)
        .bind(policy.streaming)
        .bind(policy.description)
        .execute(&mut *tx)
        .await
        .map_err(store_err)?;
        if updated.rows_affected() == 0 {
            return Err(rolter_core::Error::NotFound(format!(
                "guardrail policy {id}"
            )));
        }
        sqlx::query("delete from guardrail_policy_providers where policy_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(store_err)?;
        write_policy_providers(&mut tx, id, policy.provider_ids).await?;
        tx.commit().await.map_err(store_err)?;
        self.get_policy(id).await
    }

    pub async fn delete_policy(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from guardrail_policies where id=$1").bind(id),
            self.0,
            || format!("guardrail policy {id}"),
        )
        .await
    }

    pub async fn delete_provider(&self, id: Uuid) -> Result<()> {
        require_affected(
            sqlx::query("delete from guardrail_providers where id=$1").bind(id),
            self.0,
            || format!("guardrail provider {id}"),
        )
        .await
    }
}

async fn write_policy_providers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    policy_id: Uuid,
    provider_ids: &[Uuid],
) -> Result<()> {
    for (position, provider_id) in provider_ids.iter().enumerate() {
        sqlx::query(
            "insert into guardrail_policy_providers (policy_id, provider_id, position) \
             values ($1, $2, $3)",
        )
        .bind(policy_id)
        .bind(provider_id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .execute(&mut **tx)
        .await
        .map_err(store_err)?;
    }
    Ok(())
}
