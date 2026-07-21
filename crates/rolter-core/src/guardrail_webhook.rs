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
    /// inspect the non-streaming response before delivery.
    ///
    /// Reserved: validated and carried in the config, but the gateway does not yet
    /// run the output stage. Streaming/SSE buffering is deferred, mirroring the
    /// built-in guardrail phasing.
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
}

/// Custom guardrail webhook configuration (`[guardrail_webhook]`). Disabled by
/// default; an inert block adds no hot-path cost.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuardrailWebhookConfig {
    #[serde(default)]
    pub enabled: bool,
    /// http(s) endpoint the gateway POSTs the contract envelope to
    #[serde(default)]
    pub url: String,
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
            url: String::new(),
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
        let url = self.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            problems.push(format!(
                "guardrail_webhook.url '{url}' must be an http:// or https:// URL"
            ));
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

    #[test]
    fn disabled_config_is_never_a_problem() {
        let cfg = GuardrailWebhookConfig {
            enabled: false,
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
