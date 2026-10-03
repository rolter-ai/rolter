//! Deployment-wide built-in and webhook guardrail management (#562).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::{
    BuiltinRule, Error, FailureMode, GuardAction, GuardStage, GuardrailRule,
    GuardrailWebhookConfig, GuardrailsConfig, WebhookAuth, WebhookStage,
};
use rolter_store::postgres::models::{GuardrailProvider, GuardrailRule as GuardrailRuleRow};
use rolter_store::postgres::repo::{GuardrailProviderInput, GuardrailRepo, GuardrailRuleInput};

use crate::crud::{log_audit, pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/guardrails/rules",
            get(list_rules).post(create_rule),
        )
        .route(
            "/api/v1/guardrails/rules/{id}",
            axum::routing::put(update_rule).delete(delete_rule),
        )
        .route(
            "/api/v1/guardrails/providers",
            get(list_providers).post(create_provider),
        )
        .route(
            "/api/v1/guardrails/providers/{id}",
            axum::routing::put(update_provider).delete(delete_provider),
        )
}

#[derive(Debug, Deserialize)]
struct RuleBody {
    name: String,
    enabled: bool,
    source_type: String,
    builtin: Option<String>,
    pattern: Option<String>,
    stage: String,
    action: String,
    replacement: Option<String>,
    include_system: bool,
    position: i32,
}

#[derive(Debug, Deserialize)]
struct ProviderBody {
    name: String,
    enabled: bool,
    url: String,
    stage: String,
    timeout_ms: i32,
    max_retries: i32,
    failure_mode: String,
    max_body_bytes: i32,
    auth_kind: String,
    auth_env: Option<String>,
}

impl RuleBody {
    // create and update send the same body and write the same columns, so both
    // borrow their repo input from here rather than restating the field list
    fn as_input(&self) -> GuardrailRuleInput<'_> {
        GuardrailRuleInput {
            name: &self.name,
            enabled: self.enabled,
            source_type: &self.source_type,
            builtin: self.builtin.as_deref(),
            pattern: self.pattern.as_deref(),
            stage: &self.stage,
            action: &self.action,
            replacement: self.replacement.as_deref(),
            include_system: self.include_system,
            position: self.position,
        }
    }
}

impl ProviderBody {
    fn as_input(&self) -> GuardrailProviderInput<'_> {
        GuardrailProviderInput {
            name: &self.name,
            enabled: self.enabled,
            url: &self.url,
            stage: &self.stage,
            timeout_ms: self.timeout_ms,
            max_retries: self.max_retries,
            failure_mode: &self.failure_mode,
            max_body_bytes: self.max_body_bytes,
            auth_kind: &self.auth_kind,
            auth_env: self.auth_env.as_deref(),
        }
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn parse_builtin(value: Option<&str>) -> ApiResult<Option<BuiltinRule>> {
    match value {
        None => Ok(None),
        Some("email") => Ok(Some(BuiltinRule::Email)),
        Some("phone") => Ok(Some(BuiltinRule::Phone)),
        Some("api_token") => Ok(Some(BuiltinRule::ApiToken)),
        Some("payment_card") => Ok(Some(BuiltinRule::PaymentCard)),
        Some(value) => Err(invalid(format!(
            "unknown built-in guardrail rule '{value}'"
        ))),
    }
}

fn validate_rule(body: &RuleBody) -> ApiResult<()> {
    if body.position < 0 {
        return Err(invalid("position must be zero or greater"));
    }
    if !matches!(body.source_type.as_str(), "builtin" | "pattern") {
        return Err(invalid("source_type must be builtin or pattern"));
    }
    match body.source_type.as_str() {
        "builtin" if body.builtin.is_none() || body.pattern.is_some() => {
            return Err(invalid(
                "a built-in rule requires builtin and must not set pattern",
            ));
        }
        "pattern" if body.builtin.is_some() || body.pattern.as_deref().unwrap_or("").is_empty() => {
            return Err(invalid(
                "a pattern rule requires pattern and must not set builtin",
            ));
        }
        _ => {}
    }
    let stage = match body.stage.as_str() {
        "pre_call" => GuardStage::PreCall,
        "post_call" => GuardStage::PostCall,
        _ => return Err(invalid("stage must be pre_call or post_call")),
    };
    let action = match body.action.as_str() {
        "annotate" => GuardAction::Annotate,
        "block" => GuardAction::Block,
        "redact" => GuardAction::Redact,
        _ => return Err(invalid("action must be annotate, block or redact")),
    };
    let rule = GuardrailRule {
        name: body.name.clone(),
        builtin: parse_builtin(body.builtin.as_deref())?,
        pattern: body.pattern.clone(),
        stage,
        action,
        replacement: body.replacement.clone(),
        include_system: body.include_system,
    };
    let problems = GuardrailsConfig {
        enabled: true,
        rules: vec![rule],
        ..GuardrailsConfig::default()
    }
    .validate();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(invalid(problems.join("; ")))
    }
}

fn validate_provider(body: &ProviderBody) -> ApiResult<()> {
    if body.name.trim().is_empty() {
        return Err(invalid("name must not be empty"));
    }
    if body.timeout_ms <= 0 || body.max_retries < 0 || body.max_body_bytes <= 0 {
        return Err(invalid(
            "timeout_ms and max_body_bytes must be positive; max_retries must not be negative",
        ));
    }
    if !(body.url.starts_with("http://") || body.url.starts_with("https://")) {
        return Err(invalid("url must start with http:// or https://"));
    }
    let stage = match body.stage.as_str() {
        "pre_call" => WebhookStage::PreCall,
        "post_call" => WebhookStage::PostCall,
        _ => return Err(invalid("stage must be pre_call or post_call")),
    };
    let failure_mode = match body.failure_mode.as_str() {
        "fail_open" => FailureMode::FailOpen,
        "fail_closed" => FailureMode::FailClosed,
        _ => return Err(invalid("failure_mode must be fail_open or fail_closed")),
    };
    let auth = match (body.auth_kind.as_str(), body.auth_env.as_deref()) {
        ("none", None) => None,
        ("bearer", Some(value)) => Some(WebhookAuth::Bearer {
            token_env: value.to_string(),
        }),
        ("shared_secret", Some(value)) => Some(WebhookAuth::SharedSecret {
            secret_env: value.to_string(),
        }),
        ("none", Some(_)) => return Err(invalid("auth_env must be empty when auth_kind is none")),
        ("bearer" | "shared_secret", None) => {
            return Err(invalid("auth_env is required for the selected auth_kind"));
        }
        _ => return Err(invalid("auth_kind must be none, bearer or shared_secret")),
    };
    let problems = GuardrailWebhookConfig {
        enabled: body.enabled,
        url: body.url.clone(),
        stage,
        timeout_ms: body.timeout_ms as u64,
        max_retries: body.max_retries as u32,
        failure_mode,
        max_body_bytes: body.max_body_bytes as usize,
        auth,
    }
    .validate();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(invalid(problems.join("; ")))
    }
}

async fn list_rules(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<GuardrailRuleRow>>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Read))?;
    Ok(Json(GuardrailRepo(pool(&state)).list_rules().await?))
}

async fn create_rule(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<RuleBody>,
) -> ApiResult<(StatusCode, Json<GuardrailRuleRow>)> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Create))?;
    validate_rule(&body)?;
    let row = GuardrailRepo(pool(&state))
        .create_rule(body.as_input())
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_rule.create",
        "guardrail_rule",
        row.id,
        serde_json::json!({"name": row.name, "enabled": row.enabled}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<RuleBody>,
) -> ApiResult<Json<GuardrailRuleRow>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Update))?;
    validate_rule(&body)?;
    let row = GuardrailRepo(pool(&state))
        .update_rule(id, body.as_input())
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_rule.update",
        "guardrail_rule",
        id,
        serde_json::json!({"name": row.name, "enabled": row.enabled}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Delete))?;
    GuardrailRepo(pool(&state)).delete_rule(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_rule.delete",
        "guardrail_rule",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_providers(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<GuardrailProvider>>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_provider", Read))?;
    Ok(Json(GuardrailRepo(pool(&state)).list_providers().await?))
}

async fn create_provider(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<ProviderBody>,
) -> ApiResult<(StatusCode, Json<GuardrailProvider>)> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_provider", Create))?;
    validate_provider(&body)?;
    let row = GuardrailRepo(pool(&state))
        .create_provider(body.as_input())
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_provider.create",
        "guardrail_provider",
        row.id,
        serde_json::json!({"name": row.name, "enabled": row.enabled}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<ProviderBody>,
) -> ApiResult<Json<GuardrailProvider>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_provider", Update))?;
    validate_provider(&body)?;
    let row = GuardrailRepo(pool(&state))
        .update_provider(id, body.as_input())
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_provider.update",
        "guardrail_provider",
        id,
        serde_json::json!({"name": row.name, "enabled": row.enabled}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_provider", Delete))?;
    GuardrailRepo(pool(&state)).delete_provider(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_provider.delete",
        "guardrail_provider",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_invalid_custom_pattern_before_storage() {
        let body = RuleBody {
            name: "broken".into(),
            enabled: true,
            source_type: "pattern".into(),
            builtin: None,
            pattern: Some("[".into()),
            stage: "pre_call".into(),
            action: "block".into(),
            replacement: None,
            include_system: false,
            position: 0,
        };
        assert!(validate_rule(&body).is_err());
    }

    #[test]
    fn disabled_provider_still_needs_a_storable_url() {
        let body = ProviderBody {
            name: "paused".into(),
            enabled: false,
            url: String::new(),
            stage: "pre_call".into(),
            timeout_ms: 2_000,
            max_retries: 0,
            failure_mode: "fail_open".into(),
            max_body_bytes: 65_536,
            auth_kind: "none".into(),
            auth_env: None,
        };
        assert!(validate_provider(&body).is_err());
    }

    #[test]
    fn rejects_secret_values_without_an_auth_mode() {
        let body = ProviderBody {
            name: "provider".into(),
            enabled: true,
            url: "https://guardrails.internal/evaluate".into(),
            stage: "pre_call".into(),
            timeout_ms: 2_000,
            max_retries: 0,
            failure_mode: "fail_closed".into(),
            max_body_bytes: 65_536,
            auth_kind: "none".into(),
            auth_env: Some("secret".into()),
        };
        assert!(validate_provider(&body).is_err());
    }
}
