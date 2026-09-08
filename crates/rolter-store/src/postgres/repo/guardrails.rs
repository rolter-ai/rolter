//! Built-in guardrail rules and the external guardrail providers they consult.
//!
//! Extracted from the monolithic `repo.rs` as the second domain module (#1042).

use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::{Error, Result};

use super::super::models::{GuardrailProvider, GuardrailRule};
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
    pub url: &'a str,
    pub stage: &'a str,
    pub timeout_ms: i32,
    pub max_retries: i32,
    pub failure_mode: &'a str,
    pub max_body_bytes: i32,
    pub auth_kind: &'a str,
    pub auth_env: Option<&'a str>,
}

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
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "select {GUARDRAIL_RULE_COLUMNS} from guardrail_rules where id = $1"
            ))
            .bind(id),
            self.0,
            || format!("guardrail rule {id}"),
        )
        .await
    }

    pub async fn create_rule(&self, rule: GuardrailRuleInput<'_>) -> Result<GuardrailRule> {
        sqlx::query_as(&format!(
            "insert into guardrail_rules (name, enabled, source_type, builtin, pattern, stage, \
             action, replacement, include_system, position) values \
             ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) returning {GUARDRAIL_RULE_COLUMNS}"
        ))
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
            sqlx::query_as(&format!(
                "update guardrail_rules set name=$2, enabled=$3, source_type=$4, builtin=$5, \
                 pattern=$6, stage=$7, action=$8, replacement=$9, include_system=$10, \
                 position=$11, updated_at=now() where id=$1 returning {GUARDRAIL_RULE_COLUMNS}"
            ))
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
        sqlx::query_as(&format!(
            "select {GUARDRAIL_PROVIDER_COLUMNS} from guardrail_providers order by enabled desc, name"
        ))
        .fetch_all(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn get_provider(&self, id: Uuid) -> Result<GuardrailProvider> {
        fetch_optional_or_not_found(
            sqlx::query_as(&format!(
                "select {GUARDRAIL_PROVIDER_COLUMNS} from guardrail_providers where id=$1"
            ))
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
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if provider.enabled {
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
        .fetch_one(&mut *tx)
        .await
        .map_err(store_err)?;
        tx.commit().await.map_err(store_err)?;
        Ok(row)
    }

    pub async fn update_provider(
        &self,
        id: Uuid,
        provider: GuardrailProviderInput<'_>,
    ) -> Result<GuardrailProvider> {
        let mut tx = self.0.begin().await.map_err(store_err)?;
        if provider.enabled {
            sqlx::query("update guardrail_providers set enabled=false, updated_at=now() where enabled and id<>$1")
                .bind(id).execute(&mut *tx).await.map_err(store_err)?;
        }
        // not fetch_optional_or_not_found: the row is read on the transaction,
        // which must still be committed on the hit
        let row = sqlx::query_as(&format!(
            "update guardrail_providers set name=$2, enabled=$3, url=$4, stage=$5, timeout_ms=$6, \
             max_retries=$7, failure_mode=$8, max_body_bytes=$9, auth_kind=$10, auth_env=$11, \
             updated_at=now() where id=$1 returning {GUARDRAIL_PROVIDER_COLUMNS}"
        ))
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
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_err)?
        .ok_or_else(|| Error::NotFound(format!("guardrail provider {id}")))?;
        tx.commit().await.map_err(store_err)?;
        Ok(row)
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
