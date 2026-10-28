//! Try a guardrail rule, the whole rule set, or a provider on a sample before
//! it goes live (#2159, #2265, #3084).
//!
//! The rule and provider endpoints take an *unsaved* draft, so the dashboard can test before the
//! first save, and neither writes anything. They are gated like the matching
//! update (`guardrail_rule:update`, `guardrail_provider:update`): the rule trial
//! is read-only but the provider trial makes the control plane egress and
//! reveals whether a credential is set, which is a mutation-level power.
//!
//! The sample is the operator's own text and is never logged, stored or put in
//! an audit entry. The rule trial runs the gateway's own matcher
//! ([`rolter_core::dry_run_rule`]); the provider trial sends the envelope the
//! gateway sends, through the same egress policy the control plane applies to
//! every other operator-supplied URL. The policy trial runs the effective rule
//! set (config-file rules, then the enabled dashboard rules) in order through
//! the same scan, optionally with an unsaved draft slotted in at a position.

use std::time::{Duration, Instant};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use rolter_core::{
    DryRun, DryRunOutcome, GuardAction, GuardStage, GuardrailRule, GuardrailWebhookConfig,
    GuardrailsConfig, PolicyRun, PolicyStep, WebhookAuth, WebhookDecision, WebhookRequest,
    WebhookStage, WebhookTenant,
};
use rolter_store::postgres::repo::GuardrailRepo;

use crate::crud::{invalid_field, pool, ApiResult, SafeJson};
use crate::guardrails::{parse_builtin, parse_webhook_auth};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

/// Longest sample either trial accepts. The gateway scans up to 256 KiB per
/// request; a dashboard textarea never needs that, and a smaller cap keeps one
/// click from costing a megabyte of regex work or of egress.
const MAX_SAMPLE_BYTES: usize = 64 * 1024;

/// Ceiling on the provider's own `timeout_ms` for a test, so a saved
/// 60-second timeout cannot hold a control-plane request open that long.
const MAX_TEST_TIMEOUT_MS: u64 = 10_000;

/// Most of a webhook's answer the test reads. A decision is a few hundred
/// bytes; a transform echoes the (capped) content back, so a megabyte is ample
/// and a misbehaving endpoint cannot make the control plane buffer more.
const MAX_RESPONSE_BYTES: usize = 1 << 20;

/// Longest `reason` echoed back, so an endpoint cannot turn the result into a
/// large payload.
const MAX_REASON_CHARS: usize = 500;

/// What a provider test sends when the operator supplies no sample.
const DEFAULT_SAMPLE: &str = "Hello, this is a harmless test message from the rolter dashboard.";

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/guardrails/rules/test", post(test_rule))
        .route("/api/v1/guardrails/policy/test", post(test_policy))
        .route("/api/v1/guardrails/providers/test", post(test_provider))
}

/// The fields of a rule under trial, shared by the single-rule and policy trials.
#[derive(Debug, Deserialize)]
struct RuleFields {
    #[serde(default)]
    builtin: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default = "default_pre_call")]
    stage: String,
    #[serde(default = "default_annotate")]
    action: String,
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    include_system: bool,
}

#[derive(Debug, Deserialize)]
struct RuleTestBody {
    #[serde(flatten)]
    fields: RuleFields,
    /// treat the sample as a system message, which a rule skips unless it
    /// opts in with `include_system`
    #[serde(default)]
    system: bool,
    sample: String,
}

fn default_pre_call() -> String {
    "pre_call".to_string()
}

fn default_annotate() -> String {
    "annotate".to_string()
}

fn check_sample(sample: &str) -> ApiResult<()> {
    if sample.len() > MAX_SAMPLE_BYTES {
        return Err(invalid_field(
            "sample",
            format!("sample must be at most {MAX_SAMPLE_BYTES} bytes"),
        ));
    }
    Ok(())
}

impl RuleFields {
    fn rule(&self, name: &str) -> ApiResult<GuardrailRule> {
        let stage = parse_stage(&self.stage)?;
        let action = match self.action.as_str() {
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
        Ok(GuardrailRule {
            name: name.to_string(),
            builtin: parse_builtin(self.builtin.as_deref())?,
            pattern: self.pattern.clone().filter(|p| !p.is_empty()),
            stage,
            action,
            // an empty token means "the detector's default", as when saving
            replacement: self.replacement.clone().filter(|t| !t.trim().is_empty()),
            include_system: self.include_system,
        })
    }
}

fn parse_stage(stage: &str) -> ApiResult<GuardStage> {
    match stage {
        "pre_call" => Ok(GuardStage::PreCall),
        "post_call" => Ok(GuardStage::PostCall),
        _ => Err(invalid_field(
            "stage",
            "stage must be pre_call or post_call",
        )),
    }
}

impl RuleTestBody {
    fn rule(&self) -> ApiResult<GuardrailRule> {
        self.fields.rule("draft")
    }
}

async fn test_rule(
    principal: Principal,
    SafeJson(body): SafeJson<RuleTestBody>,
) -> ApiResult<Json<DryRun>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Update))?;
    check_sample(&body.sample)?;
    let rule = body.rule()?;
    let run = rolter_core::dry_run_rule(&rule, &body.sample, body.system).map_err(|problems| {
        // the core check reports one joined text; a pattern is what it most
        // often concerns, and the dialog shows it under that field
        invalid_field("pattern", problems.join("; "))
    })?;
    Ok(Json(run))
}

/// An unsaved rule to slot into the policy.
#[derive(Debug, Deserialize)]
struct PolicyDraft {
    /// a dashboard rule with this name is replaced by the draft, which is how
    /// an edit is tried; a config-file rule's name is refused, since the file
    /// wins that collision in the real policy
    #[serde(default)]
    name: Option<String>,
    /// where the draft runs among the dashboard rules, as on save: ascending
    /// position, ties by name. Config-file rules always run before all of them
    #[serde(default)]
    position: i32,
    #[serde(flatten)]
    fields: RuleFields,
}

#[derive(Debug, Deserialize)]
struct PolicyTestBody {
    #[serde(default = "default_pre_call")]
    stage: String,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    draft: Option<PolicyDraft>,
    sample: String,
}

/// Where a rule in the policy comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RuleSource {
    /// the bootstrap config file; immutable at runtime and run first
    File,
    /// a saved, enabled dashboard rule
    Dashboard,
    /// the unsaved draft under trial
    Draft,
}

#[derive(Debug, Serialize)]
struct PolicyTestStep {
    #[serde(flatten)]
    step: PolicyStep,
    source: RuleSource,
}

/// [`PolicyRun`] with each step labelled by where its rule comes from.
#[derive(Debug, Serialize)]
struct PolicyTestResult {
    outcome: DryRunOutcome,
    steps: Vec<PolicyTestStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocked_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redacted: Option<String>,
}

async fn test_policy(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<PolicyTestBody>,
) -> ApiResult<Json<PolicyTestResult>> {
    // the same gate as the single-rule trial: read-only, but it names every
    // rule of the policy, which only the roles that edit rules may see
    authorize_superadmin(&principal, superadmin_cap!("guardrail_rule", Update))?;
    check_sample(&body.sample)?;
    let stage = parse_stage(&body.stage)?;

    let draft = match &body.draft {
        Some(draft) => {
            let name = draft.name.as_deref().map(str::trim).unwrap_or_default();
            let rule = draft
                .fields
                .rule(if name.is_empty() { "draft" } else { name })?;
            let problems = GuardrailsConfig {
                enabled: true,
                rules: vec![rule.clone()],
                ..GuardrailsConfig::default()
            }
            .validate();
            if !problems.is_empty() {
                return Err(invalid_field("pattern", problems.join("; ")));
            }
            Some((rule, draft.position))
        }
        None => None,
    };

    // the rule list the snapshot hands the gateway, so the answer is what the
    // fleet runs rather than a second reading of the tables
    let effective = state.store.load().await?.guardrails;
    let file_count = state
        .store
        .file_guardrail_rules()
        .min(effective.rules.len());
    let positions: std::collections::HashMap<String, i32> = GuardrailRepo(pool(&state))
        .list_rules()
        .await?
        .into_iter()
        .map(|row| (row.name, row.position))
        .collect();

    let mut config = GuardrailsConfig {
        // a saved dashboard rule switches the policy on, so a draft is tried
        // as it would run once saved
        enabled: effective.enabled || draft.is_some(),
        max_scan_bytes: effective.max_scan_bytes,
        streaming_post_call: effective.streaming_post_call,
        rules: Vec::with_capacity(effective.rules.len() + 1),
    };
    let mut sources = Vec::with_capacity(effective.rules.len() + 1);
    let mut dashboard: Vec<(GuardrailRule, i32, RuleSource)> = Vec::new();
    for (index, rule) in effective.rules.into_iter().enumerate() {
        if index < file_count {
            config.rules.push(rule);
            sources.push(RuleSource::File);
        } else {
            let position = positions.get(&rule.name).copied().unwrap_or(0);
            dashboard.push((rule, position, RuleSource::Dashboard));
        }
    }
    if let Some((rule, position)) = draft {
        let name = rule.name.trim().to_string();
        if config.rules.iter().any(|r| r.name.trim() == name) {
            return Err(invalid_field(
                "name",
                format!("a config-file rule is already named '{name}' and wins that name"),
            ));
        }
        dashboard.retain(|(r, _, _)| r.name.trim() != name);
        let at = dashboard
            .iter()
            .position(|(r, p, _)| (*p, r.name.as_str()) > (position, rule.name.as_str()))
            .unwrap_or(dashboard.len());
        dashboard.insert(at, (rule, position, RuleSource::Draft));
    }
    for (rule, _, source) in dashboard {
        config.rules.push(rule);
        sources.push(source);
    }

    let PolicyRun {
        outcome,
        steps,
        blocked_by,
        redacted,
    } = rolter_core::dry_run_policy(&config, stage, &body.sample, body.system);
    let steps = steps
        .into_iter()
        .zip(sources)
        .map(|(step, source)| PolicyTestStep { step, source })
        .collect();
    Ok(Json(PolicyTestResult {
        outcome,
        steps,
        blocked_by,
        redacted,
    }))
}

#[derive(Debug, Deserialize)]
struct ProviderTestBody {
    url: String,
    #[serde(default = "default_pre_call")]
    stage: String,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: i64,
    #[serde(default = "default_max_body_bytes")]
    max_body_bytes: i64,
    #[serde(default = "default_auth_kind")]
    auth_kind: String,
    #[serde(default)]
    auth_env: Option<String>,
    /// text sent as the user message; a harmless fixed sentence when absent
    #[serde(default)]
    sample: Option<String>,
    /// send the call without the credential, for a webhook that is reached
    /// only through a gateway whose environment the control plane cannot see
    #[serde(default)]
    skip_credential: bool,
}

fn default_timeout_ms() -> i64 {
    rolter_core::guardrail_webhook::DEFAULT_TIMEOUT_MS as i64
}

fn default_max_body_bytes() -> i64 {
    rolter_core::guardrail_webhook::DEFAULT_MAX_BODY_BYTES as i64
}

fn default_auth_kind() -> String {
    "none".to_string()
}

/// Why a provider test did not get a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TrialErrorKind {
    /// the egress policy refuses the url or an address it resolves to
    EgressDenied,
    /// the credential's environment variable is unset or empty on the control
    /// plane; the webhook was not contacted
    CredentialUnset,
    Timeout,
    /// refused, unreachable or unresolvable
    Connection,
    Tls,
    /// a non-2xx answer; the gateway retries these and then applies the
    /// provider's failure mode
    HttpStatus,
    /// a 2xx answer that is not a decision; the gateway treats it as allow
    Unparsable,
    /// the answer exceeded the size the test reads
    ResponseTooLarge,
    Other,
}

#[derive(Debug, Serialize)]
struct TrialError {
    kind: TrialErrorKind,
    message: String,
}

impl TrialError {
    fn new(kind: TrialErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// What the credential did in the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Credential {
    /// the provider has none configured
    NotConfigured,
    /// read from the control plane's environment and attached
    Sent,
    /// the caller asked to leave it out
    Skipped,
    /// the named variable is not set here, so the call was not made
    Unset,
}

/// The webhook's decision, as the gateway would act on it.
#[derive(Debug, Serialize)]
struct TrialDecision {
    /// `allow`, `block`, `transform` or `annotate`
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// the rewritten content, for `transform`
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    /// the annotations, for `annotate`
    #[serde(skip_serializing_if = "Option::is_none")]
    annotations: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ProviderTestResult {
    /// a decision came back; false means see `error`
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    decision: Option<TrialDecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<TrialError>,
    credential: Credential,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    /// whole round trip, including connecting; zero when nothing was sent
    latency_ms: u64,
    /// the sample was cut to `max_body_bytes` before sending
    truncated: bool,
}

impl ProviderTestResult {
    fn failed(credential: Credential, error: TrialError) -> Self {
        Self {
            ok: false,
            decision: None,
            error: Some(error),
            credential,
            http_status: None,
            latency_ms: 0,
            truncated: false,
        }
    }
}

fn webhook_config(body: &ProviderTestBody) -> ApiResult<GuardrailWebhookConfig> {
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
    if body.timeout_ms <= 0 || body.max_body_bytes <= 0 {
        return Err(invalid_field(
            "timeout_ms",
            "timeout_ms and max_body_bytes must be positive",
        ));
    }
    let auth = parse_webhook_auth(&body.auth_kind, body.auth_env.as_deref())?;
    if let Some(auth) = &auth {
        let name = auth.env_var();
        crate::secret_env::refuse("auth_env", name, "to test a webhook")?;
    }
    let config = GuardrailWebhookConfig {
        enabled: true,
        url: body.url.clone(),
        stage,
        timeout_ms: (body.timeout_ms as u64).min(MAX_TEST_TIMEOUT_MS),
        // one attempt: a retry would hide the flakiness a test is there to show
        max_retries: 0,
        max_body_bytes: body.max_body_bytes as usize,
        auth,
        ..GuardrailWebhookConfig::default()
    };
    let problems = config.validate();
    if problems.is_empty() {
        Ok(config)
    } else {
        Err(invalid_field("url", problems.join("; ")))
    }
}

async fn test_provider(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<ProviderTestBody>,
) -> ApiResult<Json<ProviderTestResult>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_provider", Update))?;
    if let Some(sample) = &body.sample {
        check_sample(sample)?;
    }
    let config = webhook_config(&body)?;

    // a refusal is a result, not a 4xx: the dialog shows it where it shows any
    // other reason the webhook was not reached
    if let Err(problem) = state.egress.check_url(config.url.trim(), "url") {
        return Ok(Json(ProviderTestResult::failed(
            Credential::NotConfigured,
            TrialError::new(TrialErrorKind::EgressDenied, problem),
        )));
    }

    let (credential, secret) = match (&config.auth, body.skip_credential) {
        (None, _) => (Credential::NotConfigured, None),
        (Some(_), true) => (Credential::Skipped, None),
        (Some(auth), false) => match std::env::var(auth.env_var()) {
            Ok(value) if !value.is_empty() => (Credential::Sent, Some(value)),
            _ => {
                return Ok(Json(ProviderTestResult::failed(
                    Credential::Unset,
                    TrialError::new(
                        TrialErrorKind::CredentialUnset,
                        format!(
                            "{} is not set on the control plane. Gateways read it from their own \
                             environment, so this says nothing about them; set it here to test \
                             with the credential, or test without it.",
                            auth.env_var()
                        ),
                    ),
                )));
            }
        },
    };

    let sample = body.sample.as_deref().unwrap_or(DEFAULT_SAMPLE);
    Ok(Json(
        send_sample(&state, &config, credential, secret.as_deref(), sample).await,
    ))
}

/// POST the envelope once and describe what came back.
async fn send_sample(
    state: &ControlState,
    config: &GuardrailWebhookConfig,
    credential: Credential,
    secret: Option<&str>,
    sample: &str,
) -> ProviderTestResult {
    let content = serde_json::json!({"messages": [{"role": "user", "content": sample}]});
    // the same bound the gateway applies: an oversized body becomes a
    // truncated preview string and is flagged
    let serialized = serde_json::to_vec(&content).unwrap_or_default();
    let (payload, truncated) = if serialized.len() > config.max_body_bytes {
        let preview: String = content
            .to_string()
            .chars()
            .take(config.max_body_bytes)
            .collect();
        (Value::String(preview), true)
    } else {
        (content, false)
    };
    let trace_id = format!("rolter-test-{}", uuid::Uuid::new_v4());
    let envelope = WebhookRequest {
        direction: match config.stage {
            WebhookStage::PreCall => "request",
            WebhookStage::PostCall => "response",
        },
        stage: config.stage,
        model: "rolter-test",
        route: "rolter-test",
        trace_id: &trace_id,
        tenant: WebhookTenant::default(),
        truncated,
        content: &payload,
        policy: None,
        streamed: false,
    };

    let fail = |error: TrialError, http_status: Option<u16>, started: Instant| ProviderTestResult {
        ok: false,
        decision: None,
        error: Some(error),
        credential,
        http_status,
        latency_ms: started.elapsed().as_millis() as u64,
        truncated,
    };

    let started = Instant::now();
    let client = match crate::egress_client::builder(&state.egress)
        .timeout(Duration::from_millis(config.timeout_ms))
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            return fail(
                TrialError::new(TrialErrorKind::Other, "the test client could not be built"),
                None,
                started,
            )
        }
    };
    let mut request = client
        .post(config.url.trim())
        .header("X-Rolter-Trace-Id", &trace_id)
        .json(&envelope);
    if let (Some(auth), Some(secret)) = (&config.auth, secret) {
        request = match auth {
            WebhookAuth::Bearer { .. } => request.bearer_auth(secret),
            WebhookAuth::SharedSecret { .. } => request.header("X-Rolter-Guardrail-Secret", secret),
        };
    }

    let mut response = match request.send().await {
        Ok(response) => response,
        Err(err) => return fail(classify(&err), None, started),
    };
    let status = response.status().as_u16();
    if !response.status().is_success() {
        // the status alone, never the body: an endpoint's error body can echo
        // the credential it just rejected
        return fail(
            TrialError::new(
                TrialErrorKind::HttpStatus,
                format!("the webhook answered HTTP {status}"),
            ),
            Some(status),
            started,
        );
    }

    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return fail(
                        TrialError::new(
                            TrialErrorKind::ResponseTooLarge,
                            format!("the answer is larger than {MAX_RESPONSE_BYTES} bytes"),
                        ),
                        Some(status),
                        started,
                    );
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(err) => return fail(classify(&err), Some(status), started),
        }
    }

    let latency_ms = started.elapsed().as_millis() as u64;
    // the same strict reading the gateway applies (#459): an answer that is
    // not a decision resolves through the provider's failure mode there, so a
    // test must not report it as a passing one
    let decision =
        match rolter_core::ScoredDecision::parse(&bytes) {
            Some(scored) => scored.decision,
            None => return fail(
                TrialError::new(
                    TrialErrorKind::Unparsable,
                    "the answer is not a decision (an object with an \"action\" of allow, block, \
                     transform or annotate, or scored \"categories\"); the gateway would apply \
                     the provider's failure mode",
                ),
                Some(status),
                started,
            ),
        };
    ProviderTestResult {
        ok: true,
        decision: Some(describe(decision)),
        error: None,
        credential,
        http_status: Some(status),
        latency_ms,
        truncated,
    }
}

fn describe(decision: WebhookDecision) -> TrialDecision {
    match decision {
        WebhookDecision::Allow => TrialDecision {
            action: "allow",
            reason: None,
            content: None,
            annotations: None,
        },
        WebhookDecision::Block { reason } => TrialDecision {
            action: "block",
            reason: reason.map(|r| r.chars().take(MAX_REASON_CHARS).collect()),
            content: None,
            annotations: None,
        },
        WebhookDecision::Transform { content } => TrialDecision {
            action: "transform",
            reason: None,
            content: Some(content),
            annotations: None,
        },
        WebhookDecision::Annotate { annotations } => TrialDecision {
            action: "annotate",
            reason: None,
            content: None,
            annotations: Some(annotations),
        },
    }
}

/// Name a transport failure. The messages are fixed text: a reqwest error
/// prints the url, whose query string can carry a token.
fn classify(err: &reqwest::Error) -> TrialError {
    let mut chain = String::new();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(current) = source {
        chain.push_str(&current.to_string().to_lowercase());
        chain.push(' ');
        source = current.source();
    }
    if chain.contains("egress policy") {
        return TrialError::new(
            TrialErrorKind::EgressDenied,
            "the url resolves to an address the egress policy denies",
        );
    }
    if err.is_timeout() {
        return TrialError::new(
            TrialErrorKind::Timeout,
            "the webhook did not answer in time",
        );
    }
    if chain.contains("certificate") || chain.contains("tls") || chain.contains("handshake") {
        return TrialError::new(
            TrialErrorKind::Tls,
            "the TLS handshake failed or the certificate was not accepted",
        );
    }
    if err.is_connect() {
        return TrialError::new(
            TrialErrorKind::Connection,
            "could not connect to the webhook (refused, unreachable or unresolvable)",
        );
    }
    TrialError::new(TrialErrorKind::Other, "the call failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_body(url: &str) -> ProviderTestBody {
        ProviderTestBody {
            url: url.to_string(),
            stage: "pre_call".into(),
            timeout_ms: 2_000,
            max_body_bytes: 65_536,
            auth_kind: "none".into(),
            auth_env: None,
            sample: None,
            skip_credential: false,
        }
    }

    #[test]
    fn a_test_never_retries_and_caps_the_timeout() {
        let mut body = provider_body("https://guard.internal/check");
        body.timeout_ms = 600_000;
        let config = webhook_config(&body).unwrap();
        assert_eq!(config.max_retries, 0);
        assert_eq!(config.timeout_ms, MAX_TEST_TIMEOUT_MS);
    }

    #[test]
    fn control_plane_secrets_cannot_be_named_as_the_credential() {
        let mut body = provider_body("https://guard.internal/check");
        body.auth_kind = "bearer".into();
        body.auth_env = Some("rolter_kek".into());
        assert!(webhook_config(&body).is_err());
        body.auth_env = Some("ROLTER_GUARDRAIL_TOKEN".into());
        assert!(webhook_config(&body).is_ok());
    }

    #[test]
    fn rejects_a_non_http_url_and_an_oversized_sample() {
        assert!(webhook_config(&provider_body("ftp://x")).is_err());
        assert!(check_sample(&"x".repeat(MAX_SAMPLE_BYTES + 1)).is_err());
        assert!(check_sample(&"x".repeat(MAX_SAMPLE_BYTES)).is_ok());
    }

    #[test]
    fn rule_body_defaults_to_a_pre_call_annotate_rule() {
        let body: RuleTestBody =
            serde_json::from_str(r#"{"pattern":"secret","sample":"a secret"}"#).unwrap();
        let rule = body.rule().unwrap();
        assert_eq!(rule.stage, GuardStage::PreCall);
        assert_eq!(rule.action, GuardAction::Annotate);
        assert!(body.rule().is_ok());
    }

    #[test]
    fn an_empty_replacement_means_the_default_token() {
        let body: RuleTestBody = serde_json::from_str(
            r#"{"builtin":"email","action":"redact","replacement":"  ","sample":"a@b.io"}"#,
        )
        .unwrap();
        assert_eq!(body.rule().unwrap().replacement, None);
    }
}
