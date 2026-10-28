//! Custom guardrail webhook contract and configuration (ROL-257).
//!
//! Lets an operator plug a self-hosted semantic guardrail service (e.g. Guardrails
//! AI, LLM Guard) into the gateway without embedding any vendor or model. The
//! gateway POSTs a stable JSON envelope to the configured HTTP endpoint before
//! proxying a request (and, once the output stage lands, before delivering a
//! response); the service replies with an allow/block/transform/annotate
//! decision. This is the vendor-neutral counterpart to the built-in regex
//! guardrails (ROL-261) — the two can run side by side.
//!
//! This module owns only the config model, validation, and the wire contract.
//! The async HTTP client and request-path wiring live in the gateway.

use serde::{Deserialize, Serialize};

/// Default per-call timeout for the webhook, in milliseconds.
pub const DEFAULT_TIMEOUT_MS: u64 = 2_000;
/// Default cap on the content bytes forwarded to the webhook.
pub const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;

/// Stage at which the webhook is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookStage {
    /// inspect the request before proxying upstream
    #[default]
    PreCall,
    /// inspect the response before delivery (#459). A streamed response is
    /// refused or buffered per the policy's `streaming` strategy (ADR-0037),
    /// never delivered unchecked
    PostCall,
}

/// What the gateway does when the webhook is unreachable, times out, or errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    /// forward the request unchanged (availability over enforcement)
    #[default]
    FailOpen,
    /// reject the request with an OpenAI-compatible error (enforcement over availability)
    FailClosed,
}

/// Where a guardrail provider's verdict comes from (#3329, ADR-0039).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardProviderKind {
    /// an HTTP service spoken to over this module's webhook contract
    #[default]
    Webhook,
    /// a classifier the gateway runs in process from `model_path`
    Local,
    /// a kind this build does not know, read from a newer control plane's
    /// snapshot. It fails closed: the stage blocks whatever the provider's
    /// `failure_mode` says, so a future kind is additive (ADR-0038)
    #[serde(other)]
    Unknown,
}

impl GuardProviderKind {
    /// The stored and serialized spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webhook => "webhook",
            Self::Local => "local",
            Self::Unknown => "unknown",
        }
    }

    /// Parse the stored spelling; `None` for anything but a known kind.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "webhook" => Some(Self::Webhook),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    fn is_webhook(&self) -> bool {
        *self == Self::Webhook
    }
}

/// Whether `value` is a SHA-256 digest written as 64 hex digits.
pub fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Credential attached to the webhook call. The secret is never inlined in
/// config; it is resolved from the named environment variable at call time.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookAuth {
    /// `Authorization: Bearer <env value>`
    Bearer { token_env: String },
    /// a shared secret sent in the `X-Rolter-Guardrail-Secret` header
    SharedSecret { secret_env: String },
}

impl WebhookAuth {
    /// The environment variable this auth mode reads its secret from.
    pub fn env_var(&self) -> &str {
        match self {
            Self::Bearer { token_env } => token_env,
            Self::SharedSecret { secret_env } => secret_env,
        }
    }

    /// The secret, read from the environment unless the variable is one of the
    /// deployment's own secrets (`ROLTER_GUARDRAIL_TOKEN` stays allowed). A row
    /// saved before the control plane refused such names must not send them to
    /// the webhook's url. `allowed` adds the variables a caller documents as
    /// its own credential.
    pub fn resolve_secret(&self, allowed: &[&str]) -> Option<String> {
        crate::secret_env::credential_from_env(self.env_var(), allowed)
    }
}

/// Custom guardrail webhook configuration (`[guardrail_webhook]`). Disabled by
/// default; an inert block adds no hot-path cost.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuardrailWebhookConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Label recorded with this provider's outcomes on the request log
    /// (`provider_blocked:<name>`), so a count follows the provider that
    /// produced it after the active one changes. The registry fills it from the
    /// row's name; empty records the bare action.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `webhook` (the default) or `local`, an in-process classifier
    #[serde(default, skip_serializing_if = "GuardProviderKind::is_webhook")]
    pub kind: GuardProviderKind,
    /// http(s) endpoint the gateway POSTs the contract envelope to; empty for
    /// a local provider
    #[serde(default)]
    pub url: String,
    /// a local provider's weights file on the gateway's file system
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_path: String,
    /// the SHA-256 the weights file must have, as 64 hex digits; required for
    /// a local provider, so a swapped file is refused rather than obeyed
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model_sha256: String,
    #[serde(default)]
    pub stage: WebhookStage,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// additional attempts on a transient failure (connect/timeout/5xx); 0 = none
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub failure_mode: FailureMode,
    /// cap on the content bytes forwarded; oversized content is truncated and flagged
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<WebhookAuth>,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn default_max_body_bytes() -> usize {
    DEFAULT_MAX_BODY_BYTES
}

impl Default for GuardrailWebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            name: String::new(),
            kind: GuardProviderKind::Webhook,
            url: String::new(),
            model_path: String::new(),
            model_sha256: String::new(),
            stage: WebhookStage::default(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_retries: 0,
            failure_mode: FailureMode::default(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            auth: None,
        }
    }
}

impl GuardrailWebhookConfig {
    /// Validate the webhook config. Returns human-readable problems for the
    /// aggregate config validator; an empty vec means it is safe to load. A
    /// disabled block is never a problem, even if other fields are unset.
    pub fn validate(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.enabled {
            return problems;
        }
        match self.kind {
            GuardProviderKind::Webhook => {
                let url = self.url.trim();
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    problems.push(format!(
                        "guardrail_webhook.url '{url}' must be an http:// or https:// URL"
                    ));
                }
                if !self.model_path.is_empty() || !self.model_sha256.is_empty() {
                    problems.push(
                        "guardrail_webhook.model_path and model_sha256 apply to a local \
                         provider only"
                            .to_string(),
                    );
                }
            }
            GuardProviderKind::Local => problems.extend(self.local_problems()),
            // not a problem here: a snapshot from a newer control plane may
            // carry a kind this build does not know, and refusing the whole
            // snapshot over it would freeze every other setting. it fails
            // closed at request time instead, and a config file naming one is
            // refused by `GatewayConfig::validate`
            GuardProviderKind::Unknown => {}
        }
        if self.timeout_ms == 0 {
            problems.push("guardrail_webhook.timeout_ms must be greater than zero".to_string());
        }
        if self.max_body_bytes == 0 {
            problems.push("guardrail_webhook.max_body_bytes must be greater than zero".to_string());
        }
        if let Some(auth) = &self.auth {
            if auth.env_var().trim().is_empty() {
                problems.push(
                    "guardrail_webhook.auth references an empty environment variable name"
                        .to_string(),
                );
            }
        }
        problems
    }

    /// Problems specific to a local provider: it names a model and its pin,
    /// and nothing a webhook call would use.
    fn local_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.model_path.trim().is_empty() {
            problems.push("guardrail_webhook.model_path is required for a local provider".into());
        }
        if !is_sha256_hex(self.model_sha256.trim()) {
            problems.push(
                "guardrail_webhook.model_sha256 must be the model file's SHA-256 as 64 hex digits"
                    .into(),
            );
        }
        if !self.url.trim().is_empty() {
            problems.push("guardrail_webhook.url must be empty for a local provider".into());
        }
        if self.auth.is_some() {
            problems.push("guardrail_webhook.auth does not apply to a local provider".into());
        }
        problems
    }
}

/// Tenant identity forwarded to the webhook as metadata. Ids only — never secrets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct WebhookTenant {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// The JSON envelope the gateway POSTs to the webhook. Borrows so the request
/// path serializes without extra allocation.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookRequest<'a> {
    /// `"request"` for pre-call, `"response"` for post-call
    pub direction: &'a str,
    pub stage: WebhookStage,
    pub model: &'a str,
    pub route: &'a str,
    /// trace/correlation id propagated end to end
    pub trace_id: &'a str,
    pub tenant: WebhookTenant,
    /// true when `content` was truncated to `max_body_bytes`
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// the request or response body under inspection (already size-bounded)
    pub content: &'a serde_json::Value,
    /// the scoped policy that selected this provider (#459); absent for the
    /// unattached providers' implicit policy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<&'a str>,
    /// true when `content` is a buffered streamed response, sent as
    /// `{"stream": true, "text": "…"}` rather than the event sequence
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub streamed: bool,
}

/// The webhook's decision. `action` selects the variant; unknown actions
/// deserialize as [`WebhookDecision::Allow`] so a misbehaving service fails safe
/// toward availability (the operator picks fail-closed explicitly per stage).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WebhookDecision {
    /// forward unchanged
    Allow,
    /// reject with an OpenAI-compatible error
    Block {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// replace the inspected content with `content` before continuing
    Transform { content: serde_json::Value },
    /// forward unchanged but attach structured annotations (recorded, not applied)
    Annotate {
        #[serde(default)]
        annotations: serde_json::Value,
    },
}

impl WebhookDecision {
    /// Parse a webhook response body, defaulting to `allow` on an unrecognized or
    /// malformed decision so a broken service never silently blocks traffic. The
    /// caller's `failure_mode` governs transport failures separately.
    pub fn parse(body: &[u8]) -> Self {
        serde_json::from_slice(body).unwrap_or(WebhookDecision::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> GuardrailWebhookConfig {
        GuardrailWebhookConfig {
            enabled: true,
            name: "injection".to_string(),
            kind: GuardProviderKind::Local,
            model_path: "/models/injection.json".to_string(),
            model_sha256: "ab".repeat(32),
            ..GuardrailWebhookConfig::default()
        }
    }

    #[test]
    fn a_local_provider_needs_a_model_and_its_pin_and_no_url() {
        assert!(local().validate().is_empty(), "{:?}", local().validate());
        let mut bad = local();
        bad.model_path = " ".to_string();
        bad.model_sha256 = "not-a-digest".to_string();
        bad.url = "https://x".to_string();
        bad.auth = Some(WebhookAuth::Bearer {
            token_env: "TOKEN".to_string(),
        });
        let problems = bad.validate().join("\n");
        for expected in [
            "model_path is required",
            "64 hex digits",
            "url must be empty",
            "auth",
        ] {
            assert!(problems.contains(expected), "{expected}: {problems}");
        }
        // a webhook takes neither model field
        let webhook = GuardrailWebhookConfig {
            enabled: true,
            url: "https://x".to_string(),
            model_path: "/m.json".to_string(),
            ..GuardrailWebhookConfig::default()
        };
        assert!(webhook.validate().join("").contains("local provider only"));
    }

    #[test]
    fn an_unknown_kind_parses_and_is_left_to_the_file_validator() {
        let parsed: GuardrailWebhookConfig =
            serde_json::from_value(serde_json::json!({"enabled": true, "kind": "gpu_cluster"}))
                .unwrap();
        assert_eq!(parsed.kind, GuardProviderKind::Unknown);
        // a snapshot carrying it must still apply (it fails closed per request)
        assert!(parsed.validate().is_empty());
        assert_eq!(
            GuardProviderKind::parse("local"),
            Some(GuardProviderKind::Local)
        );
        assert_eq!(GuardProviderKind::parse("unknown"), None);
    }

    #[test]
    fn a_webhook_serializes_without_a_kind_and_a_local_one_with_it() {
        let webhook = serde_json::to_value(GuardrailWebhookConfig::default()).unwrap();
        assert!(webhook.get("kind").is_none());
        assert!(webhook.get("model_path").is_none());
        let local = serde_json::to_value(local()).unwrap();
        assert_eq!(local["kind"], "local");
        assert_eq!(local["model_sha256"], "ab".repeat(32));
    }

    #[test]
    fn disabled_config_is_never_a_problem() {
        let cfg = GuardrailWebhookConfig {
            enabled: false,
            name: String::new(),
            url: String::new(),
            timeout_ms: 0,
            ..GuardrailWebhookConfig::default()
        };
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn enabled_requires_valid_url_and_timeout() {
        let cfg = GuardrailWebhookConfig {
            enabled: true,
            url: "ftp://x".to_string(),
            timeout_ms: 0,
            max_body_bytes: 0,
            ..GuardrailWebhookConfig::default()
        };
        let problems = cfg.validate();
        assert!(problems.iter().any(|p| p.contains("http")));
        assert!(problems.iter().any(|p| p.contains("timeout_ms")));
        assert!(problems.iter().any(|p| p.contains("max_body_bytes")));
    }

    #[test]
    fn valid_enabled_config_passes() {
        let cfg = GuardrailWebhookConfig {
            enabled: true,
            url: "https://guard.internal/check".to_string(),
            auth: Some(WebhookAuth::Bearer {
                token_env: "GUARD_TOKEN".to_string(),
            }),
            ..GuardrailWebhookConfig::default()
        };
        assert!(cfg.validate().is_empty());
    }

    #[test]
    fn empty_auth_env_is_rejected() {
        let cfg = GuardrailWebhookConfig {
            enabled: true,
            url: "https://guard.internal/check".to_string(),
            auth: Some(WebhookAuth::SharedSecret {
                secret_env: "  ".to_string(),
            }),
            ..GuardrailWebhookConfig::default()
        };
        assert!(cfg
            .validate()
            .iter()
            .any(|p| p.contains("environment variable")));
    }

    #[test]
    fn decision_parses_each_action() {
        assert_eq!(
            WebhookDecision::parse(br#"{"action":"allow"}"#),
            WebhookDecision::Allow
        );
        assert_eq!(
            WebhookDecision::parse(br#"{"action":"block","reason":"pii"}"#),
            WebhookDecision::Block {
                reason: Some("pii".to_string())
            }
        );
        assert!(matches!(
            WebhookDecision::parse(br#"{"action":"transform","content":{"messages":[]}}"#),
            WebhookDecision::Transform { .. }
        ));
    }

    #[test]
    fn unknown_or_malformed_decision_defaults_to_allow() {
        assert_eq!(
            WebhookDecision::parse(br#"{"action":"nuke"}"#),
            WebhookDecision::Allow
        );
        assert_eq!(WebhookDecision::parse(b"not json"), WebhookDecision::Allow);
    }
}
