//! Deployment-wide built-in and webhook guardrail management (#562).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::{
    BuiltinRule, FailureMode, GatewayConfig, GuardAction, GuardProviderKind, GuardStage,
    GuardrailRule, GuardrailWebhookConfig, GuardrailsConfig, WebhookAuth, WebhookStage,
};
use rolter_store::postgres::models::{
    GuardrailPolicyRow, GuardrailProvider, GuardrailRule as GuardrailRuleRow,
};
use rolter_store::postgres::repo::{GuardrailProviderInput, GuardrailRepo, GuardrailRuleInput};

use crate::crud::{invalid_field, log_audit, pool, publish_config_change, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/guardrails/effective", get(get_effective))
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
    /// `webhook` (the default) or `local` (#3329)
    #[serde(default = "default_kind")]
    kind: String,
    /// empty for a local provider
    #[serde(default)]
    url: String,
    #[serde(default)]
    model_path: Option<String>,
    #[serde(default)]
    model_sha256: Option<String>,
    stage: String,
    timeout_ms: i32,
    max_retries: i32,
    failure_mode: String,
    max_body_bytes: i32,
    auth_kind: String,
    auth_env: Option<String>,
}

fn default_kind() -> String {
    "webhook".to_string()
}

impl RuleBody {
    // an empty token would delete matches outright, so it is stored as absent
    // and the detector's default token applies
    fn stored_replacement(&self) -> Option<&str> {
        self.replacement
            .as_deref()
            .filter(|token| !token.trim().is_empty())
    }

    // create and update send the same body and write the same columns, so both
    // borrow their repo input from here rather than restating the field list
    fn as_input(&self) -> GuardrailRuleInput<'_> {
        GuardrailRuleInput {
            // stored trimmed: route overrides are compared against the trimmed
            // name at runtime, so a padded name would never match its override
            name: self.name.trim(),
            enabled: self.enabled,
            source_type: &self.source_type,
            builtin: self.builtin.as_deref(),
            pattern: self.pattern.as_deref(),
            stage: &self.stage,
            action: &self.action,
            replacement: self.stored_replacement(),
            include_system: self.include_system,
            position: self.position,
        }
    }
}

impl ProviderBody {
    fn as_input(&self) -> GuardrailProviderInput<'_> {
        let is_local = self.kind == "local";
        GuardrailProviderInput {
            name: &self.name,
            enabled: self.enabled,
            kind: &self.kind,
            url: &self.url,
            // stored trimmed, the digest lowercased as the schema requires;
            // a webhook stores neither
            model_path: self
                .model_path
                .as_deref()
                .map(str::trim)
                .filter(|_| is_local),
            model_sha256: self.model_sha256.as_deref().filter(|_| is_local),
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

pub(crate) fn parse_builtin(value: Option<&str>) -> ApiResult<Option<BuiltinRule>> {
    match value {
        None => Ok(None),
        Some("email") => Ok(Some(BuiltinRule::Email)),
        Some("phone") => Ok(Some(BuiltinRule::Phone)),
        Some("api_token") => Ok(Some(BuiltinRule::ApiToken)),
        Some("payment_card") => Ok(Some(BuiltinRule::PaymentCard)),
        Some(value) => Err(invalid_field(
            "builtin",
            format!("unknown built-in guardrail rule '{value}'"),
        )),
    }
}

fn validate_rule(body: &RuleBody) -> ApiResult<()> {
    if body.position < 0 {
        return Err(invalid_field(
            "position",
            "position must be zero or greater",
        ));
    }
    if !matches!(body.source_type.as_str(), "builtin" | "pattern") {
        return Err(invalid_field(
            "source_type",
            "source_type must be builtin or pattern",
        ));
    }
    match body.source_type.as_str() {
        "builtin" if body.builtin.is_none() || body.pattern.is_some() => {
            return Err(invalid_field(
                "builtin",
                "a built-in rule requires builtin and must not set pattern",
            ));
        }
        "pattern" if body.builtin.is_some() || body.pattern.as_deref().unwrap_or("").is_empty() => {
            return Err(invalid_field(
                "pattern",
                "a pattern rule requires pattern and must not set builtin",
            ));
        }
        _ => {}
    }
    let stage = match body.stage.as_str() {
        "pre_call" => GuardStage::PreCall,
        "post_call" => GuardStage::PostCall,
        _ => {
            return Err(invalid_field(
                "stage",
                "stage must be pre_call or post_call",
            ))
        }
    };
    let action = match body.action.as_str() {
        "annotate" => GuardAction::Annotate,
        "block" => GuardAction::Block,
        "redact" => GuardAction::Redact,
        _ => {
            return Err(invalid_field(
                "action",
                "action must be annotate, block or redact",
            ))
        }
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
    // the core check reports one joined text, so the field is the one a rule
    // problem most often concerns (an unparsable pattern)
    if problems.is_empty() {
        Ok(())
    } else {
        Err(invalid_field("pattern", problems.join("; ")))
    }
}

/// The webhook credential a provider row names, shared by save and test so the
/// two cannot disagree on which pairs are valid.
pub(crate) fn parse_webhook_auth(
    auth_kind: &str,
    auth_env: Option<&str>,
) -> ApiResult<Option<WebhookAuth>> {
    Ok(match (auth_kind, auth_env) {
        ("none", None) => None,
        ("bearer", Some(value)) => Some(WebhookAuth::Bearer {
            token_env: value.to_string(),
        }),
        ("shared_secret", Some(value)) => Some(WebhookAuth::SharedSecret {
            secret_env: value.to_string(),
        }),
        ("none", Some(_)) => {
            return Err(invalid_field(
                "auth_env",
                "auth_env must be empty when auth_kind is none",
            ))
        }
        ("bearer" | "shared_secret", None) => {
            return Err(invalid_field(
                "auth_env",
                "auth_env is required for the selected auth_kind",
            ));
        }
        _ => {
            return Err(invalid_field(
                "auth_kind",
                "auth_kind must be none, bearer or shared_secret",
            ))
        }
    })
}

fn validate_provider(body: &ProviderBody) -> ApiResult<()> {
    if body.name.trim().is_empty() {
        return Err(invalid_field("name", "name must not be empty"));
    }
    if body.timeout_ms <= 0 || body.max_retries < 0 || body.max_body_bytes <= 0 {
        return Err(invalid_field(
            "timeout_ms",
            "timeout_ms and max_body_bytes must be positive; max_retries must not be negative",
        ));
    }
    let kind = GuardProviderKind::parse(&body.kind)
        .ok_or_else(|| invalid_field("kind", "kind must be webhook or local"))?;
    match kind {
        GuardProviderKind::Local => validate_local_fields(body)?,
        _ => {
            if !(body.url.starts_with("http://") || body.url.starts_with("https://")) {
                return Err(invalid_field(
                    "url",
                    "url must start with http:// or https://",
                ));
            }
            if body.model_path.is_some() || body.model_sha256.is_some() {
                return Err(invalid_field(
                    "model_path",
                    "model_path and model_sha256 apply to a local provider only",
                ));
            }
        }
    }
    let stage = match body.stage.as_str() {
        "pre_call" => WebhookStage::PreCall,
        "post_call" => WebhookStage::PostCall,
        _ => {
            return Err(invalid_field(
                "stage",
                "stage must be pre_call or post_call",
            ))
        }
    };
    let failure_mode = match body.failure_mode.as_str() {
        "fail_open" => FailureMode::FailOpen,
        "fail_closed" => FailureMode::FailClosed,
        _ => {
            return Err(invalid_field(
                "failure_mode",
                "failure_mode must be fail_open or fail_closed",
            ))
        }
    };
    let auth = parse_webhook_auth(&body.auth_kind, body.auth_env.as_deref())?;
    let problems = GuardrailWebhookConfig {
        enabled: body.enabled,
        kind,
        url: body.url.clone(),
        model_path: body.model_path.clone().unwrap_or_default(),
        model_sha256: body.model_sha256.clone().unwrap_or_default(),
        stage,
        timeout_ms: body.timeout_ms as u64,
        max_retries: body.max_retries as u32,
        failure_mode,
        max_body_bytes: body.max_body_bytes as usize,
        auth,
        ..Default::default()
    }
    .validate();
    // as above: the joined text cannot name one field, so the endpoint stands in
    if problems.is_empty() {
        Ok(())
    } else {
        Err(invalid_field("url", problems.join("; ")))
    }
}

/// A local provider names a model and its pin, and no url or credential. The
/// path is where every gateway reads the file; the control plane never opens
/// it, so it is checked for shape only (ADR-0039).
fn validate_local_fields(body: &ProviderBody) -> ApiResult<()> {
    let path = body.model_path.as_deref().map(str::trim).unwrap_or("");
    if path.is_empty() {
        return Err(invalid_field(
            "model_path",
            "model_path is required for a local provider",
        ));
    }
    if path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(invalid_field(
            "model_path",
            "model_path must be a file path of at most 4096 bytes",
        ));
    }
    let digest = body.model_sha256.as_deref().unwrap_or("");
    if !rolter_core::guardrail_webhook::is_sha256_hex(digest)
        || digest.bytes().any(|b| b.is_ascii_uppercase())
    {
        return Err(invalid_field(
            "model_sha256",
            "model_sha256 must be the model file's SHA-256 as 64 lowercase hex digits",
        ));
    }
    if !body.url.is_empty() {
        return Err(invalid_field(
            "url",
            "url must be empty for a local provider",
        ));
    }
    if body.max_retries != 0 {
        return Err(invalid_field(
            "max_retries",
            "a local model gives the same answer twice; max_retries must be 0",
        ));
    }
    if body.auth_kind != "none" || body.auth_env.is_some() {
        return Err(invalid_field(
            "auth_kind",
            "a local provider takes no credential; auth_kind must be none",
        ));
    }
    Ok(())
}

/// Where an effective guardrail setting comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GuardrailSource {
    /// the config file's `[guardrails]` / `[guardrail_webhook]`
    File,
    /// a dashboard row
    Registry,
    /// no webhook is configured anywhere
    None,
}

#[derive(Debug, Serialize)]
struct EffectiveRule {
    #[serde(flatten)]
    rule: GuardrailRule,
    source: GuardrailSource,
    /// the dashboard row behind a registry rule
    rule_id: Option<Uuid>,
    /// the enabled dashboard row a config-file rule displaced
    overrides: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct EffectiveGuardrails {
    /// the `guardrails` feature flag: off, no rule inspects traffic
    guardrails: bool,
    streaming_post_call: rolter_core::StreamingPostCall,
    rules: Vec<EffectiveRule>,
    /// a single-provider view kept for older clients: the file's
    /// `[guardrail_webhook]` when it is on, else the first enabled registry
    /// provider. Several providers can run since #459; `semantic` lists them
    webhook: Option<GuardrailWebhookConfig>,
    source: GuardrailSource,
    /// the registry provider in force, set when `source` is `registry`
    provider_id: Option<Uuid>,
    /// always `null` since #459: an enabled config-file webhook runs beside
    /// the registry's providers rather than displacing one
    overrides: Option<Uuid>,
    /// every provider and policy the gateways run (#459)
    semantic: SemanticView,
}

/// What the gateways' semantic guards are made of.
#[derive(Debug, Serialize)]
struct SemanticView {
    /// every enabled provider, the file's first
    providers: Vec<EffectiveProvider>,
    /// every policy, enabled or not, narrowest scope first
    policies: Vec<EffectivePolicy>,
    /// enabled providers no policy lists, which run on every request no policy
    /// matches
    unattached: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EffectiveProvider {
    name: String,
    kind: GuardProviderKind,
    url: String,
    stage: WebhookStage,
    failure_mode: FailureMode,
    timeout_ms: u64,
    source: GuardrailSource,
    /// the dashboard row behind a registry provider
    provider_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct EffectivePolicy {
    #[serde(flatten)]
    policy: rolter_core::GuardrailPolicy,
    source: GuardrailSource,
    /// the dashboard row behind a registry policy
    policy_id: Option<Uuid>,
}

/// Work out where the effective rules and webhook come from.
///
/// `MergedConfigStore::load` decides the winner (a file rule beats a row of the
/// same name, an enabled file webhook beats the enabled provider); this names
/// the winner from the file's own names rather than comparing fields, so a row
/// that is a copy of the file's rule is still reported as displaced.
fn describe_effective(
    config: GatewayConfig,
    owned: &crate::ConfigOwned,
    rules: &[GuardrailRuleRow],
    providers: &[GuardrailProvider],
    policies: &[GuardrailPolicyRow],
) -> EffectiveGuardrails {
    let enabled_row = |name: &str| rules.iter().find(|row| row.enabled && row.name == name);
    let effective = config
        .guardrails
        .rules
        .into_iter()
        .map(|rule| {
            if owned.guardrail_rules.contains(&rule.name) {
                let overrides = enabled_row(&rule.name).map(|row| row.id);
                EffectiveRule {
                    rule,
                    source: GuardrailSource::File,
                    rule_id: None,
                    overrides,
                }
            } else {
                let rule_id = enabled_row(&rule.name).map(|row| row.id);
                EffectiveRule {
                    rule,
                    source: GuardrailSource::Registry,
                    rule_id,
                    overrides: None,
                }
            }
        })
        .collect();
    let row_id = |name: &str| {
        providers
            .iter()
            .find(|row| row.name.trim() == name)
            .map(|row| row.id)
    };
    let file_owned = |name: &str| owned.semantic_providers.contains(name);
    let compiled = rolter_core::CompiledSemanticGuards::from_config(
        &config.semantic_guards,
        &config.guardrail_webhook,
    );
    let unattached = compiled
        .unattached()
        .into_iter()
        .map(|name| name.trim().to_string())
        .collect();
    let mut running: Vec<EffectiveProvider> = Vec::new();
    if config.guardrail_webhook.enabled {
        let webhook = &config.guardrail_webhook;
        running.push(EffectiveProvider {
            name: webhook.name.trim().to_string(),
            kind: webhook.kind,
            url: webhook.url.clone(),
            stage: webhook.stage,
            failure_mode: webhook.failure_mode,
            timeout_ms: webhook.timeout_ms,
            source: GuardrailSource::File,
            provider_id: None,
        });
    }
    for provider in config
        .semantic_guards
        .providers
        .iter()
        .filter(|p| p.enabled)
    {
        let name = provider.name.trim();
        let from_file = file_owned(name);
        running.push(EffectiveProvider {
            name: name.to_string(),
            kind: provider.kind,
            url: provider.url.clone(),
            stage: provider.stage,
            failure_mode: provider.failure_mode,
            timeout_ms: provider.timeout_ms,
            source: if from_file {
                GuardrailSource::File
            } else {
                GuardrailSource::Registry
            },
            provider_id: (!from_file).then(|| row_id(name)).flatten(),
        });
    }
    let mut described: Vec<EffectivePolicy> = config
        .semantic_guards
        .policies
        .iter()
        .map(|policy| {
            let from_file = owned.semantic_policies.contains(policy.name.trim());
            EffectivePolicy {
                policy: policy.clone(),
                source: if from_file {
                    GuardrailSource::File
                } else {
                    GuardrailSource::Registry
                },
                policy_id: (!from_file)
                    .then(|| {
                        policies
                            .iter()
                            .find(|row| row.name.trim() == policy.name.trim())
                            .map(|row| row.id)
                    })
                    .flatten(),
            }
        })
        .collect();
    described.sort_by_key(|entry| {
        let rank = rolter_core::PolicyScope::PRECEDENCE
            .iter()
            .position(|scope| *scope == entry.policy.scope)
            .unwrap_or(usize::MAX);
        (rank, entry.policy.name.clone())
    });

    // the legacy single-provider view
    let legacy = if config.guardrail_webhook.enabled {
        Some((
            config.guardrail_webhook.clone(),
            GuardrailSource::File,
            None,
        ))
    } else {
        config
            .semantic_guards
            .providers
            .iter()
            .find(|p| p.enabled && !file_owned(p.name.trim()))
            .map(|p| (p.clone(), GuardrailSource::Registry, row_id(p.name.trim())))
    };
    let (webhook, source, provider_id) = match legacy {
        Some((webhook, source, id)) => (Some(webhook), source, id),
        None => (None, GuardrailSource::None, None),
    };
    EffectiveGuardrails {
        guardrails: config.feature_flags.guardrails,
        streaming_post_call: config.guardrails.streaming_post_call,
        rules: effective,
        webhook,
        source,
        provider_id,
        overrides: None,
        semantic: SemanticView {
            providers: running,
            policies: described,
            unattached,
        },
    }
}

async fn get_effective(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<EffectiveGuardrails>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Read))?;
    let repo = GuardrailRepo(pool(&state));
    let rules = repo.list_rules().await?;
    let providers = repo.list_providers().await?;
    let policies = repo.list_policies().await?;
    let config = state.store.load().await?;
    Ok(Json(describe_effective(
        config,
        &state.config_owned,
        &rules,
        &providers,
        &policies,
    )))
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
    if body.kind != "local" {
        crate::crud::require_allowed_egress(&state, &body.url, "guardrail provider url")?;
    }
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
    if body.kind != "local" {
        crate::crud::require_allowed_egress(&state, &body.url, "guardrail provider url")?;
    }
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
    let repo = GuardrailRepo(pool(&state));
    // a policy that lists the provider would silently stop checking with it,
    // so the policy has to let go of it first (#459)
    let listing = repo.policies_listing_provider(id).await?;
    if !listing.is_empty() {
        return Err(crate::crud::ApiError::Conflict(format!(
            "guardrail provider is listed by the policies {}; remove it from them first",
            listing.join(", ")
        )));
    }
    repo.delete_provider(id).await?;
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

    fn core_rule(name: &str) -> GuardrailRule {
        GuardrailRule {
            name: name.into(),
            builtin: Some(BuiltinRule::Email),
            pattern: None,
            stage: GuardStage::PreCall,
            action: GuardAction::Redact,
            replacement: None,
            include_system: false,
        }
    }

    fn row(name: &str, enabled: bool) -> GuardrailRuleRow {
        GuardrailRuleRow {
            id: Uuid::new_v4(),
            name: name.into(),
            enabled,
            source_type: "builtin".into(),
            builtin: Some("email".into()),
            pattern: None,
            stage: "pre_call".into(),
            action: "redact".into(),
            replacement: None,
            include_system: false,
            position: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn provider(enabled: bool) -> GuardrailProvider {
        GuardrailProvider {
            id: Uuid::new_v4(),
            name: "hook".into(),
            enabled,
            kind: "webhook".into(),
            url: "https://guard.internal/check".into(),
            model_path: None,
            model_sha256: None,
            stage: "pre_call".into(),
            timeout_ms: 2_000,
            max_retries: 0,
            failure_mode: "fail_open".into(),
            max_body_bytes: 65_536,
            auth_kind: "none".into(),
            auth_env: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn webhook() -> GuardrailWebhookConfig {
        GuardrailWebhookConfig {
            enabled: true,
            url: "https://guard.internal/check".into(),
            ..GuardrailWebhookConfig::default()
        }
    }

    #[test]
    fn a_file_rule_displaces_an_identical_enabled_row() {
        let copy = row("email", true);
        let mut config = GatewayConfig::default();
        config.guardrails.rules = vec![core_rule("email"), core_rule("dashboard-only")];
        let mut owned = crate::ConfigOwned::default();
        owned.guardrail_rules.insert("email".into());
        let rows = [copy.clone(), row("dashboard-only", true)];
        let out = describe_effective(config, &owned, &rows, &[], &[]);
        assert_eq!(out.rules[0].source, GuardrailSource::File);
        assert_eq!(out.rules[0].rule_id, None);
        assert_eq!(out.rules[0].overrides, Some(copy.id));
        assert_eq!(out.rules[1].source, GuardrailSource::Registry);
        assert_eq!(out.rules[1].rule_id, Some(rows[1].id));
        assert_eq!(out.rules[1].overrides, None);
    }

    #[test]
    fn the_legacy_webhook_view_prefers_the_file_and_displaces_nothing() {
        let row = provider(true);
        let registry = GuardrailWebhookConfig {
            name: "hook".into(),
            ..webhook()
        };
        let config = GatewayConfig {
            semantic_guards: rolter_core::SemanticGuardsConfig {
                providers: vec![registry],
                policies: Vec::new(),
                ..Default::default()
            },
            ..GatewayConfig::default()
        };
        let owned = crate::ConfigOwned::default();
        let from_row =
            describe_effective(config.clone(), &owned, &[], std::slice::from_ref(&row), &[]);
        assert_eq!(from_row.source, GuardrailSource::Registry);
        assert_eq!(from_row.provider_id, Some(row.id));
        assert_eq!(from_row.semantic.providers.len(), 1);
        assert_eq!(from_row.semantic.unattached, ["hook"]);

        let both = GatewayConfig {
            guardrail_webhook: webhook(),
            ..config
        };
        let from_file = describe_effective(both, &owned, &[], std::slice::from_ref(&row), &[]);
        assert_eq!(from_file.source, GuardrailSource::File);
        assert_eq!(from_file.provider_id, None);
        assert_eq!(from_file.overrides, None);
        // both run: the file's webhook no longer displaces the registry's
        let sources: Vec<GuardrailSource> = from_file
            .semantic
            .providers
            .iter()
            .map(|p| p.source)
            .collect();
        assert_eq!(sources, [GuardrailSource::File, GuardrailSource::Registry]);
        let none = describe_effective(GatewayConfig::default(), &owned, &[], &[], &[]);
        assert_eq!(none.source, GuardrailSource::None);
        assert!(none.webhook.is_none());
        assert!(none.semantic.providers.is_empty());
    }

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
    fn empty_replacement_is_stored_as_absent() {
        let mut body = RuleBody {
            name: "email".into(),
            enabled: true,
            source_type: "builtin".into(),
            builtin: Some("email".into()),
            pattern: None,
            stage: "pre_call".into(),
            action: "redact".into(),
            replacement: Some(String::new()),
            include_system: false,
            position: 0,
        };
        assert_eq!(body.as_input().replacement, None);
        body.replacement = Some("  ".into());
        assert_eq!(body.as_input().replacement, None);
        body.replacement = Some("[X]".into());
        assert_eq!(body.as_input().replacement, Some("[X]"));
    }

    #[test]
    fn disabled_provider_still_needs_a_storable_url() {
        let body = ProviderBody {
            name: "paused".into(),
            enabled: false,
            kind: "webhook".into(),
            url: String::new(),
            model_path: None,
            model_sha256: None,
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
            kind: "webhook".into(),
            url: "https://guardrails.internal/evaluate".into(),
            model_path: None,
            model_sha256: None,
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

    fn local_body() -> ProviderBody {
        ProviderBody {
            name: "injection".into(),
            enabled: true,
            kind: "local".into(),
            url: String::new(),
            model_path: Some(" /models/injection.json ".into()),
            model_sha256: Some("ab".repeat(32)),
            stage: "pre_call".into(),
            timeout_ms: 50,
            max_retries: 0,
            failure_mode: "fail_closed".into(),
            max_body_bytes: 65_536,
            auth_kind: "none".into(),
            auth_env: None,
        }
    }

    fn refused_field(body: &ProviderBody) -> String {
        match validate_provider(body) {
            Err(error) => format!("{error:?}"),
            Ok(()) => panic!("expected a refusal"),
        }
    }

    #[test]
    fn a_local_provider_is_validated_by_its_own_fields() {
        let body = local_body();
        assert!(validate_provider(&body).is_ok());
        let input = body.as_input();
        assert_eq!(input.model_path, Some("/models/injection.json"));
        assert_eq!(input.url, "");

        let mut no_path = local_body();
        no_path.model_path = None;
        assert!(refused_field(&no_path).contains("model_path"));
        let mut bad_pin = local_body();
        bad_pin.model_sha256 = Some("AB".repeat(32));
        assert!(refused_field(&bad_pin).contains("model_sha256"));
        let mut with_url = local_body();
        with_url.url = "https://x".into();
        assert!(refused_field(&with_url).contains("url"));
        let mut with_auth = local_body();
        with_auth.auth_kind = "bearer".into();
        with_auth.auth_env = Some("TOKEN".into());
        assert!(refused_field(&with_auth).contains("auth_kind"));
        let mut retried = local_body();
        retried.max_retries = 2;
        assert!(refused_field(&retried).contains("max_retries"));
        let mut unknown = local_body();
        unknown.kind = "gpu".into();
        assert!(refused_field(&unknown).contains("kind"));
        // a webhook carries no model fields
        let mut webhook = local_body();
        webhook.kind = "webhook".into();
        webhook.url = "https://guard.internal/check".into();
        assert!(refused_field(&webhook).contains("model_path"));
    }

    #[test]
    fn a_body_without_a_kind_is_a_webhook() {
        let body: ProviderBody = serde_json::from_value(serde_json::json!({
            "name": "hook", "enabled": true, "url": "https://guard.internal/check",
            "stage": "pre_call", "timeout_ms": 2000, "max_retries": 0,
            "failure_mode": "fail_open", "max_body_bytes": 65536, "auth_kind": "none"
        }))
        .unwrap();
        assert_eq!(body.kind, "webhook");
        assert!(validate_provider(&body).is_ok());
        assert_eq!(body.as_input().model_path, None);
    }
}
