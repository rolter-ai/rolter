use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rustls_pki_types::{pem::PemObject, CertificateDer};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::error::Result;

const COMPLEXITY_POLICY_PARAM: &str = "_rolter_complexity";

/// Root bootstrap configuration loaded from a TOML file or the database.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GatewayConfig {
    /// Schema version this file was written for (#326).
    ///
    /// Absent on every file written before the stamp existed, which is why it
    /// is optional and why [`crate::config_migrate::PRE_STAMP_SCHEMA_VERSION`]
    /// stands in for absence rather than the chain treating it as a special
    /// case. Set by [`crate::config_migrate::migrate`] on the way through, so a
    /// loaded config always reports the version it was understood as, not the
    /// version the file on disk carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<u32>,
    /// Providers the store loader left out because their stored row could not
    /// be read (#2950).
    ///
    /// A bootstrap-file route, group or key may name a database provider, and
    /// to [`sanitize_for_snapshot`](Self::sanitize_for_snapshot) a provider
    /// the loader never returned is just an unknown name, which is refused as
    /// a typo. This set lets that pass tell the two apart and prune the
    /// reference like any other omitted provider. Never read from or written
    /// to a file or the wire: it only travels from the store to the sanitizer.
    ///
    /// Each entry is `(org, name)`, with the global namespace written `""`
    /// ([`crate::namespace::GLOBAL`]), since two orgs may each have a provider
    /// of one name (#1857) and one org's unreadable row must not take the
    /// other's references with it (#3464).
    #[serde(skip)]
    pub left_out_providers: std::collections::HashSet<(String, String)>,
    #[serde(default)]
    pub server: ServerConfig,
    /// outbound TLS trust configuration shared by upstream providers
    #[serde(default)]
    pub tls: TlsConfig,
    /// effective (readonly) providers: the deprecated top-level `[[providers]]`
    /// array and the `[providers] readonly = [...]` tier both land here (ADR-0022)
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// `[providers] default = [...]`: idempotently seeded into the default DB
    /// project at startup, then DB-owned and editable (ADR-0022). Never
    /// config-owned; empty for the deprecated array form
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_defaults: Vec<ProviderConfig>,
    #[serde(default)]
    pub routes: Vec<ModelRoute>,
    /// effective (readonly) provider groups addressable as `group-slug/model`
    /// (ADR-0017 addendum): the deprecated `[[provider_groups]]` array and the
    /// `[provider_groups] readonly = [...]` tier both land here
    #[serde(default)]
    pub provider_groups: Vec<ProviderGroupConfig>,
    /// `[provider_groups] default = [...]`: seeded to the DB once, then editable
    /// (ADR-0022); never config-owned
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_group_defaults: Vec<ProviderGroupConfig>,
    /// Model presets declared in the bootstrap file. `readonly` routes remain
    /// config-owned; `default` routes are a one-time control-plane DB seed.
    #[serde(default)]
    pub models: ModelTiersConfig,
    #[serde(default)]
    pub virtual_keys: Vec<VirtualKeyConfig>,
    /// database-defined virtual keys, carried as peppered digests plus scope
    /// identity (never plaintext). composed from the store, not the toml
    #[serde(default)]
    pub db_virtual_keys: Vec<VirtualKeyRecord>,
    /// MCP servers projected from the control plane into the protected
    /// control-to-gateway snapshot channel.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// live, scope-valid MCP OAuth sessions. Access tokens cross only the
    /// protected internal snapshot channel and are never exposed publicly.
    #[serde(default)]
    pub mcp_oauth_sessions: Vec<McpOAuthSessionConfig>,
    /// enabled MCP tool groups. The proxy treats them as an access boundary:
    /// a server named by any of them only serves the tools they list (#1403)
    #[serde(default)]
    pub mcp_tool_groups: Vec<McpToolGroupConfig>,
    /// organization MCP settings the proxy applies per request (#1404). An
    /// organization with no entry has never saved any and gets the defaults
    #[serde(default)]
    pub mcp_settings: Vec<McpOrgSettingsConfig>,
    /// Whether an organization that never saved MCP settings enforces its
    /// servers' tool manifests (#3398). Read by `rolter-seed --import` and
    /// written by `rolter config export`; the control plane never ships it in
    /// the snapshot, which carries the marker inside each server instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_manifest_enforcement: Vec<McpManifestEnforcementConfig>,
    /// The MCP gateway settings an organization saved (#3482). Read by
    /// `rolter-seed --import` and written by `rolter config export`; the
    /// control plane never ships it in the snapshot, which carries the same
    /// values per organization as `mcp_settings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_gateway_settings: Vec<McpGatewaySettingsConfig>,
    /// The MCP tool groups an organization defined, with the teams, projects
    /// and keys each is assigned to (#3514). Read by `rolter-seed --import`
    /// and written by `rolter config export`; the control plane never ships it
    /// in the snapshot, which carries the groups as `mcp_tool_groups`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_gateway_tool_groups: Vec<McpGatewayToolGroupConfig>,
    #[serde(default)]
    pub model_prices: Vec<ModelPriceConfig>,
    /// settlement currency and rate table. Budgets and accumulated spend are
    /// denominated in `currency.base`; a model priced in anything else is
    /// converted through this table before it reaches a counter
    #[serde(default)]
    pub currency: crate::currency::CurrencyConfig,
    /// spend caps enforced by the gateway against Redis-tracked cumulative cost
    #[serde(default)]
    pub budgets: Vec<BudgetConfig>,
    /// what to do about traffic against a model with no price row, which
    /// accrues no spend and so can never exceed a budget (#974).
    ///
    /// Deliberately deployment-wide rather than per-budget: it decides what the
    /// gateway may serve at all, which is not a property of any one counter,
    /// and a request that matches no budget needs an answer too. A per-budget
    /// override is tracked separately — it needs a schema column and a CRUD
    /// surface to exist for DB-defined budgets as well as toml ones, and a
    /// setting that silently only worked for half of them would be worse than
    /// not having it.
    #[serde(default)]
    pub unpriced_policy: UnpricedPolicy,
    /// request/token throughput caps enforced against a Redis sliding window
    #[serde(default)]
    pub rate_limits: Vec<RateLimitConfig>,
    /// upstream retry policy for transient failures (408/429/5xx, connect errors)
    #[serde(default)]
    pub retry: RetryConfig,
    /// per-target cooldown applied after a transient upstream failure
    #[serde(default)]
    pub cooldown: CooldownConfig,
    /// exact-match response cache (Redis) shared across replicas; off by default
    #[serde(default)]
    pub cache: CacheConfig,
    /// tenant-scoped routing records for OpenAI Responses lifecycle operations
    #[serde(default)]
    pub responses: ResponsesConfig,
    /// upstream connect/response timeouts
    #[serde(default)]
    pub timeouts: TimeoutConfig,
    /// cross-dialect translation behavior (anthropic version, default max tokens)
    #[serde(default)]
    pub compatibility: CompatibilityConfig,
    /// header handling on the upstream leg and the base URL advertised to clients
    #[serde(default)]
    pub client: ClientConfig,
    /// deployment-wide ingress policy owned by the control plane's Security
    /// screen (#1162). A file-configured gateway may set the same two keys
    /// under `[security]` (`required_headers`, `auth_bypass_routes`) and both
    /// are enforced; absent, the policy is empty and adds no rules
    #[serde(default)]
    pub security: SecurityPolicyConfig,
    /// inference parameters filled in when a client omits them
    #[serde(default)]
    pub model_defaults: ModelDefaultsConfig,
    /// deployment-wide adaptive-routing policy for `adaptive` routes
    #[serde(default)]
    pub adaptive_routing: AdaptiveRoutingConfig,
    /// bounded per-provider dispatch queues and overload behaviour
    #[serde(default)]
    pub queue: QueueConfig,
    /// active upstream health probing
    #[serde(default)]
    pub health: HealthConfig,
    /// per-target circuit breaker for sustained upstream failures
    #[serde(default)]
    pub breaker: BreakerConfig,
    /// background scrape of upstream engine `/metrics` for load-aware routing
    #[serde(default)]
    pub metrics_scrape: MetricsScrapeConfig,
    /// guardrails for persistent OpenAI Realtime WebSocket sessions
    #[serde(default)]
    pub realtime: RealtimeConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    /// bounded queue for post-response budget and rate-limit recording
    #[serde(default)]
    pub usage_recording: UsageRecordingConfig,
    /// deployment-wide gates managed by the control plane
    #[serde(default)]
    pub feature_flags: FeatureFlagsConfig,
    /// built-in regex guardrails and PII redaction, evaluated in the gateway with
    /// no external service; disabled by default (ROL-261)
    #[serde(default)]
    pub guardrails: crate::guardrails::GuardrailsConfig,
    /// where the gateway may send upstream traffic; denies link-local (cloud
    /// metadata) by default so a crafted provider `api_base` can't turn the
    /// proxy into an SSRF primitive
    #[serde(default)]
    pub egress: EgressPolicy,
    /// custom guardrail webhook: a self-hosted HTTP service consulted before
    /// proxying, vendor-neutral. Disabled by default (ROL-257)
    #[serde(default)]
    pub guardrail_webhook: crate::guardrail_webhook::GuardrailWebhookConfig,
    /// semantic guardrail providers that can run side by side, and the
    /// policies scoped to the deployment, an org, a project, a route or a
    /// virtual key that pick which of them judge a request (#459). Empty by
    /// default
    #[serde(default)]
    pub semantic_guards: crate::semantic_guards::SemanticGuardsConfig,
    /// external PII sanitizer: a self-hosted service that returns *transformed*
    /// content (placeholders) rather than an allow/block decision, with
    /// opt-in restoration. Disabled by default (#848)
    #[serde(default)]
    pub pii_sanitizer: crate::pii_sanitizer::PiiSanitizerConfig,
    /// centrally-managed, versioned prompt templates and route decorators,
    /// rendered at admission with structural variable escaping. Disabled by
    /// default (ROL-256)
    #[serde(default)]
    pub prompt_templates: crate::prompt_templates::PromptTemplatesConfig,
    /// enabled webhook plugin instances (control-plane registry: #567), for
    /// the gateway's pre_route/pre_upstream/post_response dispatch runtime
    /// (#509). Empty by default
    #[serde(default)]
    pub plugins: crate::plugin_dispatch::PluginsConfig,
}

/// Deployment-wide gates for runtime subsystems.
///
/// These are separate from subsystem tuning so the control plane can disable
/// configured behavior without discarding its parameters.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FeatureFlagsConfig {
    #[serde(default = "default_true")]
    pub response_cache: bool,
    #[serde(default = "default_true")]
    pub cache_aware_routing: bool,
    #[serde(default = "default_true")]
    pub circuit_breaker: bool,
    #[serde(default = "default_true")]
    pub active_health_checks: bool,
    #[serde(default = "default_true")]
    pub complexity_routing: bool,
    #[serde(default = "default_true")]
    pub guardrails: bool,
}

impl Default for FeatureFlagsConfig {
    fn default() -> Self {
        Self {
            response_cache: true,
            cache_aware_routing: true,
            circuit_breaker: true,
            active_health_checks: true,
            complexity_routing: true,
            guardrails: true,
        }
    }
}

impl GatewayConfig {
    /// Apply control-plane feature gates to the effective snapshot.
    pub fn apply_feature_flags(&mut self) {
        self.cache.enabled = self.feature_flags.response_cache;
        self.breaker.enabled = self.feature_flags.circuit_breaker;
        self.health.enabled = self.feature_flags.active_health_checks;
        self.guardrails.enabled = self.feature_flags.guardrails;
        for route in &mut self.routes {
            if !self.feature_flags.cache_aware_routing
                && matches!(
                    route.strategy,
                    BalancingStrategy::CacheAware
                        | BalancingStrategy::PreciseCacheAware
                        | BalancingStrategy::LmcacheAware
                )
            {
                route.strategy = BalancingStrategy::PowerOfTwo;
            }
            if !self.feature_flags.complexity_routing {
                route.params.remove(COMPLEXITY_POLICY_PARAM);
            }
        }
    }
}

impl ProviderKind {
    /// whether this provider kind must use `api_key_env` and avoid inline keys
    pub fn requires_env_api_key(self) -> bool {
        matches!(
            self,
            ProviderKind::OllamaCloud
                | ProviderKind::Openrouter
                | ProviderKind::Gemini
                | ProviderKind::GeminiNative
                | ProviderKind::GeminiInteractions
                | ProviderKind::Mistral
                | ProviderKind::Groq
                | ProviderKind::Xai
                | ProviderKind::MetaLlamaApi
                | ProviderKind::Cohere
                | ProviderKind::Perplexity
                | ProviderKind::Together
                | ProviderKind::Fireworks
                | ProviderKind::Databricks
                | ProviderKind::AlephAlpha
                | ProviderKind::Nebius
                | ProviderKind::Ovhcloud
                | ProviderKind::Scaleway
                | ProviderKind::Deepseek
                | ProviderKind::Qwen
                | ProviderKind::Zhipu
                | ProviderKind::Kimi
                | ProviderKind::Ernie
                | ProviderKind::Doubao
                | ProviderKind::Hunyuan
                | ProviderKind::Yi
                | ProviderKind::Minimax
                | ProviderKind::Baichuan
                | ProviderKind::Gigachat
                | ProviderKind::YandexGpt
                | ProviderKind::CloudRu
                | ProviderKind::MtsAi
                | ProviderKind::Naver
                | ProviderKind::Upstage
                | ProviderKind::Rinna
                | ProviderKind::Rakuten
                | ProviderKind::Sarvam
                | ProviderKind::Krutrim
                | ProviderKind::Falcon
        )
    }

    /// whether `provider_url` should remove the gateway `/v1` prefix
    pub fn strips_gateway_v1_prefix(self) -> bool {
        matches!(
            self,
            ProviderKind::Openrouter
                | ProviderKind::AzureOpenai
                | ProviderKind::Bedrock
                | ProviderKind::Vertex
                | ProviderKind::Gemini
                | ProviderKind::Mistral
                | ProviderKind::Groq
                | ProviderKind::Xai
                | ProviderKind::MetaLlamaApi
                | ProviderKind::Cohere
                | ProviderKind::Perplexity
                | ProviderKind::Together
                | ProviderKind::Fireworks
                | ProviderKind::Databricks
                | ProviderKind::AlephAlpha
                | ProviderKind::Nebius
                | ProviderKind::Ovhcloud
                | ProviderKind::Scaleway
                | ProviderKind::Deepseek
                | ProviderKind::Qwen
                | ProviderKind::Zhipu
                | ProviderKind::Kimi
                | ProviderKind::Ernie
                | ProviderKind::Doubao
                | ProviderKind::Hunyuan
                | ProviderKind::Yi
                | ProviderKind::Minimax
                | ProviderKind::Baichuan
                | ProviderKind::Gigachat
                | ProviderKind::YandexGpt
                | ProviderKind::CloudRu
                | ProviderKind::MtsAi
                | ProviderKind::Naver
                | ProviderKind::Upstage
                | ProviderKind::Rinna
                | ProviderKind::Rakuten
                | ProviderKind::Sarvam
                | ProviderKind::Krutrim
                | ProviderKind::Falcon
                // infinity serves `{url_prefix}/embeddings` and
                // `{url_prefix}/rerank`, and its default prefix is empty, so
                // the base is the server address plus any `--url-prefix`
                | ProviderKind::Infinity
        )
    }

    /// Every provider kind, so callers can enumerate rather than re-list them.
    ///
    /// `kind_is_listed_in_all` keeps this honest: it matches exhaustively, so
    /// adding a variant without adding it here fails to compile.
    /// [`ProviderKind::Unknown`] is not listed: it is never configurable.
    pub const ALL: [ProviderKind; 50] = [
        ProviderKind::Openai,
        ProviderKind::Anthropic,
        ProviderKind::OpenaiCompatible,
        ProviderKind::Ollama,
        ProviderKind::OllamaCloud,
        ProviderKind::LlamaCpp,
        ProviderKind::Openrouter,
        ProviderKind::Tei,
        ProviderKind::AzureOpenai,
        ProviderKind::Bedrock,
        ProviderKind::Vertex,
        ProviderKind::Gemini,
        ProviderKind::GeminiNative,
        ProviderKind::GeminiInteractions,
        ProviderKind::Mistral,
        ProviderKind::Groq,
        ProviderKind::Xai,
        ProviderKind::MetaLlamaApi,
        ProviderKind::Cohere,
        ProviderKind::Perplexity,
        ProviderKind::Together,
        ProviderKind::Fireworks,
        ProviderKind::Databricks,
        ProviderKind::AlephAlpha,
        ProviderKind::Nebius,
        ProviderKind::Ovhcloud,
        ProviderKind::Scaleway,
        ProviderKind::Deepseek,
        ProviderKind::Qwen,
        ProviderKind::Zhipu,
        ProviderKind::Kimi,
        ProviderKind::Ernie,
        ProviderKind::Doubao,
        ProviderKind::Hunyuan,
        ProviderKind::Yi,
        ProviderKind::Minimax,
        ProviderKind::Baichuan,
        ProviderKind::Gigachat,
        ProviderKind::YandexGpt,
        ProviderKind::CloudRu,
        ProviderKind::MtsAi,
        ProviderKind::Naver,
        ProviderKind::Upstage,
        ProviderKind::Rinna,
        ProviderKind::Rakuten,
        ProviderKind::Sarvam,
        ProviderKind::Krutrim,
        ProviderKind::Falcon,
        ProviderKind::SystemOne,
        ProviderKind::Infinity,
    ];

    /// Whether a configured `api_base` is expected to already carry the API
    /// version prefix (`.../v1`).
    ///
    /// This is the same fact as [`Self::strips_gateway_v1_prefix`] seen from
    /// the operator's side rather than the router's. When the gateway strips
    /// the `/v1` it was asked for, the base has to supply one; when it does
    /// not, the base must not, or the two concatenate into `/v1/v1/...`.
    ///
    /// It is named separately because it is what the dashboard needs to tell
    /// an operator, and the dashboard was previously telling everyone to
    /// include `/v1` — the wrong answer for the default kind (#947).
    pub fn base_includes_v1(self) -> bool {
        self.strips_gateway_v1_prefix()
    }

    /// The upstream URL a gateway request for `path` resolves to.
    ///
    /// The one place this rule is written down. `rolter_proxy::provider_url`
    /// delegates here, and the dashboard previews the result as the operator
    /// types, so what is shown before saving is what will actually be called.
    pub fn resolve_upstream_url(self, api_base: &str, path: &str) -> String {
        let base = api_base.trim_end_matches('/');
        if self.base_includes_v1() {
            let suffix = path.strip_prefix("/v1").unwrap_or(path);
            format!("{base}{suffix}")
        } else {
            format!("{base}{path}")
        }
    }

    /// The header that carries this kind's credential, and the value to put in it.
    ///
    /// The one place the scheme is written down, for the same reason as
    /// [`Self::resolve_upstream_url`]. `rolter_proxy` authenticates every
    /// forwarded request through it, and the gateway's health sweep and the
    /// control plane's "Test connection" authenticate their probes through it
    /// too, so a probe can never be rejected for presenting the key in a way the
    /// proxy would not (#2806: the connection test sent Anthropic's key as a
    /// bearer token, which that API does not read, and reported a valid key as
    /// rejected).
    ///
    /// The name is lowercase. Only the bearer form allocates; the raw schemes
    /// borrow the key, which matters on the request path.
    pub fn auth_header(self, key: &str) -> (&'static str, std::borrow::Cow<'_, str>) {
        use std::borrow::Cow;
        match self {
            ProviderKind::Anthropic => ("x-api-key", Cow::Borrowed(key)),
            ProviderKind::AzureOpenai => ("api-key", Cow::Borrowed(key)),
            // native gemini authenticates its generativelanguage endpoints with
            // an api-key header, not a bearer token (unlike the openai-compat
            // `gemini` shim)
            ProviderKind::GeminiNative | ProviderKind::GeminiInteractions => {
                ("x-goog-api-key", Cow::Borrowed(key))
            }
            _ => ("authorization", Cow::Owned(format!("Bearer {key}"))),
        }
    }

    /// The `api_base` a hosted kind is pinned to unless its provider sets
    /// `allow_custom_api_base` (ADR-0029); `None` for a kind with no pin.
    pub fn pinned_api_base(self) -> Option<&'static str> {
        match self {
            ProviderKind::Openrouter => Some("https://openrouter.ai/api/v1"),
            _ => None,
        }
    }

    /// Why `api_base` breaks this kind's host pin, or `None` when it is the
    /// pinned base, the kind has no pin, or the provider opted out.
    pub fn host_pin_problem(self, api_base: &str, allow_custom_api_base: bool) -> Option<String> {
        let pinned = self.pinned_api_base()?;
        if allow_custom_api_base || api_base.trim().trim_end_matches('/') == pinned {
            return None;
        }
        Some(format!(
            "api_base must be {pinned} unless the provider sets allow_custom_api_base = true"
        ))
    }

    /// The doubled-prefix defect, or `None` when the base is well-formed.
    ///
    /// Only reported for kinds that append `/v1` themselves: for the rest a
    /// trailing `/v1` is required, not a mistake.
    pub fn api_base_problem(self, api_base: &str) -> Option<String> {
        let base = api_base.trim().trim_end_matches('/');
        if base.is_empty() || self.base_includes_v1() || !base.ends_with("/v1") {
            return None;
        }
        Some(format!(
            "this base ends in /v1, and rolter appends /v1 itself for this provider kind, \
             so requests would go to {}. Remove the trailing /v1",
            self.resolve_upstream_url(api_base, "/v1/chat/completions")
        ))
    }
}

/// Two-tier bootstrap model presets for database-backed control planes.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModelTiersConfig {
    /// immutable routes served directly from config and rejected by CRUD
    #[serde(default)]
    pub readonly: Vec<ModelRoute>,
    /// idempotent seed routes: inserted into the default DB project only when
    /// absent, then owned and editable by the database
    #[serde(default, rename = "default")]
    pub defaults: Vec<ModelRoute>,
}

/// Split a config section value into `(readonly, default)` tiers (ADR-0022). A
/// section accepts either the deprecated flat array (all entries treated as
/// `readonly`) or the tiered table `{ readonly = [...], default = [...] }`, so a
/// single TOML key supports both `[[providers]]` and `[providers]
/// readonly/default`. Used for providers and provider groups.
fn split_section<T: serde::de::DeserializeOwned>(v: toml::Value) -> Result<(Vec<T>, Vec<T>)> {
    match v {
        toml::Value::Array(_) => Ok((v.try_into()?, Vec::new())),
        toml::Value::Table(mut table) => {
            let readonly = match table.remove("readonly") {
                Some(r) => r.try_into()?,
                None => Vec::new(),
            };
            let defaults = match table.remove("default") {
                Some(d) => d.try_into()?,
                None => Vec::new(),
            };
            Ok((readonly, defaults))
        }
        _ => Err(crate::Error::Config(
            "expected an array or a { readonly, default } table".to_string(),
        )),
    }
}

/// Outbound TLS trust configuration.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TlsConfig {
    /// PEM files whose certificates are added to the platform trust roots
    #[serde(default)]
    pub ca_bundles: Vec<PathBuf>,
}

/// Listener configuration for a rolter process.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// deployment-wide secret mixed into virtual-key digests. keeps plaintext
    /// keys out of gateway memory and makes a leaked digest useless without it.
    /// falls back to the `ROLTER_KEY_PEPPER` env var when unset (see
    /// [`ServerConfig::resolve_key_pepper`]).
    #[serde(default)]
    pub key_pepper: Option<String>,
    /// path the prometheus metrics endpoint is served on. configurable so it
    /// doesn't collide with an upstream app or sidecar that already owns
    /// `/metrics` behind the same reverse proxy. defaults to `/metrics`.
    #[serde(default = "default_metrics_path")]
    pub metrics_path: String,
    /// maximum accepted request body size in bytes. raises axum's restrictive
    /// 2 MiB default so large LLM payloads (long context, base64 images/audio)
    /// aren't silently rejected; an oversized body gets an OpenAI-style 413.
    /// defaults to 32 MiB
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,
    /// whether virtual-key auth is enforced when the effective key set is
    /// empty. `Some(true)` always denies (fail closed), `Some(false)` always
    /// allows the keyless path, and `None` defers to the deployment: managed
    /// gateways (those polling a control-plane snapshot) fail closed, while a
    /// static local config stays keyless for `fake-llm` style development.
    /// without this, revoking the last key would silently open the data plane
    #[serde(default)]
    pub require_auth: Option<bool>,
}

impl ServerConfig {
    /// Resolve the key pepper: explicit config wins, else `ROLTER_KEY_PEPPER`,
    /// else empty (keys are still hashed, just without a secret pepper).
    pub fn resolve_key_pepper(&self) -> String {
        self.key_pepper
            .clone()
            .or_else(|| std::env::var("ROLTER_KEY_PEPPER").ok())
            .unwrap_or_default()
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            key_pepper: None,
            metrics_path: default_metrics_path(),
            max_body_bytes: default_max_body_bytes(),
            require_auth: None,
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_max_body_bytes() -> usize {
    32 * 1024 * 1024
}

fn default_port() -> u16 {
    4000
}

fn default_metrics_path() -> String {
    "/metrics".to_string()
}

/// Transports an MCP server may speak, and the one authoritative definition of
/// that set (#783).
///
/// Config validation, the control plane's registry and tool-call-log ingest all
/// read this, and the `mcp_servers.transport` /
/// `mcp_gateway_settings.default_transport` check constraints match it — pinned
/// by a test in `rolter-control`. `stdio` is deliberately absent: it names a
/// local subprocess, which a hosted control plane cannot dial and the gateway's
/// HTTP proxy path has never served.
///
/// `websocket` is accepted although the gateway has no proxy path for it: it
/// stays valid for registry rows already stored under the check constraints and
/// for external producers posting tool-call events, so dropping it would orphan
/// stored data (#2432). The `/mcp/{server}` proxy rejects it with
/// `mcp_transport_unsupported`, so the gateway itself never writes such a row.
pub const MCP_TRANSPORTS: &[&str] = &["sse", "streamable_http", "websocket"];

/// Gateway request paths reserved by the built-in routes; the metrics path must
/// not collide with any of these.
pub const RESERVED_PATHS: &[&str] = &[
    "/",
    "/healthz",
    "/openapi.json",
    "/docs",
    "/docs/scalar.js",
    "/v1/models",
    "/v1/chat/completions",
    "/v1/completions",
    "/v1/responses",
    "/v1/messages",
    "/v1/embeddings",
    "/v1/rerank",
    "/v1/images/generations",
    "/v1/audio/speech",
    "/v1/audio/transcriptions",
    "/v1/audio/translations",
    "/v1/realtime",
];

/// Limits applied to long-lived Realtime WebSocket sessions.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RealtimeConfig {
    /// maximum concurrent Realtime sessions across this gateway instance; 0 disables the cap
    #[serde(default = "default_realtime_max_connections")]
    pub max_connections: u64,
    /// maximum session age in seconds; 0 disables the limit
    #[serde(default = "default_realtime_max_session_secs")]
    pub max_session_secs: u64,
    /// close an inactive session after this many seconds; 0 disables the limit
    #[serde(default = "default_realtime_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// how often a live session's completed response turns are written to the
    /// request log, added to budgets and `tpm` windows, and followed by a
    /// budget re-check. 0 flushes after every completed turn instead of on a
    /// timer, and re-checks the budgets every second while the session is
    /// quiet; metering itself cannot be switched off (#1396)
    #[serde(default = "default_realtime_usage_flush_secs")]
    pub usage_flush_secs: u64,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            max_connections: default_realtime_max_connections(),
            max_session_secs: default_realtime_max_session_secs(),
            idle_timeout_secs: default_realtime_idle_timeout_secs(),
            usage_flush_secs: default_realtime_usage_flush_secs(),
        }
    }
}

fn default_realtime_max_connections() -> u64 {
    1_000
}

fn default_realtime_max_session_secs() -> u64 {
    3_600
}

fn default_realtime_idle_timeout_secs() -> u64 {
    300
}

fn default_realtime_usage_flush_secs() -> u64 {
    15
}

/// The wire protocol a provider speaks.
///
/// The `Default` is [`ProviderKind::OpenaiCompatible`] — the one variant that
/// names no vendor and carries no host pin, so a defaulted provider cannot be
/// mistaken for a hosted operator. `kind` is a required field everywhere it is
/// deserialized, so nothing on a config or database path can reach the default
/// by omission; it exists for test and bench fixtures built with
/// `..Default::default()`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// native openai chat/completions api
    Openai,
    /// native anthropic messages api
    Anthropic,
    /// any openai-compatible endpoint such as vllm, tgi or ollama
    #[default]
    OpenaiCompatible,
    /// a self-hosted ollama daemon using its openai-compatible api
    Ollama,
    /// direct ollama cloud access using an environment-sourced bearer key
    OllamaCloud,
    /// a self-hosted llama.cpp llama-server using its openai-compatible api
    LlamaCpp,
    /// openrouter's hosted openai-compatible api
    Openrouter,
    /// hugging face text embeddings inference openai-compatible api
    Tei,
    /// azure openai's openai-compatible v1 api
    AzureOpenai,
    /// amazon bedrock's openai-compatible chat completions api
    Bedrock,
    /// vertex ai's openai-compatible chat completions api
    Vertex,
    /// google gemini's openai-compatible api (`/v1beta/openai`)
    Gemini,
    /// google gemini's native generateContent api (`/v1beta/models/{model}:generateContent`),
    /// reached by translating openai/anthropic requests to the gemini wire format
    GeminiNative,
    /// google gemini's interactions api (`/v1beta/interactions`), represented as
    /// a dedicated provider kind for stateful conversation threading
    GeminiInteractions,
    /// mistral's hosted openai-compatible api
    Mistral,
    /// groq's hosted openai-compatible api (`/openai/v1`)
    Groq,
    /// xai's hosted openai-compatible api for grok models (`/v1`)
    Xai,
    /// meta llama api's hosted openai-compatible api
    MetaLlamaApi,
    /// cohere's hosted openai-compatible api
    Cohere,
    /// perplexity's hosted openai-compatible api
    Perplexity,
    /// together ai's hosted openai-compatible api
    Together,
    /// fireworks ai's hosted openai-compatible api
    Fireworks,
    /// databricks foundation model apis
    Databricks,
    /// aleph alpha's hosted openai-compatible api
    AlephAlpha,
    /// nebius ai studio's hosted openai-compatible api
    Nebius,
    /// ovhcloud ai endpoints openai-compatible api
    Ovhcloud,
    /// scaleway generative apis openai-compatible api
    Scaleway,
    /// deepseek's hosted openai-compatible api
    Deepseek,
    /// alibaba qwen dashscope openai-compatible api
    Qwen,
    /// zhipu glm openai-compatible api
    Zhipu,
    /// moonshot kimi openai-compatible api
    Kimi,
    /// baidu ernie qianfan openai-compatible api
    Ernie,
    /// bytedance doubao volcengine openai-compatible api
    Doubao,
    /// tencent hunyuan openai-compatible api
    Hunyuan,
    /// 01.ai yi openai-compatible api
    Yi,
    /// minimax hosted openai-compatible api
    Minimax,
    /// baichuan ai hosted openai-compatible api
    Baichuan,
    /// sber gigachat hosted openai-compatible api
    Gigachat,
    /// yandex gpt studio hosted openai-compatible api
    YandexGpt,
    /// cloud.ru foundation models hosted openai-compatible api
    CloudRu,
    /// mts ai cotype hosted openai-compatible api
    MtsAi,
    /// naver hyperclova x hosted openai-compatible api
    Naver,
    /// upstage solar hosted openai-compatible api
    Upstage,
    /// rinna hosted openai-compatible api
    Rinna,
    /// rakuten ai hosted openai-compatible api
    Rakuten,
    /// sarvam ai hosted openai-compatible api
    Sarvam,
    /// krutrim ola hosted openai-compatible api
    Krutrim,
    /// g42/tii falcon hosted openai-compatible api
    Falcon,
    /// a System One decision endpoint (`POST /v1/systemone`): typed answers to
    /// choice, score and yes/no questions instead of generated text. TypeSafe's
    /// Jev, vLLM Semantic Router's decision models and Ollama's all speak it
    SystemOne,
    /// a self-hosted michaelfeil/infinity server: openai-shaped embeddings and
    /// a cohere-style rerank, many models from one process, keyless by default
    Infinity,
    /// a kind this build does not know, read from a newer control plane's
    /// snapshot (#3347). Such a provider is never dialled: the snapshot
    /// sanitizer omits it, and the routes, groups and keys that named it lose
    /// those references through the same cascade as any other omitted provider.
    /// A config file or an API request that names an unknown kind is refused
    #[serde(other)]
    Unknown,
}

/// Instruction-role semantics supported by an upstream target.
///
/// Endpoint compatibility alone is not enough to determine this: OpenAI-style
/// servers may render roles through a model-specific chat template.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleProfile {
    /// Preserve distinct OpenAI `developer` and `system` messages.
    Openai,
    /// Accept system instructions only; `developer` is lowered to `system`.
    #[default]
    SystemOnly,
    /// Anthropic Messages top-level `system` blocks.
    Anthropic,
    /// a profile this build does not know (#3347); read as
    /// [`RoleProfile::SystemOnly`], the one every upstream accepts
    #[serde(other)]
    Unknown,
}

/// An upstream provider rolter can forward to.
///
/// `Default` yields an unnamed, unaddressed openai-compatible target: every
/// field is empty, absent or false. It is there so a fixture can name the two
/// or three fields a test actually cares about and write `..Default::default()`
/// for the rest — adding a field to this struct then stays a no-op for every
/// test in the workspace instead of a compile error in all of them (#1150).
/// Production code that maps a stored row or a parsed config into a
/// `ProviderConfig` still writes the literal out in full, so the compiler keeps
/// forcing a decision about each new field on the paths where one is owed.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub name: String,
    /// stable, URL-safe identity for `provider-slug/model` addressing. When
    /// absent (e.g. a bootstrap TOML that predates slugs) callers derive one
    /// from `name` via [`crate::slug::slugify`]; the postgres store always
    /// populates it from the `providers.slug` column
    #[serde(default)]
    pub slug: Option<String>,
    pub kind: ProviderKind,
    /// base url without a trailing slash, e.g. `https://api.openai.com`
    pub api_base: String,
    /// inline api key; prefer `api_key_env` so secrets stay out of config files
    #[serde(default)]
    pub api_key: Option<String>,
    /// name of an environment variable to read the api key from
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// optional outbound egress proxy url (http/https/socks5)
    #[serde(default)]
    pub egress_proxy: Option<String>,
    /// outbound egress proxy pool. Entries rotate round-robin; a legacy
    /// `egress_proxy` is prepended as a one-element pool when this is empty.
    /// Use `${ENV_VAR}` for authenticated proxy URLs so credentials never
    /// appear in serialized configuration or database rows.
    #[serde(default)]
    pub egress_proxies: Vec<String>,
    /// optional vLLM ZMQ KV-event subscriber settings for this target
    #[serde(default)]
    pub kv_events: Option<KvEventsConfig>,
    /// optional LMCache controller signal for this target
    #[serde(default)]
    pub lmcache: Option<LmCacheConfig>,
    /// provider-specific PEM CA bundles; when set, replaces `[tls].ca_bundles`
    #[serde(default)]
    pub ca_bundles: Option<Vec<PathBuf>>,
    /// multiple weighted api keys for this provider; when non-empty it takes
    /// precedence over the single `api_key`/`api_key_env` pair. Providers cap
    /// throughput per key, so rotating across keys multiplies effective RPM/TPM
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,
    /// when true, active health checks send a minimal real completion
    /// (`max_tokens = 1`) to this provider instead of the free liveness probe,
    /// so a healthy result proves end-to-end inference works — not merely that
    /// the API answers. Off by default: every sweep burns a few tokens. Requires
    /// `llm_probe_model` to name a model to call
    #[serde(default)]
    pub also_track_via_llm_call: bool,
    /// model name to use for the `also_track_via_llm_call` completion (the
    /// upstream model id, e.g. `gpt-4o-mini`). Ignored unless the flag is set
    #[serde(default)]
    pub llm_probe_model: Option<String>,
    /// optional statuspage.io-style status JSON url (e.g.
    /// `https://status.anthropic.com/api/v2/status.json`). When set, a slow
    /// background poll records the provider's public status as a secondary
    /// `status_page` health signal; it never gates routing on its own
    #[serde(default)]
    pub status_page_url: Option<String>,
    /// Explicit role semantics for this provider. When omitted, native OpenAI
    /// preserves both instruction roles, Anthropic uses top-level system blocks,
    /// and OpenAI-compatible providers use the conservative `system_only` mode.
    #[serde(default)]
    pub role_profile: Option<RoleProfile>,
    /// Per-upstream-model role-profile overrides. Use this for custom chat
    /// templates that explicitly support `developer`.
    #[serde(default)]
    pub model_role_profiles: HashMap<String, RoleProfile>,
    /// Opt out of the host pin a hosted provider kind carries by default, so
    /// this provider may point at any `api_base`.
    ///
    /// The pin exists because a hosted kind names one operator: an
    /// `openrouter` provider whose `api_base` could be edited would send an
    /// OpenRouter key to whatever host the edit named. Setting this makes that
    /// redirection a deliberate, reviewable line in the configuration rather
    /// than something a config edit can do silently, and `rolter check` warns
    /// on every provider that sets it. The `api_key_env` rule is not relaxed:
    /// the credential still cannot be inlined in the config or the database.
    ///
    /// Set it for an egress proxy in front of the vendor, an
    /// endpoint that re-implements the vendor's surface, or a local fake used
    /// to exercise the dialect. See ADR-0029.
    #[serde(default)]
    pub allow_custom_api_base: bool,
    /// the org the provider belongs to in the store; absent for a provider
    /// from the gateway's own config file (see [`Tenancy`])
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenancy: Option<Tenancy>,
    /// scope the provider to one project: only keys minted in that project may
    /// reach it, through a route or through `provider-slug/model`. Unset (the
    /// default) keeps it org-wide, usable by every project of the org.
    ///
    /// In a file this means "the project the file is imported into" (see
    /// `rolter-seed --import`); a file has no project names of its own. The
    /// store is authoritative at runtime, where the scope is
    /// `tenancy.project_id`, so a gateway reading a file directly ignores the
    /// flag: its keys carry no project to compare against.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_scoped: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KvEventsConfig {
    /// subscriber endpoint, e.g. `tcp://vllm-0:5557`
    pub endpoint: String,
    /// ZMQ topic; vLLM examples use `kv-events`
    #[serde(default = "default_kv_events_topic")]
    pub topic: String,
    /// maximum resident block identities kept for this provider
    #[serde(default = "default_kv_events_max_blocks")]
    pub max_blocks: usize,
    /// telemetry older than this is ignored
    #[serde(default = "default_kv_events_stale_secs")]
    pub stale_secs: u64,
}

impl KvEventsConfig {
    /// Problems with the settings themselves, apart from whether the egress
    /// policy allows the endpoint. Shared by the file config and the control
    /// API so a stored provider is held to the same rules as a file one.
    pub fn shape_problems(&self, provider: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if !self.endpoint.starts_with("tcp://") || self.endpoint.len() <= "tcp://".len() {
            problems.push(format!(
                "provider '{provider}' kv_events.endpoint must be a non-empty tcp:// URL"
            ));
        }
        if self.max_blocks == 0 || self.stale_secs == 0 {
            problems.push(format!(
                "provider '{provider}' kv_events max_blocks and stale_secs must be greater than zero"
            ));
        }
        problems
    }
}

impl LmCacheConfig {
    /// Problems with the settings themselves, apart from the egress policy;
    /// see [`KvEventsConfig::shape_problems`].
    pub fn shape_problems(&self, provider: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if !is_http_url(&self.endpoint) {
            problems.push(format!(
                "provider '{provider}' lmcache.endpoint must be an http(s) URL"
            ));
        }
        if self.refresh_secs == 0 || self.stale_secs == 0 {
            problems.push(format!(
                "provider '{provider}' lmcache refresh_secs and stale_secs must be greater than zero"
            ));
        }
        problems
    }
}

fn default_kv_events_topic() -> String {
    "kv-events".to_string()
}

fn default_kv_events_max_blocks() -> usize {
    1_000_000
}

fn default_kv_events_stale_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LmCacheConfig {
    /// HTTP endpoint returning `{ "occupancy": 0..1, "cache_available": bool }`
    pub endpoint: String,
    #[serde(default = "default_lmcache_refresh_secs")]
    pub refresh_secs: u64,
    #[serde(default = "default_lmcache_stale_secs")]
    pub stale_secs: u64,
}

fn default_lmcache_refresh_secs() -> u64 {
    2
}

fn default_lmcache_stale_secs() -> u64 {
    10
}

/// One of a provider's weighted API keys. Same inline-vs-env split as the
/// provider-level pair: prefer `env` so secrets stay out of config files.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiKeyConfig {
    /// inline key value; prefer `env`
    #[serde(default)]
    pub key: Option<String>,
    /// name of an environment variable to read the key from
    #[serde(default)]
    pub env: Option<String>,
    /// relative selection weight; 0 is never picked while another key has a positive weight
    #[serde(default = "default_weight")]
    pub weight: u32,
}

/// A defaulted entry carries no secret and the neutral weight the TOML
/// `#[serde(default)]` would apply, so a fixture can name only the field it
/// cares about. Derived `Default` would give `weight = 0`, which now means
/// "never picked while another key has a positive weight".
impl Default for ApiKeyConfig {
    fn default() -> Self {
        Self {
            key: None,
            env: None,
            weight: default_weight(),
        }
    }
}

impl ApiKeyConfig {
    /// Resolve this entry's key, preferring the inline value then the env var.
    pub fn resolve(&self) -> Option<String> {
        if let Some(k) = &self.key {
            return Some(k.clone());
        }
        self.env
            .as_ref()
            .and_then(|e| crate::secret_env::credential_from_env(e, &[]))
    }
}

impl ProviderConfig {
    /// Configured proxy references in rotation order. The legacy singular
    /// field remains a backwards-compatible one-element pool.
    pub fn egress_proxy_pool(&self) -> Vec<&str> {
        if self.egress_proxies.is_empty() {
            self.egress_proxy.iter().map(String::as_str).collect()
        } else {
            self.egress_proxies.iter().map(String::as_str).collect()
        }
    }

    /// Resolve the configured role semantics for an upstream model.
    pub fn role_profile_for(&self, model: Option<&str>) -> RoleProfile {
        model
            .and_then(|model| self.model_role_profiles.get(model).copied())
            .or(self.role_profile)
            .unwrap_or(match self.kind {
                ProviderKind::Openai => RoleProfile::Openai,
                ProviderKind::Anthropic => RoleProfile::Anthropic,
                _ => RoleProfile::SystemOnly,
            })
    }

    /// Resolve the effective api key. With `api_keys` configured this is the
    /// first resolvable entry; otherwise the legacy single `api_key`/`api_key_env`
    /// pair. Callers that balance across keys use [`Self::resolve_api_keys`].
    pub fn resolve_api_key(&self) -> Option<String> {
        if !self.api_keys.is_empty() {
            return self.api_keys.iter().find_map(|k| k.resolve());
        }
        if let Some(k) = &self.api_key {
            return Some(k.clone());
        }
        self.api_key_env
            .as_ref()
            .and_then(|e| crate::secret_env::credential_from_env(e, &[]))
    }

    /// Resolve every configured key with its weight. A legacy single-key
    /// provider yields a one-element list with weight 1, so callers can treat
    /// every provider uniformly. Entries whose secret cannot be resolved
    /// (missing env var) are skipped.
    pub fn resolve_api_keys(&self) -> Vec<(String, u32)> {
        if self.api_keys.is_empty() {
            return self
                .resolve_api_key()
                .map(|k| vec![(k, 1)])
                .unwrap_or_default();
        }
        self.api_keys
            .iter()
            .filter_map(|k| k.resolve().map(|s| (s, k.weight)))
            .collect()
    }

    /// Name of the environment variable this provider's key is read from, in
    /// whichever spelling the document used: the first `api_keys` entry that
    /// names an `env`, else the legacy `api_key_env`.
    ///
    /// Persistence paths use this rather than the field. The control-plane
    /// store holds one plaintext variable name per provider, and reading
    /// `api_key_env` directly made a plural-form provider look credential-less,
    /// so `rolter-seed --import` dropped its key on the floor (#1514).
    ///
    /// This deliberately never falls back to an inline `key`: the value it
    /// returns is written to a database column and rendered into an exported
    /// document, and neither may ever carry a secret.
    pub fn api_key_env_name(&self) -> Option<&str> {
        if !self.api_keys.is_empty() {
            return self.api_keys.iter().find_map(|k| k.env.as_deref());
        }
        self.api_key_env.as_deref()
    }

    /// The inline secret literal this provider carries, in whichever spelling:
    /// the first `api_keys` entry with a `key`, else the legacy `api_key`.
    ///
    /// This is a secret. It exists for the one caller that seals a credential
    /// into `provider_keys` at rest; nothing may render it into a document.
    pub fn inline_api_key(&self) -> Option<&str> {
        if !self.api_keys.is_empty() {
            return self.api_keys.iter().find_map(|k| k.key.as_deref());
        }
        self.api_key.as_deref()
    }

    /// Whether this provider carries a credential that is not one environment
    /// variable: an inline secret in either spelling, or more than one key.
    ///
    /// The kinds that [`ProviderKind::requires_env_api_key`] are refused when
    /// this is true. It reads both spellings because a document migrated to the
    /// plural form (#1720) holds its one variable as an `api_keys` entry, which
    /// must keep passing the rule the singular `api_key_env` always passed.
    pub fn has_non_env_credential(&self) -> bool {
        self.api_key.is_some()
            || self.api_keys.len() > 1
            || self.api_keys.iter().any(|k| k.key.is_some())
    }

    /// How many configured keys a single-credential store cannot hold.
    ///
    /// `providers` carries one `api_key_env` and `provider_keys` one sealed
    /// ciphertext per provider, so a multi-key provider loses everything past
    /// the first entry on import. Callers warn rather than fail: dropping the
    /// surplus silently is what made #1514 hard to see, and refusing the whole
    /// import would take a working deployment down over a weight.
    pub fn surplus_api_key_count(&self) -> usize {
        self.api_keys.len().saturating_sub(1)
    }

    /// Pick one resolved api key by weight, given a random draw `r` in
    /// `[0.0, 1.0)`. A single-key provider always yields that key; `None` only
    /// when no key resolves at all.
    pub fn pick_api_key(&self, r: f64) -> Option<String> {
        self.pick_api_key_indexed(r, |_| false).map(|(_, k)| k)
    }

    /// Weighted pick that skips keys whose index is `blocked` (e.g. parked on a
    /// cooldown) and reports the chosen index for attribution. When every key
    /// is blocked the pick fails open to the full pool, so requests still flow.
    pub fn pick_api_key_indexed(
        &self,
        r: f64,
        blocked: impl Fn(usize) -> bool,
    ) -> Option<(usize, String)> {
        let mut keys = self.resolve_api_keys();
        if keys.is_empty() {
            return None;
        }
        if keys.len() == 1 {
            // `pop` rather than `remove(0)`: same key, no panicking call for
            // code scanning to read as a log sink fed by the key list (#2911)
            return keys.pop().map(|(key, _)| (0, key));
        }
        let open: Vec<usize> = (0..keys.len()).filter(|i| !blocked(*i)).collect();
        let pool: Vec<usize> = if open.is_empty() {
            (0..keys.len()).collect()
        } else {
            open
        };
        let picked = weighted_index(pool.iter().map(|&i| keys[i].1), r)?;
        let i = pool[picked];
        Some((i, std::mem::take(&mut keys[i].0)))
    }
}

/// Load-balancing strategy applied to a route's targets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BalancingStrategy {
    #[default]
    RoundRobin,
    Random,
    PowerOfTwo,
    ConsistentHash,
    CacheAware,
    /// smooth weighted round-robin honouring each target's `weight`
    Weighted,
    /// composable filter → weighted-score → argmax pipeline (static weight +
    /// in-flight load + prefix-cache affinity scorers)
    Pipeline,
    /// cost-aware: prefer the target with the lowest catalog price for its
    /// upstream model (in-flight load breaks ties)
    Cheapest,
    /// latency-aware: prefer the target with the lowest observed request
    /// latency (in-flight load breaks ties, and carries a cold route alone)
    Fastest,
    /// exact resident-prefix routing from vLLM KV-cache events
    PreciseCacheAware,
    /// LMCache controller occupancy and cache-availability routing
    LmcacheAware,
    /// self-tuning blend of observed latency, catalog cost and in-flight load,
    /// governed by [`AdaptiveRoutingConfig`]; falls back to the `pipeline`
    /// stack whenever it is disabled or the evidence is too thin
    Adaptive,
    /// LoRA-adapter affinity: prefer a target that already holds the requested
    /// adapter resident, with prefix affinity and in-flight load behind it
    LoraAware,
    /// per-request predicted latency: rank targets by what *this* request is
    /// modelled to cost on each, from the queue it would join and its own
    /// prompt size, rather than by a per-target average
    PredictedLatency,
    /// a strategy this build does not know, read from a newer control plane's
    /// snapshot (#3347). The route refuses: the snapshot sanitizer omits it,
    /// so a request for its model is answered as for any unknown model rather
    /// than balanced by a guess
    #[serde(other)]
    Unknown,
}

/// A named set of providers addressable as `group-slug/model` (ADR-0017
/// addendum). A request that resolves to a group **pins the group** and fans
/// out across its members with `strategy`; each member forwards to a provider,
/// optionally rewriting the upstream model (the default is passthrough of the
/// requested model name). The `slug` shares the provider slug namespace, so a
/// left segment resolves to at most one of {provider, group} — providers win a
/// tie deterministically.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProviderGroupConfig {
    /// display name
    pub name: String,
    /// stable, URL-safe identity; when absent it is derived from `name` via
    /// [`crate::slug::slugify`], mirroring [`ProviderConfig::slug`]
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub strategy: BalancingStrategy,
    /// member providers this group fans out across; an empty group never
    /// resolves (there is nothing to route to)
    #[serde(default)]
    pub members: Vec<GroupMember>,
    /// the org the group belongs to in the store; absent for a group from the
    /// gateway's own config file (see [`Tenancy`])
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenancy: Option<Tenancy>,
    /// scope the group to one project, exactly as [`ProviderConfig::project_scoped`]
    /// does for a provider. A project-scoped group may hold that project's
    /// providers and org-wide ones; an org-wide group may hold only org-wide
    /// providers, since a member scoped to a project would otherwise be
    /// reachable by every project through the group.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_scoped: bool,
}

/// One member of a [`ProviderGroupConfig`]: a provider plus optional upstream
/// model rewrite and balancing weight.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupMember {
    /// name of the [`ProviderConfig`] this member forwards to
    pub provider: String,
    /// upstream model id; if absent the requested model name (the right segment
    /// of `group-slug/model`) is forwarded as-is (passthrough)
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

/// A single upstream target within a route.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Target {
    /// name of the [`ProviderConfig`] this target forwards to
    pub provider: String,
    /// upstream model id; if absent the requested model name is forwarded as-is
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// the failures this target is a fallback for (#1584). A target with a set
    /// is never dialled first and never picked by ordinary balancing: it is
    /// tried only after an attempt on another target failed with one of these
    /// classes. Empty, the default, makes it an ordinary member of the pool.
    /// See [`crate::failover`]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes_over: Vec<crate::failover::FailureClass>,
}

impl Default for Target {
    /// An empty provider name with the weight an omitted `weight` key
    /// deserializes to (1, not 0: a zero-weight target is never chosen). It
    /// exists for fixtures; production code writes every field out.
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: None,
            weight: default_weight(),
            takes_over: Vec::new(),
        }
    }
}

impl Target {
    /// Whether this target is a restricted fallback rather than a member of
    /// the pool.
    pub fn is_restricted(&self) -> bool {
        !self.takes_over.is_empty()
    }
}

/// Extra settings attached to one public model route. These are deliberately
/// separate from [`ProviderConfig`]: a provider owns credentials and shared
/// transport defaults, while a model may need a narrower policy.
///
/// Every field here is read by something. `model_type`, `capabilities` and
/// `context_length` are catalog metadata the dashboard reads back and
/// `GET /v1/models` serves (#2939); `description` is dashboard-only; the gateway
/// applies `headers`, `locked_headers`, `limits.timeout_secs`, `limits.retries`,
/// `limits.output_tokens`, `limits.rpm`, `limits.tpm`, `limits.concurrency`,
/// `visibility` and `guardrails`.
// `additional_fields` (#1665) and then `base_url`, `insecure_tls`, `pricing`
// and the `context_window` limit (#2924) used to sit here (the window is back as
// `context_length`, with `GET /v1/models` as its reader, #2939), as did `rpm`, `tpm`
// and `concurrency` until #2936 gave them a reader: stored, validated, put in every snapshot and read by nothing,
// while the docs and the dashboard described them as working. `base_url` and
// `insecure_tls` were the worst of it. a per-route endpoint would have carried
// the provider's credential to whatever host the route named, and the provider
// owns the key. an insecure-tls switch is not something a route should own
// either; a private ca is `ca_bundles`. a stored blob that still carries any of
// these keys keeps deserializing whole, because none of these types deny
// unknown fields, and nothing rolter serializes emits them again. `rolter
// check` reports them in a file as unknown keys
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct AdvancedModelConfig {
    /// provider/model family selected in the catalog (for example `chat` or
    /// `embedding`). The dashboard reads it to offer the parameters and prices
    /// that fit the family, and `GET /v1/models` serves it; the gateway does
    /// not enforce it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_type: Option<String>,
    /// what the model supports (`streaming`, `tools`, ...). A label for the
    /// dashboard and for `GET /v1/models`, not a gate: the gateway forwards a
    /// request whether or not the flag is set
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// size of the model's context window in tokens, served on `GET /v1/models`
    /// as `context_length`. Advertised, not enforced: a request longer than it
    /// is forwarded and the upstream answers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// free-text note kept with the route. Shown in the dashboard and returned
    /// by the routes API; never sent to API clients
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub limits: ModelLimits,
    /// headers sent to the upstream on every call this route makes, for every
    /// target. Credentials belong to the provider: an authentication header
    /// here is refused (see [`AdvancedModelConfig::header_problems`])
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// names from [`Self::headers`] a caller's own header may not replace. A
    /// header the deployment forwards from the caller (`[client]
    /// forwarded_headers`) wins over an unlocked route header of the same name
    /// and loses to a locked one
    #[serde(default)]
    pub locked_headers: Vec<String>,
    #[serde(default)]
    pub visibility: ModelVisibility,
    /// per-route guardrail selection, layered over the global `[guardrails]`
    /// rule set (#590)
    #[serde(default)]
    pub guardrails: crate::guardrails::RouteGuardrails,
}

/// Header names a route may not set: the credential of every provider kind
/// (see [`ProviderKind::auth_header`]) and the headers that frame the request
/// or its connection.
///
/// A route is edited by a project admin and a provider, with the key it holds,
/// by an org admin, so a route that could replace `authorization` could
/// present a credential of its own in place of the provider's. The framing
/// headers are the gateway's to set: a route that set `content-length` could
/// not describe the body it forwards.
pub const RESERVED_ROUTE_HEADERS: &[&str] = &[
    "api-key",
    "authorization",
    "connection",
    "content-length",
    "content-type",
    "expect",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "x-api-key",
    "x-goog-api-key",
];

/// Keys that sat directly in a route's `advanced` block and were read by
/// nothing (#2924); see the note on [`AdvancedModelConfig`].
pub const RETIRED_ADVANCED_KEYS: &[&str] = &["base_url", "insecure_tls", "pricing"];

/// Keys that sat in a route's `advanced.limits` and are read by nothing
/// (#2924). `rpm`, `tpm` and `concurrency` were on this list until #2936.
pub const RETIRED_LIMIT_KEYS: &[&str] = &["context_window"];

/// Remove the retired keys from a route's `advanced` JSON, returning the dotted
/// name of each one that was there.
///
/// The control plane stores the blob it is sent rather than a re-serialization
/// of [`AdvancedModelConfig`], so without this a client still sending
/// `insecure_tls: true` would read it back from the routes API as if it were
/// stored policy. It accepts and drops them instead: a refusal would break an
/// older dashboard's save over a value that never did anything.
pub fn strip_retired_advanced_keys(advanced: &mut serde_json::Value) -> Vec<String> {
    let mut dropped = Vec::new();
    let Some(object) = advanced.as_object_mut() else {
        return dropped;
    };
    for key in RETIRED_ADVANCED_KEYS {
        if object.remove(*key).is_some() {
            dropped.push((*key).to_string());
        }
    }
    if let Some(limits) = object.get_mut("limits").and_then(|l| l.as_object_mut()) {
        for key in RETIRED_LIMIT_KEYS {
            if limits.remove(*key).is_some() {
                dropped.push(format!("limits.{key}"));
            }
        }
    }
    dropped
}

/// Most extra attempts a route may ask for, the same ceiling the control plane
/// puts on `[retry].max_retries` (0 to 10). A stored value above it is applied
/// as this, so a row written before the field was read cannot turn one failing
/// request into a long loop.
pub const MAX_ROUTE_RETRIES: u32 = 10;

impl AdvancedModelConfig {
    /// Every reason a header on this route cannot be sent, one sentence each.
    ///
    /// A header whose name is not a valid token, whose value is not visible
    /// ASCII, or whose name is in [`RESERVED_ROUTE_HEADERS`]. The same rule the
    /// control plane applies on write and the proxy applies when it builds the
    /// route's headers, so the three cannot disagree about what is sendable.
    /// Names in `locked_headers` are checked as names only: locking one that has
    /// no value here is how a route keeps a forwarded caller header out.
    pub fn header_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut names: Vec<&String> = self.headers.keys().collect();
        names.sort();
        for name in names {
            let value = &self.headers[name];
            if !is_header_name(name) {
                problems.push(format!("header name '{name}' is not valid"));
            } else if is_reserved_route_header(name) {
                problems.push(format!(
                    "header '{name}' cannot be set on a route: it carries the provider's \
                     credential or frames the request"
                ));
            } else if !is_header_value(value) {
                problems.push(format!(
                    "header '{name}' has a value that is not visible ASCII on one line"
                ));
            }
        }
        for name in &self.locked_headers {
            if !is_header_name(name) {
                problems.push(format!("locked header name '{name}' is not valid"));
            }
        }
        problems
    }

    /// Drop every header [`Self::header_problems`] names, returning those
    /// sentences. What is left is exactly what the proxy would have sent, so
    /// the route keeps serving without them.
    pub fn drop_unsendable_headers(&mut self) -> Vec<String> {
        let problems = self.header_problems();
        if !problems.is_empty() {
            self.headers
                .retain(|name, value| is_sendable_route_header(name, value));
            self.locked_headers.retain(|name| is_header_name(name));
        }
        problems
    }
}

/// Whether `name` may be sent on a route: a valid token that is not reserved.
fn is_sendable_route_header(name: &str, value: &str) -> bool {
    is_header_name(name) && !is_reserved_route_header(name) && is_header_value(value)
}

/// Whether `name` is in [`RESERVED_ROUTE_HEADERS`], compared without case.
pub fn is_reserved_route_header(name: &str) -> bool {
    RESERVED_ROUTE_HEADERS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

/// RFC 9110 `token`: the characters a header name may be made of.
fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// A header value the wire can carry: visible ASCII, spaces and tabs.
fn is_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
}

/// Per-model request settings. `None` inherits the gateway setting.
///
/// `context_window` used to sit beside these and was read by nothing (#2924);
/// see the note on [`AdvancedModelConfig`].
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelLimits {
    /// bound on the wait for the upstream's response headers, in whole seconds;
    /// replaces `[timeouts].request_secs` for this route. The body of a
    /// streamed response is not cut short by it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
    /// extra attempts after the first, each on another target; replaces
    /// `[retry].max_retries` for this route. `0` turns retries off. Applied up
    /// to [`MAX_ROUTE_RETRIES`]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    /// refuse a request that asks for more output tokens than this
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    /// requests per minute this route admits, across every caller and every
    /// gateway replica that shares the deployment's Redis. Refused with `429`
    /// and `Retry-After`. Without Redis the cap is not enforced (#2936)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,
    /// tokens per minute this route admits, counted from the usage of finished
    /// responses, so a burst of requests in flight can overshoot it by their
    /// own usage. Shared through Redis like `rpm` (#2936)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u32>,
    /// requests this route serves at once on one gateway replica, a streamed
    /// response included until its last byte. Refused with `429` and
    /// `Retry-After`; the count is per replica and works without Redis (#2936)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
}

/// Largest `rpm` or `concurrency` the control plane accepts for a route.
pub const MAX_ROUTE_THROUGHPUT: u32 = 10_000_000;

impl ModelLimits {
    /// Every reason one of `rpm`, `tpm` or `concurrency` cannot be applied, one
    /// sentence each: zero would refuse every request, which is a route that
    /// should be removed rather than limited, and `rpm` and `concurrency` stop
    /// at [`MAX_ROUTE_THROUGHPUT`].
    pub fn throughput_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, value, ceiling) in [
            ("rpm", self.rpm, MAX_ROUTE_THROUGHPUT),
            ("tpm", self.tpm, u32::MAX),
            ("concurrency", self.concurrency, MAX_ROUTE_THROUGHPUT),
        ] {
            if value.is_some_and(|v| v == 0 || v > ceiling) {
                problems.push(format!("limits.{name} must be between 1 and {ceiling}"));
            }
        }
        problems
    }

    /// Unset every throughput cap [`Self::throughput_problems`] names, so the
    /// route serves unlimited on that axis instead of refusing everything.
    pub fn drop_unusable_throughput(&mut self) -> Vec<String> {
        let problems = self.throughput_problems();
        if !problems.is_empty() {
            let usable = |v: &mut Option<u32>, ceiling: u32| {
                if v.is_some_and(|n| n == 0 || n > ceiling) {
                    *v = None;
                }
            };
            usable(&mut self.rpm, MAX_ROUTE_THROUGHPUT);
            usable(&mut self.tpm, u32::MAX);
            usable(&mut self.concurrency, MAX_ROUTE_THROUGHPUT);
        }
        problems
    }

    /// The retry budget a request to this route gets, given the gateway-wide
    /// `default`.
    pub fn max_retries(&self, default: u32) -> u32 {
        self.retries.map_or(default, |n| n.min(MAX_ROUTE_RETRIES))
    }
}

/// The org, and for a route the project, a row belongs to in the store.
///
/// Absent on rows from a gateway-only config file: those belong to the
/// deployment and every key may use them. A row that carries one is served
/// only to keys of the same org (#1844), which is what keeps one tenant's
/// provider credentials out of another tenant's requests.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Tenancy {
    pub org_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl Tenancy {
    /// Whether a key of `key_org` may use a row owned by `tenancy`.
    ///
    /// A row with no tenancy belongs to the deployment. A key with no org was
    /// defined in the gateway's own config file, so it is the operator's and
    /// may use any row. Otherwise the orgs must match.
    pub fn admits(tenancy: Option<&Tenancy>, key_org: &str) -> bool {
        match tenancy {
            None => true,
            Some(owner) => key_org.is_empty() || owner.org_id == key_org,
        }
    }

    /// Whether a key minted in `key_project` of `key_org` may use a provider or
    /// provider group owned by `tenancy`.
    ///
    /// [`admits`](Self::admits) for the org, plus the optional project scope: a
    /// provider or group whose `project_id` is set serves only that project, and
    /// a key with an org but no project is refused too. Unset means org-wide.
    /// Only for providers and groups: on a route `project_id` names the
    /// route's own project and narrows nothing unless `project_only` is set.
    pub fn admits_scoped(tenancy: Option<&Tenancy>, key_org: &str, key_project: &str) -> bool {
        if !Self::admits(tenancy, key_org) {
            return false;
        }
        match tenancy.and_then(|t| t.project_id.as_deref()) {
            Some(scope) => key_org.is_empty() || scope == key_project,
            None => true,
        }
    }
}

/// Catalog visibility restrictions attached to a route. Gateway key/team
/// identity is sufficient for key and team checks; user restrictions are
/// retained for control-plane authorization.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct ModelVisibility {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_role: Option<String>,
    #[serde(default)]
    pub allowed_team_ids: Vec<String>,
    #[serde(default)]
    pub allowed_key_ids: Vec<String>,
    #[serde(default)]
    pub allowed_user_ids: Vec<String>,
    /// Serve the route only to keys minted in its own project, rather than to
    /// every key of its org (the default). Keys from the gateway's own config
    /// carry no org and are not narrowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_only: bool,
}

fn default_weight() -> u32 {
    1
}

/// Maps a public model name to one or more upstream targets plus a strategy.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelRoute {
    /// public model name clients request, e.g. `gpt-4o`
    pub model: String,
    #[serde(default)]
    pub strategy: BalancingStrategy,
    /// upstream targets for the classic single-pool path; may be empty when the
    /// route routes through [`ModelRoute::variants`] instead
    #[serde(default)]
    pub targets: Vec<Target>,
    /// admin-set default inference params injected into the request body (e.g.
    /// `temperature`, `max_tokens`, `stop`). An unset param is passed through
    /// untouched. Provider-agnostic: keys are whatever the upstream accepts.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// whether callers may override the admin defaults in [`ModelRoute::params`]
    #[serde(default)]
    pub param_policy: ParamPolicy,
    /// catalog metadata and model-scoped execution policy
    #[serde(default)]
    pub advanced: AdvancedModelConfig,
    /// optional weighted variants for A/B, canary, and key-split traffic. When
    /// present, a request samples one variant by weight (the primary) and falls
    /// back to the remaining variants in declared order; `targets`/`strategy`
    /// above drive the classic single-pool path when this is empty.
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// opt-in exact-match response caching for this route. Caching applies only
    /// when both the global `[cache]` switch and this route opt-in are enabled.
    #[serde(default)]
    pub cache: Option<RouteCache>,
    /// the org and project the route belongs to in the store; absent for a
    /// route from the gateway's own config file (see [`Tenancy`])
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenancy: Option<Tenancy>,
}

impl ModelRoute {
    /// Apply the admin param defaults to a parsed JSON request body in place.
    ///
    /// For each configured default param: when the caller did not send it, the
    /// default is injected; when the caller did send it, the value is kept only
    /// if [`ParamPolicy`] permits overriding that param, otherwise the admin
    /// default silently wins (the safer default, matching most gateways). Params
    /// with no configured default are never touched.
    pub fn apply_params(&self, body: &mut serde_json::Value) {
        if self.params.is_empty() {
            return;
        }
        let Some(obj) = body.as_object_mut() else {
            return;
        };
        for (key, default) in &self.params {
            let caller_sent = obj.contains_key(key);
            if !caller_sent || !self.param_policy.may_override(key) {
                obj.insert(key.clone(), default.clone());
            }
        }
    }

    /// Apply the route's param defaults with a variant's params layered over
    /// them (the variant wins on a key collision), under the route's override
    /// policy. Same per-param semantics as [`ModelRoute::apply_params`]: a
    /// caller value survives only where the policy permits, otherwise the
    /// (merged) admin default wins.
    pub fn apply_variant_params(&self, variant: &Variant, body: &mut serde_json::Value) {
        if self.params.is_empty() && variant.params.is_empty() {
            return;
        }
        let Some(obj) = body.as_object_mut() else {
            return;
        };
        // route defaults first, then the variant overrides them by key
        let mut effective: HashMap<&String, &serde_json::Value> = self.params.iter().collect();
        for (key, value) in &variant.params {
            effective.insert(key, value);
        }
        for (key, default) in effective {
            let caller_sent = obj.contains_key(key.as_str());
            if !caller_sent || !self.param_policy.may_override(key) {
                obj.insert(key.clone(), default.clone());
            }
        }
    }

    /// Whether this route routes through weighted variants rather than a single
    /// target pool.
    pub fn has_variants(&self) -> bool {
        !self.variants.is_empty()
    }

    /// Every target this route can forward to: the classic pool's and each
    /// variant's.
    pub fn every_target(&self) -> impl Iterator<Item = &Target> {
        self.targets.iter().chain(
            self.variants
                .iter()
                .flat_map(|variant| variant.targets.iter()),
        )
    }

    /// The targets the data plane dials first: the variants' targets when the
    /// route has variants, because `forward_variants` never consults the
    /// classic pool then, and the pool otherwise (#3430).
    pub fn dialled_targets(&self) -> Box<dyn Iterator<Item = &Target> + '_> {
        dialled_targets(&self.targets, &self.variants)
    }

    /// Whether this route has targets to dial and every one of them is
    /// restricted, so it could never serve a first dial.
    pub fn only_restricted_targets_dialled(&self) -> bool {
        only_restricted_targets_dialled(&self.targets, &self.variants)
    }

    /// Every problem with the `takes_over` sets of this route's targets, the
    /// classic pool's and every variant's together: a class listed twice on a
    /// target, and a route whose every dialled target is restricted
    /// ([`dialled_targets`](Self::dialled_targets)), which is refused by name
    /// ([`crate::failover::ALL_TARGETS_RESTRICTED`]) because it could never
    /// dial anything.
    pub fn takes_over_problems(&self) -> Vec<String> {
        // a class listed twice is judged over every target, but a route with
        // variants dials only them, so "nothing to dial" is judged apart
        let mut problems = crate::failover::duplicate_class_problems(
            &self.model,
            self.every_target()
                .map(|t| (t.provider.as_str(), t.takes_over.as_slice())),
        );
        if self.only_restricted_targets_dialled() {
            problems.push(crate::failover::all_targets_restricted(&self.model));
        }
        problems
    }

    /// Whether this route has a target that takes refusals over, which is
    /// what makes the gateway read a refused attempt's body at all.
    pub fn takes_over_refusals(&self) -> bool {
        self.every_target().any(|t| {
            t.takes_over
                .iter()
                .any(|c| c.kind == crate::failover::FailureKind::Refusal)
        })
    }

    /// Whether this route opts into response caching (the global `[cache]`
    /// switch still has to be on for caching to actually happen).
    pub fn cache_enabled(&self) -> bool {
        self.cache.as_ref().is_some_and(|c| c.enabled)
    }

    /// TTL in seconds for this route's cached responses: the route override when
    /// set, else the global `default_ttl_secs`.
    pub fn cache_ttl_secs(&self, default_ttl_secs: u64) -> u64 {
        self.cache
            .as_ref()
            .and_then(|c| c.ttl_secs)
            .unwrap_or(default_ttl_secs)
    }

    /// Whether cached entries for this route are isolated per virtual key.
    pub fn cache_per_key(&self) -> bool {
        self.cache.as_ref().is_some_and(|c| c.per_key)
    }

    /// Sample the primary variant index by weight, given a random draw `r` in
    /// `[0.0, 1.0)`. Returns `None` only when there are no variants.
    pub fn sample_variant(&self, r: f64) -> Option<usize> {
        weighted_index(self.variants.iter().map(|v| v.weight), r)
    }

    /// The order variants are tried for a request whose primary is `primary`:
    /// the primary first, then every other variant in declared order (the
    /// deterministic fallback chain).
    pub fn fallback_order(&self, primary: usize) -> Vec<usize> {
        let n = self.variants.len();
        let mut order = Vec::with_capacity(n);
        if primary < n {
            order.push(primary);
        }
        order.extend((0..n).filter(|&i| i != primary));
        order
    }
}

/// The targets the data plane dials first for a route with this pool and
/// these variants: the variants' targets when there are any, the pool
/// otherwise (#3430). The store's snapshot assembly applies the same rule
/// before the route is built.
pub fn dialled_targets<'a>(
    pool: &'a [Target],
    variants: &'a [Variant],
) -> Box<dyn Iterator<Item = &'a Target> + 'a> {
    if variants.is_empty() {
        Box::new(pool.iter())
    } else {
        Box::new(variants.iter().flat_map(|v| v.targets.iter()))
    }
}

/// Whether there are targets to dial and every one is restricted.
pub fn only_restricted_targets_dialled(pool: &[Target], variants: &[Variant]) -> bool {
    only_restricted_dialled(
        pool.iter().map(Target::is_restricted),
        variants
            .iter()
            .map(|variant| variant.targets.iter().map(Target::is_restricted)),
    )
}

/// [`only_restricted_targets_dialled`] over any representation of a route's
/// targets, given whether each one is restricted: `pool` holds one flag per
/// pool target and `variants` one set of flags per variant. The variants'
/// targets count when there are variants, the pool otherwise, as
/// [`dialled_targets`] picks them. The control plane's write checks apply the
/// rule to stored rows and request bodies through this (#3487).
pub fn only_restricted_dialled<V>(
    pool: impl IntoIterator<Item = bool>,
    variants: impl IntoIterator<Item = V>,
) -> bool
where
    V: IntoIterator<Item = bool>,
{
    fn all_restricted(mut restricted: impl Iterator<Item = bool>) -> bool {
        restricted
            .next()
            .is_some_and(|first| first && restricted.all(|r| r))
    }
    let mut variants = variants.into_iter().peekable();
    if variants.peek().is_none() {
        all_restricted(pool.into_iter())
    } else {
        all_restricted(variants.flatten())
    }
}

/// A weighted routing variant: a named, weighted bundle of ordered targets plus
/// optional param defaults. A logical model maps to a set of these for A/B,
/// canary, and per-key traffic splitting under one schema (TensorZero pattern).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Variant {
    /// stable identifier for the variant (used in logs/metrics/attribution)
    pub name: String,
    /// relative traffic share; 0 takes no first attempts while another variant has
    /// a positive weight (it is still a fallback), and an all-0 set is uniform
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// ordered upstream targets tried within this variant (provider routing order)
    pub targets: Vec<Target>,
    /// variant-scoped default inference params (same semantics as route params)
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// Pick an index from a sequence of weights proportional to weight, given a
/// random draw `r` in `[0.0, 1.0)`. An entry with weight 0 is never picked
/// while any entry has a positive weight (variant fallback goes through
/// [`ModelRoute::fallback_order`], so it is still tried after a failure); a
/// sequence whose weights are all 0 is sampled uniformly. Returns `None` for
/// an empty sequence.
fn weighted_index(weights: impl Iterator<Item = u32>, r: f64) -> Option<usize> {
    let mut shares: Vec<u64> = weights.map(u64::from).collect();
    if shares.is_empty() {
        return None;
    }
    if shares.iter().all(|&w| w == 0) {
        shares.iter_mut().for_each(|w| *w = 1);
    }
    let total: u64 = shares.iter().sum();
    // map r into [0, total); guard against r outside [0,1) landing past the end
    let mut point = (r.clamp(0.0, 1.0) * total as f64) as u64;
    if point >= total {
        point = total - 1;
    }
    let mut acc = 0u64;
    for (i, w) in shares.iter().enumerate() {
        acc += w;
        if point < acc {
            return Some(i);
        }
    }
    Some(shares.len() - 1)
}

/// Default override mode for a route's params: whether callers may override the
/// admin defaults unless listed as an exception.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideMode {
    /// callers may override any default except those in `deny`
    #[default]
    Allow,
    /// callers may override no default except those in `allow`
    Deny,
    /// a mode this build does not know (#3347); read as
    /// [`OverrideMode::Deny`], so callers override only what `allow` lists
    #[serde(other)]
    Unknown,
}

/// Per-route policy governing whether callers may override the admin param
/// defaults. The `mode` sets the baseline; `allow`/`deny` are per-param
/// exceptions to it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ParamPolicy {
    #[serde(default)]
    pub mode: OverrideMode,
    /// params callers may override when `mode = "deny"`
    #[serde(default)]
    pub allow: Vec<String>,
    /// params callers may not override when `mode = "allow"`
    #[serde(default)]
    pub deny: Vec<String>,
}

impl ParamPolicy {
    /// Whether a caller-supplied value for `param` may override the admin default.
    pub fn may_override(&self, param: &str) -> bool {
        match self.mode {
            OverrideMode::Allow => !self.deny.iter().any(|p| p == param),
            OverrideMode::Deny | OverrideMode::Unknown => self.allow.iter().any(|p| p == param),
        }
    }
}

/// The example virtual key that ships in `rolter.example.toml`, the image's
/// baked config and `easy-up`. It is public, allows every model and is for
/// local development only.
pub const PUBLIC_EXAMPLE_KEY: &str = "sk-rolter-dev";

/// A virtual api key that clients present to the gateway.
///
/// `Default` is a nameless, unscoped, non-expiring key with an empty secret —
/// useful only as a fixture base (see #1150); nothing deserializes it, `key` is
/// a required field.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct VirtualKeyConfig {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
    /// allowed public model names; empty means all models are allowed
    #[serde(default)]
    pub models: Vec<String>,
    /// allowed upstream provider names; empty means every provider on an
    /// allowed route may be selected. this is independent from `models`, which
    /// controls the public route/model namespace exposed to the client
    #[serde(default)]
    pub providers: Vec<String>,
    /// administratively revoke the key without deleting it
    #[serde(default)]
    pub disabled: bool,
    /// optional expiry; the key stops authenticating at/after this instant
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// per-key response-cache override, independent of a route's own opt-in.
    /// `None` inherits the route decision; `Some(false)` forces this key's
    /// responses to bypass the cache; `Some(true)` caches them even on a route
    /// that didn't opt in. the global `[cache]` switch is still required either way
    #[serde(default)]
    pub cache: Option<bool>,
}

impl VirtualKeyConfig {
    /// Whether this is the example key every rolter checkout publishes.
    pub fn is_public_example_key(&self) -> bool {
        self.key == PUBLIC_EXAMPLE_KEY
    }

    /// Whether the key may authenticate at `now`: not disabled and not expired.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.disabled && self.expires_at.is_none_or(|exp| now < exp)
    }
}

/// What kind of entry [`GatewayConfig::sanitize_for_snapshot`] left out of the
/// snapshot whole.
///
/// Only entries that stop being served are records. A target, header, cache or
/// price dropped from an entry that is still served stays a line in the
/// problems list and nothing more: the entry has not gone missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OmittedKind {
    /// a provider with a defect of its own
    Provider,
    /// a route with nothing left to serve
    Route,
    /// a key from the config file whose allow-list named only omitted providers
    VirtualKey,
    /// the example key every checkout publishes, dropped from a deployment that
    /// enforces auth
    PublicExampleKey,
}

/// One entry left out of the snapshot, as a record.
///
/// The structured twin of a line in the problems list: it is produced where the
/// entry is dropped, so a client never reads the sentence to learn what went
/// missing.
///
/// `index` is how a client points at exactly one row. The dashboard's
/// `GET /api/v1/config` strips tenancy, so two scopes' routes can share a
/// model and two file keys a name; a name alone would mark both. The index is
/// the position the row has in the list the store loaded, before anything was
/// dropped, which is the position it has in the matching list of that
/// endpoint (`providers`, `routes` or `virtual_keys`). A client that must be
/// safe against the two reads straddling a write checks that the row at
/// `index` still carries `name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OmittedEntry {
    pub kind: OmittedKind,
    /// a provider name, a route model or a key's `name` (`<unnamed>` without one)
    pub name: String,
    /// why, in the words the problem line uses
    pub reason: String,
    /// position of the row in its list, see above
    pub index: usize,
}

/// What [`GatewayConfig::sanitize_for_snapshot_report`] found: the sentences
/// and, for every entry it dropped whole, the record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotReport {
    /// one English line per finding, including ones that drop nothing (a denied
    /// operator url, a request id header that was replaced)
    pub problems: Vec<String>,
    /// one record per entry dropped whole
    pub omitted: Vec<OmittedEntry>,
    /// the loaded position of each file key still in the config, so a later
    /// drop reports the index the dashboard's list has rather than a shifted one
    file_key_origin: Vec<usize>,
}

/// What a file key without a `name` is called in a problem line and a record.
const UNNAMED_KEY: &str = "<unnamed>";

/// A database-defined virtual key as seen by the gateway: the peppered digest
/// (never the plaintext) plus the scope identity used for log attribution,
/// budgets and rate limits.
///
/// `Default` is an enabled key with no scope and no restrictions; it exists
/// for fixtures, production code writes every field out.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct VirtualKeyRecord {
    /// `rolter_auth::hash_key(pepper, key)` — how the gateway looks the key up
    pub key_hash: String,
    pub id: String,
    #[serde(default)]
    pub org_id: String,
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub project_id: String,
    /// user who minted the key. MCP OAuth calls require this owner identity;
    /// admin-created and config-defined keys deliberately have none.
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// allowed upstream provider names; empty means all providers are allowed
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    /// per-key response-cache override; see [`VirtualKeyConfig::cache`]
    #[serde(default)]
    pub cache: Option<bool>,
    /// business unit this key's spend is attributed to; empty when unattributed
    #[serde(default)]
    pub business_unit_id: String,
    /// customer this key's spend is attributed to; empty when unattributed
    #[serde(default)]
    pub customer_id: String,
    /// merged model/route policy of the access profiles the key's owner holds,
    /// resolved when the snapshot is built (#791). `None` is "unrestricted" and
    /// is what every key on a deployment with no access profiles carries — as
    /// does any key with no owner, since a policy is a property of a user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_policy: Option<crate::access_policy::ModelPolicy>,
    /// the secret this key was rotated away from, while its overlap window is
    /// still open (#1837). `None` once the window closes or the old secret is
    /// revoked early
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<PreviousKeySecret>,
}

/// The secret a rotated virtual key used to carry. It authenticates as the same
/// key, with the same limits and attribution, until `valid_until`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreviousKeySecret {
    /// `rolter_auth::hash_key(pepper, key)` of the retired secret
    pub key_hash: String,
    /// last instant the retired secret authenticates; the key's own
    /// `expires_at` still caps it when that is earlier
    pub valid_until: DateTime<Utc>,
}

/// How the gateway authenticates to an MCP server.
///
/// The string form is the `auth_kind` column of `mcp_servers`, whose check
/// constraint is the authority on the set (migration `0069`). An unrecognised
/// value deserializes to [`McpAuthKind::None`] rather than failing the whole
/// snapshot: a newer control plane may name a kind this gateway predates, and
/// refusing to authenticate is the safe reading of "I do not know what this
/// is" — the alternative is a gateway that drops every server on the snapshot
/// floor because one row is from the future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthKind {
    /// no credential is presented; the server is open or network-restricted
    #[default]
    None,
    /// `Authorization: Bearer <credential>`
    Bearer,
    /// `<auth_header_name>: <credential>`
    Header,
    /// a token minted per user by the consent flow, not a stored credential
    Oauth,
    /// a kind this build does not know; treated as [`McpAuthKind::None`]
    #[serde(other)]
    Unknown,
}

/// An organization-scoped MCP endpoint available to the gateway.
///
/// `credential` is plaintext. It is unsealed from the KEK when the snapshot is
/// built and travels over `/internal/snapshot`, which is already the
/// credential-bearing control-to-data-plane channel — the same route
/// [`McpOAuthSessionConfig::access_token`] takes. The [`std::fmt::Debug`] impl
/// below redacts it so it cannot reach a log line or a crash dump through the
/// snapshot being formatted (#952).
///
/// `Default` exists for the fixtures that construct this everywhere, so a test
/// naming one field does not have to spell out the rest; the production mapping
/// in `rolter-store` still writes every field out. The default is the
/// unauthenticated server with no overrides, matching the `auth_kind` column's
/// own default.
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub id: String,
    pub org_id: String,
    pub slug: String,
    pub url: String,
    pub transport: String,
    /// OAuth scopes every call through this server must carry.
    #[serde(default)]
    pub required_scopes: Vec<String>,
    /// how to authenticate; `oauth` keeps the per-user consent flow
    #[serde(default)]
    pub auth_kind: McpAuthKind,
    /// the header an API key is presented in, for [`McpAuthKind::Header`]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header_name: Option<String>,
    /// the unsealed static credential, absent for `none`/`oauth` and for a row
    /// whose ciphertext could not be decrypted
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// per-server transport overrides; `None` inherits the deployment default
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
    /// the tools the server's manifest declares, discovered or typed. Empty
    /// means it declares none, which leaves nothing to check a call against
    #[serde(default)]
    pub tools: Vec<String>,
    /// whether the server's organization enforces manifests until it saves MCP
    /// settings of its own (#3334): true for an organization created after the
    /// change, false for one that existed before it. A saved settings row
    /// decides for itself and overrides this
    #[serde(default)]
    pub enforce_manifest_by_default: bool,
}

impl std::fmt::Debug for McpServerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpServerConfig")
            .field("id", &self.id)
            .field("org_id", &self.org_id)
            .field("slug", &self.slug)
            .field("url", &self.url)
            .field("transport", &self.transport)
            .field("required_scopes", &self.required_scopes)
            .field("auth_kind", &self.auth_kind)
            .field("auth_header_name", &self.auth_header_name)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "[REDACTED]"),
            )
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("max_retries", &self.max_retries)
            .field("tools", &self.tools.len())
            .finish()
    }
}

/// An organization's MCP settings as the proxy applies them (#1404).
///
/// Precedence for the three transport values, highest first: the server's own
/// override (`McpServerConfig::connect_timeout_ms` and its siblings), then
/// these, then the deployment-level `[timeouts]` default. `default_transport`
/// is not here: a server row always names its transport, so the org default is
/// what the control plane gives a server registered without one.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct McpOrgSettingsConfig {
    pub org_id: String,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    /// times a call is repeated after a connection that was never made; a
    /// call that reached the server is never repeated
    pub max_retries: u32,
    /// Deprecated and ignored by the gateway (#3527): a request that cannot be
    /// inspected for tool policy is refused whatever this says, since one a
    /// more lenient upstream parser reads could call a refused tool. Still
    /// carried so the snapshot stays at format 1; removed in 2.0.0.
    #[serde(default)]
    pub fail_open: bool,
    /// whether a call may name a tool the server's manifest does not declare
    #[serde(default)]
    pub allow_unlisted_tools: bool,
}

/// One organization's `orgs.mcp_enforce_manifests` marker in a bootstrap file
/// (#3398): the organization by slug, and whether it enforces manifests until it
/// saves MCP settings of its own.
///
/// An import never makes an existing organization stricter, so `enforce = true`
/// is what a new organization already has and `enforce = false` is the only
/// value that changes a row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpManifestEnforcementConfig {
    pub org: String,
    pub enforce: bool,
}

/// One organization's saved `mcp_gateway_settings` row in a bootstrap file
/// (#3482): the organization by slug, and the settings it saved.
///
/// Unlike [`McpManifestEnforcementConfig`] this is the desired state: an import
/// sets the targeted organization to exactly these values, stricter or laxer
/// than it was, and only that organization. `allow_unlisted_tools = false` is
/// the organization enforcing its servers' tool manifests.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpGatewaySettingsConfig {
    pub org: String,
    /// one of [`MCP_TRANSPORTS`]
    pub default_transport: String,
    pub connect_timeout_ms: i32,
    pub request_timeout_ms: i32,
    pub max_retries: i32,
    /// `fail_open` or `fail_closed`. Deprecated: accepted and exported, and
    /// ignored by the gateway, which refuses every body it cannot judge (#3527)
    pub default_failure_mode: String,
    pub allow_unlisted_tools: bool,
}

/// One organization's MCP tool group in a bootstrap file (#3514): the group by
/// its slug, the tools it allows and whom it is assigned to, every one of them
/// named the way the file names it rather than by a database id.
///
/// The import is the desired state for the groups the file names, for the
/// organization it targets only. An assignment whose team, project or key does
/// not exist on the receiving deployment is skipped with a warning; the group
/// is still created, so its other assignments still narrow their callers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpGatewayToolGroupConfig {
    /// the organization's slug
    pub org: String,
    /// the group's slug, unique within the organization
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// a disabled group governs no one but the callers assigned to it, who get
    /// none of its tools
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// the tools the group allows, each on a server of the organization
    #[serde(default)]
    pub tools: Vec<McpGatewayToolRefConfig>,
    /// teams the group is assigned to, as `org/team`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<String>,
    /// projects the group is assigned to, as `org/team/project`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// virtual keys the group is assigned to
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<McpGatewayKeyRefConfig>,
}

/// One tool of an [`McpGatewayToolGroupConfig`]: the server by its slug in the
/// group's organization, and the tool by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpGatewayToolRefConfig {
    pub server: String,
    pub tool: String,
}

/// A virtual key an [`McpGatewayToolGroupConfig`] is assigned to, named by its
/// project and its public identifier, never by its secret or digest: its
/// `name` when it has one, else the `prefix` the dashboard shows. A reference
/// that matches no key of the project, or more than one, is skipped by the
/// import with a warning.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpGatewayKeyRefConfig {
    /// the key's project, as `org/team/project`
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// One tool of one server, as a tool group names it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpToolRefConfig {
    pub server_id: String,
    pub tool: String,
}

/// An enabled, organization-owned bundle of MCP tools (#1403).
///
/// Only enabled groups reach the snapshot. A server that at least one of them
/// names is *governed*: the proxy forwards a `tools/call` for it only when the
/// tool is in the union of the groups that name it, and refuses the rest. A
/// server no group names is not governed by this and falls back to its declared
/// manifest.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct McpToolGroupConfig {
    pub id: String,
    pub org_id: String,
    pub slug: String,
    #[serde(default)]
    pub tools: Vec<McpToolRefConfig>,
    /// who the group is assigned to (#3295). A caller whose key, project or team
    /// appears in the assignments of any enabled group is held to the groups
    /// assigned to it; a caller named by none keeps the organization-wide set
    #[serde(default)]
    pub assignments: Vec<McpToolGroupSubjectConfig>,
    /// a disabled group is carried only for the callers assigned to it (#3295):
    /// it governs no one else, but those callers stay narrowed on the servers it
    /// names and get none of its tools
    #[serde(default)]
    pub disabled: bool,
}

/// A virtual key, project or team an MCP tool group is assigned to (#3295).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct McpToolGroupSubjectConfig {
    /// `virtual_key`, `project` or `team`; any other value matches no caller
    pub kind: String,
    pub id: String,
}

/// A live OAuth session the gateway may spend for one user and MCP server.
///
/// This type is serialized only through the internal snapshot endpoint, which
/// is already the credential-bearing control-to-data-plane channel.
#[derive(Clone, Deserialize, Serialize)]
pub struct McpOAuthSessionConfig {
    pub id: String,
    pub server_id: String,
    pub user_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub access_token: String,
}

impl std::fmt::Debug for McpOAuthSessionConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpOAuthSessionConfig")
            .field("id", &self.id)
            .field("server_id", &self.server_id)
            .field("user_id", &self.user_id)
            .field("scopes", &self.scopes)
            .field("expires_at", &self.expires_at)
            .field("access_token", &"[REDACTED]")
            .finish()
    }
}

impl VirtualKeyRecord {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.disabled && self.expires_at.is_none_or(|exp| now < exp)
    }
}

/// Per-model pricing. Token rates are `currency` per million tokens (matching
/// the `model_prices` catalog); the unit rates price the requests a provider
/// bills by the image, the minute of audio or the character instead (#2937).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelPriceConfig {
    /// public model name this price applies to
    pub model: String,
    /// The rates are [`Decimal`], not `f64`: the `model_prices` columns behind
    /// them are `numeric(12,6)`, and parsing that into binary floating point
    /// discards the exactness the column was chosen for (#967).
    ///
    /// They stay *on the wire* as JSON numbers. `/internal/snapshot` carries a
    /// `config_version`, which is a change counter rather than a schema
    /// version, so control plane and gateway at different builds exchange the
    /// same shape during any rolling upgrade — emitting a string here would
    /// break an older gateway mid-upgrade, silently, on the money path. At
    /// `numeric(12,6)` there is no precision to lose in the round trip; the
    /// representation has to change only when a unit with more significant
    /// digits than `f64` carries actually exists, which is the crypto
    /// settlement `currency.rs` anticipates and nothing yet implements.
    #[serde(default, with = "rust_decimal::serde::float")]
    pub input_per_mtok: Decimal,
    #[serde(default, with = "rust_decimal::serde::float")]
    pub output_per_mtok: Decimal,
    /// price for cache-hit input tokens; falls back to `input_per_mtok`
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub cached_input_per_mtok: Option<Decimal>,
    /// price for input tokens written to the provider's prompt cache (#2876);
    /// falls back to `input_per_mtok`, which is what a row without it has
    /// always been charged. Anthropic bills a write at a premium over the
    /// input rate, so a write-heavy workload is under-stated until a row sets
    /// it. `default` is what lets a snapshot from a control plane that predates
    /// the field still parse
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub cache_write_per_mtok: Option<Decimal>,
    /// price for input tokens written to the provider's 1 hour prompt cache
    /// (#2891), for a provider that bills it apart from the 5 minute one.
    /// Anthropic does: a 1 hour write is twice the input rate where a 5 minute
    /// one is 1.25 times. Falls back to `cache_write_per_mtok`, then to
    /// `input_per_mtok`, so a row that sets only the one write rate prices both
    /// caches at it, as every row did before this field. Only a provider that
    /// reports the split (`usage.cache_creation`) has tokens to charge at it
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub cache_write_1h_per_mtok: Option<Decimal>,
    /// price of one generated image (`/v1/images/generations`), for a model
    /// the provider bills per image (#2937). The count is the `data` array of
    /// the response, or the request's `n` when the response does not carry one.
    /// Added to the token cost, so a model that reports tokens as well (the
    /// `gpt-image` family) sets only the rate the provider actually bills.
    /// `None` charges nothing for images
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub image_per_unit: Option<Decimal>,
    /// price of one minute of input audio (`/v1/audio/transcriptions`,
    /// `/v1/audio/translations`), for a model billed by the minute (#2937).
    /// The minutes are the duration the response states (`verbose_json`'s
    /// `duration`, or a `usage` of type `duration`); a response that states
    /// none is recorded as unknown usage instead of free
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub audio_input_per_minute: Option<Decimal>,
    /// price per million characters of text to synthesise
    /// (`/v1/audio/speech`), for a text-to-speech model billed that way
    /// (#2937). The characters are counted from the request's `input`, since
    /// the audio that comes back is binary and states no length
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub speech_per_mchar: Option<Decimal>,
    /// price per million audio input tokens (#1882), for a realtime or audio
    /// model that bills the audio it is sent apart from the text. The share of
    /// the prompt that is audio is what the response's usage names
    /// (`input_token_details.audio_tokens` on a realtime `response.done` and on
    /// a transcription's usage, `prompt_tokens_details.audio_tokens` on a chat
    /// completion). Falls back to `input_per_mtok`, which is what a row without
    /// it has always charged for those tokens
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub audio_input_per_mtok: Option<Decimal>,
    /// price per million audio output tokens (#1882); falls back to
    /// `output_per_mtok`. The audio share of the completion is
    /// `output_token_details.audio_tokens` (realtime) or
    /// `completion_tokens_details.audio_tokens` (chat completions)
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub audio_output_per_mtok: Option<Decimal>,
    /// price per million cached audio input tokens (#1882). Falls back to
    /// `cached_input_per_mtok`, then to the audio input rate: a cache hit is
    /// priced at the provider's cache rate when the row states one, and at the
    /// full audio rate when it does not. Only a provider that names the audio
    /// share of its cached tokens (a realtime `cached_tokens_details`) has
    /// tokens to charge at it
    #[serde(default, with = "rust_decimal::serde::float_option")]
    pub cached_audio_input_per_mtok: Option<Decimal>,
    /// currency the rates above are denominated in. An open code — ISO-4217
    /// today, a crypto or custom settlement unit tomorrow — resolved against
    /// [`crate::CurrencyConfig`]'s rate table, never against a fixed enum
    #[serde(default = "crate::currency::default_base_currency")]
    pub currency: String,
}

impl Default for ModelPriceConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            input_per_mtok: Decimal::ZERO,
            output_per_mtok: Decimal::ZERO,
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: crate::currency::default_base_currency(),
        }
    }
}

impl ModelPriceConfig {
    /// Every rate on this price that no price can have, one sentence each.
    ///
    /// A rate below zero would make [`Self::cost`] negative, so a request would
    /// credit its budget instead of spending it. The rates are [`Decimal`], so
    /// this cannot see `NaN` or infinity: those cannot be represented, and the
    /// store drops a row carrying one before it gets here (#2889).
    pub fn rate_problems(&self) -> Vec<String> {
        [
            ("input_per_mtok", Some(self.input_per_mtok)),
            ("output_per_mtok", Some(self.output_per_mtok)),
            ("cached_input_per_mtok", self.cached_input_per_mtok),
            ("cache_write_per_mtok", self.cache_write_per_mtok),
            ("cache_write_1h_per_mtok", self.cache_write_1h_per_mtok),
            ("image_per_unit", self.image_per_unit),
            ("audio_input_per_minute", self.audio_input_per_minute),
            ("speech_per_mchar", self.speech_per_mchar),
            ("audio_input_per_mtok", self.audio_input_per_mtok),
            ("audio_output_per_mtok", self.audio_output_per_mtok),
            (
                "cached_audio_input_per_mtok",
                self.cached_audio_input_per_mtok,
            ),
        ]
        .into_iter()
        .filter_map(|(field, rate)| {
            rate.filter(|rate| *rate < Decimal::ZERO)
                .map(|rate| format!("{field} is {rate}, and a rate cannot be negative"))
        })
        .collect()
    }

    /// Compute request cost, denominated in this price's own [`Self::currency`]
    /// — convert it to the base before it reaches a budget counter.
    ///
    /// `prompt` is the whole prompt, and the other input counts are shares of
    /// it rather than additions to it (#2863 made every dialect report it that
    /// way): `cached_input` is the portion served from the prompt cache
    /// (priced at the cached rate when set) and `cache_write` the portion
    /// written to it (priced at the write rate when set). `cache_write_1h` is
    /// the part of `cache_write` that went to a 1 hour cache, for a provider
    /// that reports the split (#2891); it is priced at
    /// [`Self::cache_write_1h_per_mtok`] and the rest of the writes at the
    /// plain write rate. All fall back to the input rate, and each token is
    /// priced once — cached tokens first, then written ones out of what is
    /// left, then the 1 hour share out of the writes. Pass 0 for a count that
    /// is unknown.
    pub fn cost(
        &self,
        prompt: u32,
        completion: u32,
        cached_input: u32,
        cache_write: u32,
        cache_write_1h: u32,
    ) -> Decimal {
        self.cost_with_audio(
            prompt,
            completion,
            cached_input,
            cache_write,
            cache_write_1h,
            AudioTokens::default(),
        )
    }

    /// [`Self::cost`] for a response that names the audio share of its tokens
    /// (#1882).
    ///
    /// `audio` counts are shares of `prompt` and `completion` like the cached
    /// counts, so a token is priced once. The prompt splits into an audio part
    /// and a text part; cache hits are taken out of each, the audio ones at
    /// [`Self::cached_audio_input_per_mtok`] and the text ones at the cached
    /// rate, and cache writes (which no audio provider reports) out of the
    /// text part. A cached count the provider does not split is read as text,
    /// and whatever of it does not fit the text part is priced as audio.
    /// Every audio rate falls back to its text rate, so a row that sets none
    /// is priced exactly as [`Self::cost`] prices it.
    pub fn cost_with_audio(
        &self,
        prompt: u32,
        completion: u32,
        cached_input: u32,
        cache_write: u32,
        cache_write_1h: u32,
        audio: AudioTokens,
    ) -> Decimal {
        let audio_in = audio.input.min(prompt);
        let text_in = prompt - audio_in;
        let cached = cached_input.min(prompt);
        // the audio share of the cache hits, never more than the audio there is
        let cached_audio = audio.cached_input.min(cached).min(audio_in);
        let cached_text = (cached - cached_audio).min(text_in);
        // what is left of the hits after both shares is audio the provider did
        // not split out, so it is priced with the fresh audio below
        let fresh_audio = audio_in - cached_audio;
        let written = cache_write.min(text_in - cached_text);
        let written_1h = cache_write_1h.min(written);
        let written_5m = written - written_1h;
        let fresh_text = text_in - cached_text - written;
        let audio_in_rate = self.audio_input_per_mtok.unwrap_or(self.input_per_mtok);
        let cached_rate = self.cached_input_per_mtok.unwrap_or(self.input_per_mtok);
        let cached_audio_rate = self
            .cached_audio_input_per_mtok
            .or(self.cached_input_per_mtok)
            .unwrap_or(audio_in_rate);
        let write_rate = self.cache_write_per_mtok.unwrap_or(self.input_per_mtok);
        let write_1h_rate = self.cache_write_1h_per_mtok.unwrap_or(write_rate);
        let audio_out = audio.output.min(completion);
        let audio_out_rate = self.audio_output_per_mtok.unwrap_or(self.output_per_mtok);
        // exact: token counts are integers and the rates are decimal, so the
        // only inexact step left would be the division, and a power of ten
        // divides exactly in a base-ten representation
        (Decimal::from(fresh_text) * self.input_per_mtok
            + Decimal::from(cached_text) * cached_rate
            + Decimal::from(written_5m) * write_rate
            + Decimal::from(written_1h) * write_1h_rate
            + Decimal::from(fresh_audio) * audio_in_rate
            + Decimal::from(cached_audio) * cached_audio_rate
            + Decimal::from(completion - audio_out) * self.output_per_mtok
            + Decimal::from(audio_out) * audio_out_rate)
            / Decimal::from(1_000_000u32)
    }

    /// Cost of the non-token units a request used, in this price's currency
    /// (#2937), or `None` when the request's kind has no rate set.
    ///
    /// `None` is "this price says nothing about this request's unit", which the
    /// caller reads together with whether the response reported tokens: only
    /// a request with neither is unpriced. A rate that is set with a count of
    /// zero is `Some(0)`, a priced request that cost nothing.
    pub fn unit_cost(&self, units: &BilledUnits) -> Option<Decimal> {
        let minute = Decimal::from(60u32);
        let million = Decimal::from(1_000_000u32);
        match *units {
            BilledUnits::TokensOnly => None,
            BilledUnits::Images(count) => {
                self.image_per_unit.map(|rate| rate * Decimal::from(count))
            }
            BilledUnits::AudioSeconds(seconds) => self
                .audio_input_per_minute
                .map(|rate| rate * seconds / minute),
            BilledUnits::SpeechChars(chars) => self
                .speech_per_mchar
                .map(|rate| rate * Decimal::from(chars) / million),
        }
    }

    /// [`Self::cost`] under its former name, from when every price was assumed
    /// to be USD. It is not USD unless [`Self::currency`] says so, and it prices
    /// no cache write: a caller that has the counts calls [`Self::cost`].
    #[deprecated(note = "renamed to `cost`: the result is in `self.currency`, not necessarily USD")]
    pub fn cost_usd(&self, prompt: u32, completion: u32, cached_input: u32) -> Decimal {
        self.cost(prompt, completion, cached_input, 0, 0)
    }
}

/// The audio share of a response's tokens, for a provider that bills audio and
/// text at different rates (#1882). Every count is part of the matching total
/// handed to [`ModelPriceConfig::cost_with_audio`], not an addition to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioTokens {
    /// audio tokens in the prompt
    pub input: u32,
    /// audio tokens in the completion
    pub output: u32,
    /// the audio tokens among the cache hits
    pub cached_input: u32,
}

/// The non-token quantity a request is billed by, read from the request and
/// its response (#2937). Which variant a request carries follows its endpoint,
/// so a request with none of these (chat, embeddings) prices by tokens alone.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum BilledUnits {
    /// the endpoint bills by tokens only
    #[default]
    TokensOnly,
    /// generated images
    Images(u32),
    /// seconds of input audio; `Decimal` so a fractional duration is exact
    AudioSeconds(Decimal),
    /// characters of text sent to a text-to-speech model
    SpeechChars(u64),
}

/// The scope level a [`BudgetConfig`] applies to. Matched against the request's
/// virtual-key scope chain (org → team → project → key) or the governance
/// dimensions the key is attributed to (business unit, customer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    Org,
    Team,
    Project,
    Key,
    /// business unit the key's spend rolls up to (#539)
    BusinessUnit,
    /// customer the key's spend rolls up to (#539)
    Customer,
    /// the person who minted the key, counted across every key they minted
    /// (`created_by`), so "each engineer may spend $50 a month however many
    /// keys they mint" is expressible (#1830)
    User,
    /// a scope this build does not know, read from a newer control plane's
    /// snapshot (#3347). The cap cannot be matched to the requests it was
    /// meant for, so it counts every request instead, against a counter of
    /// its own: it can only bind sooner than configured, never later
    #[serde(other)]
    Unknown,
}

/// Where a [`BudgetScope::User`] cap applies, when it does not apply to the
/// person's keys everywhere (#1830): only the keys they minted in one org, or
/// in one project. Meaningless on every other scope, where the id already
/// names a place.
///
/// Deliberately closed (#3347): it carries data, which `#[serde(other)]` cannot
/// stand in for. A new kind of place is a snapshot format bump (ADR-0038).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeWithin {
    /// an org id
    Org(String),
    /// a project id
    Project(String),
}

impl ScopeWithin {
    /// The part of a counter key that tells a narrowed cap from the person's
    /// un-narrowed one, and from the same person's cap in another place: two
    /// caps on one user must not share a counter.
    pub fn key_segment(&self) -> String {
        match self {
            ScopeWithin::Org(id) => format!("org:{id}"),
            ScopeWithin::Project(id) => format!("project:{id}"),
        }
    }

    /// Whether a request in `org` and `project` is inside this place.
    pub fn contains(&self, org: &str, project: &str) -> bool {
        match self {
            ScopeWithin::Org(id) => !org.is_empty() && id == org,
            ScopeWithin::Project(id) => !project.is_empty() && id == project,
        }
    }
}

impl BudgetScope {
    /// The scope's name inside a Redis spend key.
    pub fn key_label(self) -> &'static str {
        match self {
            BudgetScope::Org => "org",
            BudgetScope::Team => "team",
            BudgetScope::Project => "project",
            BudgetScope::Key => "key",
            BudgetScope::BusinessUnit => "business_unit",
            BudgetScope::Customer => "customer",
            BudgetScope::User => "user",
            // a counter of its own, so an older gateway counting every request
            // against it never charges a newer gateway's counter for that scope
            BudgetScope::Unknown => "unknown",
        }
    }

    /// The scope a stored `budgets.scope_type` names; `None` for a value the
    /// gateway does not enforce.
    pub fn from_scope_type(scope_type: &str) -> Option<BudgetScope> {
        Some(match scope_type {
            "org" => BudgetScope::Org,
            "team" => BudgetScope::Team,
            "project" => BudgetScope::Project,
            "virtual_key" => BudgetScope::Key,
            "business_unit" => BudgetScope::BusinessUnit,
            "customer" => BudgetScope::Customer,
            "user" => BudgetScope::User,
            _ => return None,
        })
    }
}

/// The rolling window a budget resets on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetPeriod {
    #[default]
    Monthly,
    Daily,
    /// never resets — a lifetime cap
    Total,
    /// a window this build does not know, read from a newer control plane's
    /// snapshot (#3347). The gateway cannot tell where the window starts, so
    /// the budget refuses every request it covers rather than letting them pass
    #[serde(other)]
    Unknown,
}

impl BudgetPeriod {
    /// Every spelling of a period a stored budget may carry, with the window
    /// each one means.
    ///
    /// The named forms are the ones `rolter.toml` takes. The shorthands are
    /// what budgets stored through the control plane have always used, `30d`
    /// above all since it is the column default, and they keep their meaning:
    /// `30d` is the calendar month, not a rolling thirty days. There are no
    /// rolling windows, so `7d` is not here (#1902).
    pub const SPELLINGS: [(&'static str, BudgetPeriod); 8] = [
        ("daily", BudgetPeriod::Daily),
        ("1d", BudgetPeriod::Daily),
        ("24h", BudgetPeriod::Daily),
        ("monthly", BudgetPeriod::Monthly),
        ("30d", BudgetPeriod::Monthly),
        ("total", BudgetPeriod::Total),
        ("lifetime", BudgetPeriod::Total),
        ("all", BudgetPeriod::Total),
    ];

    /// Read a stored period, ignoring case and surrounding whitespace.
    ///
    /// `None` for anything outside [`SPELLINGS`](Self::SPELLINGS). The control
    /// plane refuses such a value on write, and the snapshot loader, which has
    /// to produce a config whatever an older row says, falls back to monthly
    /// and reports the row as a config problem.
    pub fn parse(value: &str) -> Option<BudgetPeriod> {
        let value = value.trim();
        Self::SPELLINGS
            .iter()
            .find(|(spelling, _)| spelling.eq_ignore_ascii_case(value))
            .map(|(_, period)| *period)
    }

    /// Identifier of the current window at `now`; part of the Redis spend key so
    /// a new window starts with a zero counter.
    pub fn bucket(&self, now: DateTime<Utc>) -> String {
        match self {
            BudgetPeriod::Daily => now.format("%Y%m%d").to_string(),
            BudgetPeriod::Monthly => now.format("%Y%m").to_string(),
            BudgetPeriod::Total => "all".to_string(),
            // never read: a budget of an unknown period refuses before its
            // counter is consulted
            BudgetPeriod::Unknown => "unknown".to_string(),
        }
    }

    /// When the window that contains `now` ends and the counter starts again
    /// from zero; `None` for [`BudgetPeriod::Total`], which never resets.
    pub fn next_reset(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        use chrono::{Datelike, TimeZone};
        match self {
            BudgetPeriod::Daily => {
                let next = now.date_naive().succ_opt()?;
                Utc.from_utc_datetime(&next.and_hms_opt(0, 0, 0)?).into()
            }
            BudgetPeriod::Monthly => {
                let (year, month) = if now.month() == 12 {
                    (now.year() + 1, 1)
                } else {
                    (now.year(), now.month() + 1)
                };
                Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()
            }
            BudgetPeriod::Total | BudgetPeriod::Unknown => None,
        }
    }

    /// TTL for the Redis spend counter (generous, just for cleanup — the bucket
    /// key already partitions windows). `None` for [`BudgetPeriod::Total`].
    pub fn ttl_secs(&self) -> Option<u64> {
        match self {
            BudgetPeriod::Daily => Some(2 * 24 * 3600),
            BudgetPeriod::Monthly => Some(40 * 24 * 3600),
            BudgetPeriod::Total | BudgetPeriod::Unknown => None,
        }
    }
}

/// What a budget does about traffic it cannot price (#974).
///
/// A model with no price row accrues zero spend, so it can never exceed a
/// budget no matter how much is served — a cost control that fails open on
/// exactly the models an operator just added and is least likely to have priced.
/// This is the deliberate decision about that, in place of the silent pass.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Deserialize, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum UnpricedPolicy {
    /// serve it, accrue nothing, say nothing further. The pre-#974 behaviour.
    ///
    /// This is the default because `block` would reject traffic on upgrade for
    /// any deployment with an unpriced model — which #969 measured as most of
    /// them — and turning a version bump into an outage is not a safe default
    /// even when the stricter reading is the better policy. The #969 notice is
    /// what tells an operator this choice is theirs to revisit.
    #[default]
    Ignore,
    /// serve it, but log it and report the budget as an incomplete accounting
    Warn,
    /// refuse at admission with 402: a model with no price cannot be accounted
    /// for, so serving it would spend money the budget will never see
    Block,
    /// a policy this build does not know, read from a newer control plane's
    /// snapshot (#3347). Ordered above [`UnpricedPolicy::Block`] and refused
    /// like it, so it is never loosened by a narrower scope
    #[serde(other)]
    Unknown,
}

/// A spend cap applied to a scope over a rolling [`BudgetPeriod`]. The gateway
/// blocks a request when any matching budget's tracked spend has reached its
/// limit (most-restrictive-wins across the scope chain).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BudgetConfig {
    pub scope: BudgetScope,
    /// id of the scoped entity (org/team/project/virtual-key id), matched
    /// against the request's key scope chain
    pub id: String,
    /// spend cap for the window, denominated in the deployment's base currency
    /// despite the field name — `_usd` predates multi-currency support and
    /// renaming it is a wire change #967 leaves to its own slice.
    ///
    /// `Decimal`, so that `spend >= limit` has an exact boundary and two nodes
    /// summing the same requests cannot disagree about whether the cap was
    /// crossed (#967). The Postgres column behind it is already
    /// `numeric(12,4)`. JSON numbers on the wire, for the reason
    /// [`ModelPriceConfig`] gives.
    #[serde(with = "rust_decimal::serde::float")]
    pub limit_usd: Decimal,
    #[serde(default)]
    pub period: BudgetPeriod,
    /// this budget's own answer to unpriced traffic, overriding the
    /// deployment-wide [`UnpricedPolicy`]; `None` inherits it (#996).
    ///
    /// Resolution is most-restrictive-wins across the scope chain *and*
    /// against the deployment setting, the same way the caps themselves
    /// compose: a project budget can tighten what the org asked for but never
    /// loosen it. [`UnpricedPolicy`] derives `Ord` in that order, so the
    /// resolution is a `max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpriced_policy: Option<UnpricedPolicy>,
    /// narrows a [`BudgetScope::User`] budget to the keys the person minted in
    /// one org or project; `None` counts all of them (#1830)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<ScopeWithin>,
}

impl BudgetConfig {
    /// The Redis key holding this budget's spend counter for the window that
    /// contains `now`.
    ///
    /// The gateway writes and reads it, and the control plane reads it to show
    /// the spend so far, so the layout lives here once.
    pub fn spend_key(&self, now: DateTime<Utc>) -> String {
        match &self.within {
            None => format!(
                "rolter:budget:{}:{}:{}",
                self.scope.key_label(),
                self.id,
                self.period.bucket(now)
            ),
            Some(within) => format!(
                "rolter:budget:{}:{}:{}:{}",
                self.scope.key_label(),
                self.id,
                within.key_segment(),
                self.period.bucket(now)
            ),
        }
    }
}

/// A throughput cap applied to a scope over a rolling one-minute window. The
/// gateway rejects a request with 429 (+ `retry-after`) when a matching limit's
/// sliding-window count has reached `rpm` (requests) or `tpm` (tokens);
/// most-restrictive-wins across the scope chain. At least one of `rpm`/`tpm`
/// should be set — a limit with neither never blocks.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RateLimitConfig {
    pub scope: BudgetScope,
    /// id of the scoped entity (org/team/project/virtual-key id), matched
    /// against the request's key scope chain
    pub id: String,
    /// requests-per-minute cap; unset means requests are not capped
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,
    /// tokens-per-minute cap (prompt + completion); unset means tokens are not
    /// capped
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u32>,
    /// narrows a [`BudgetScope::User`] limit to the keys the person minted in
    /// one org or project; `None` counts all of them (#1830)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<ScopeWithin>,
}

/// Upstream retry policy. On a transient upstream failure (HTTP 408/429/5xx or a
/// connection error) the gateway re-picks a target (excluding ones already tried,
/// so retries fail over to sibling targets when available) and forwards again,
/// up to `max_retries` extra attempts. Backoff is exponential with full jitter
/// between `base_backoff_ms` and `max_backoff_ms`; a 429 `Retry-After` header
/// overrides the computed delay. Retries only happen before any body bytes have
/// been streamed to the client, so no partial response is ever duplicated.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RetryConfig {
    /// extra attempts after the first (0 disables retries)
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// base backoff in milliseconds for the first retry
    #[serde(default = "default_base_backoff_ms")]
    pub base_backoff_ms: u64,
    /// ceiling for the backoff delay in milliseconds
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            base_backoff_ms: default_base_backoff_ms(),
            max_backoff_ms: default_max_backoff_ms(),
        }
    }
}

fn default_max_retries() -> u32 {
    2
}

fn default_base_backoff_ms() -> u64 {
    100
}

fn default_max_backoff_ms() -> u64 {
    2_000
}

impl RetryConfig {
    /// Backoff delay in milliseconds for retry `attempt` (1-based), exponential
    /// with full jitter capped at `max_backoff_ms`. `rand01` is a uniform sample
    /// in `[0, 1)` supplied by the caller (keeps this pure and testable).
    pub fn backoff_ms(&self, attempt: u32, rand01: f64) -> u64 {
        let exp = self
            .base_backoff_ms
            .saturating_mul(1u64 << attempt.min(16).saturating_sub(1));
        let capped = exp.min(self.max_backoff_ms);
        // full jitter: sleep a random amount in [0, capped]
        (capped as f64 * rand01.clamp(0.0, 1.0)) as u64
    }
}

/// Per-target cooldown policy. After a target returns a transient failure
/// (HTTP 429/5xx or a connection error) it is parked for `base_secs` (or the
/// 429 `Retry-After`, capped at `max_secs`); while parked the balancer skips it
/// so load shifts to healthy siblings. When every sibling is parked the gateway
/// fails open and still forwards, rather than rejecting the request. Set
/// `base_secs = 0` to disable cooldowns.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CooldownConfig {
    /// how long a failing target is parked, in seconds (0 disables cooldowns)
    #[serde(default = "default_cooldown_base_secs")]
    pub base_secs: u64,
    /// ceiling for a cooldown derived from a 429 `Retry-After`, in seconds
    #[serde(default = "default_cooldown_max_secs")]
    pub max_secs: u64,
}

impl Default for CooldownConfig {
    fn default() -> Self {
        Self {
            base_secs: default_cooldown_base_secs(),
            max_secs: default_cooldown_max_secs(),
        }
    }
}

fn default_cooldown_base_secs() -> u64 {
    5
}

fn default_cooldown_max_secs() -> u64 {
    300
}

impl CooldownConfig {
    /// Whether cooldowns are active.
    pub fn enabled(&self) -> bool {
        self.base_secs > 0
    }

    /// Cooldown duration in seconds for a failure. `retry_after_secs` is the
    /// upstream 429 hint when present; it is honoured but capped at `max_secs`.
    pub fn duration_secs(&self, retry_after_secs: Option<u64>) -> u64 {
        match retry_after_secs {
            Some(ra) => ra.max(self.base_secs).min(self.max_secs),
            None => self.base_secs,
        }
    }
}

/// Global exact-match response-cache policy. Off by default; caching a route
/// also requires that route to opt in via [`RouteCache`]. Backed by the same
/// Redis the budget/rate-limit enforcers use; with no Redis it stays disabled
/// and every request is a miss (fail open).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CacheConfig {
    /// master switch; when false no route caches regardless of its opt-in
    #[serde(default)]
    pub enabled: bool,
    /// default entry TTL in seconds, overridable per route
    #[serde(default = "default_cache_ttl_secs")]
    pub default_ttl_secs: u64,
    /// Redis key prefix for cache entries, so they're easy to scope/flush
    #[serde(default = "default_cache_namespace")]
    pub namespace: String,
    /// largest response body (bytes) that may be stored; larger responses are
    /// served normally but not cached. guards against a single huge streamed
    /// completion evicting or bloating the cache. `0` means no limit
    #[serde(default = "default_cache_max_entry_bytes")]
    pub max_entry_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            default_ttl_secs: default_cache_ttl_secs(),
            namespace: default_cache_namespace(),
            max_entry_bytes: default_cache_max_entry_bytes(),
        }
    }
}

fn default_cache_ttl_secs() -> u64 {
    60
}

fn default_cache_max_entry_bytes() -> u64 {
    // 1 MiB: comfortably fits typical chat/JSON and buffered SSE completions
    // without letting a pathological response dominate the cache
    1024 * 1024
}

fn default_cache_namespace() -> String {
    "rolter:cache".to_string()
}

/// Process-local routing registry for OpenAI Responses lifecycle resources.
/// Records are bounded and expire lazily; deployments with multiple gateway
/// replicas must keep a response's lifecycle requests sticky to the replica
/// that accepted its creation request.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResponsesConfig {
    /// lifetime of a routing record after creation; 0 disables registration
    #[serde(default = "default_response_registry_ttl_secs")]
    pub registry_ttl_secs: u64,
    /// maximum live records retained by one gateway process
    #[serde(default = "default_response_registry_max_entries")]
    pub registry_max_entries: usize,
}

impl Default for ResponsesConfig {
    fn default() -> Self {
        Self {
            registry_ttl_secs: default_response_registry_ttl_secs(),
            registry_max_entries: default_response_registry_max_entries(),
        }
    }
}

fn default_response_registry_ttl_secs() -> u64 {
    24 * 60 * 60
}

fn default_response_registry_max_entries() -> usize {
    100_000
}

/// Per-route response-cache opt-in.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouteCache {
    /// whether this route caches (the global `[cache]` switch must also be on)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// entry TTL override in seconds; falls back to the global default when unset
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    /// isolate cached entries by virtual key so responses aren't shared across
    /// keys; when false all callers of the route share the cache
    #[serde(default)]
    pub per_key: bool,
    /// optional semantic lookup layered after exact-match lookup
    #[serde(default)]
    pub semantic: Option<SemanticCacheConfig>,
}

/// Per-route semantic response-cache settings. The embeddings provider is a
/// normal configured provider and therefore reuses its auth, proxy, and CA
/// policy. Candidate scans are explicitly bounded for predictable latency.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SemanticCacheConfig {
    /// provider name used for `/v1/embeddings`
    pub provider: String,
    /// embeddings model sent upstream
    pub model: String,
    /// minimum cosine similarity for a hit
    #[serde(default = "default_semantic_threshold")]
    pub threshold: f32,
    /// most recent semantic entries compared per request
    #[serde(default = "default_semantic_max_candidates")]
    pub max_candidates: usize,
}

fn default_semantic_threshold() -> f32 {
    0.92
}

fn default_semantic_max_candidates() -> usize {
    256
}

fn default_true() -> bool {
    true
}

/// Upstream timeout policy. `connect_secs` bounds establishing the TCP/TLS
/// connection; `request_secs` bounds time-to-response-headers (not the body), so
/// a hung upstream is abandoned without killing legitimately long SSE streams. A
/// timeout surfaces as a transient upstream error, so it feeds the retry and
/// cooldown machinery. Set a field to 0 to disable that timeout.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TimeoutConfig {
    /// connection-establishment timeout in seconds (0 disables)
    #[serde(default = "default_connect_secs")]
    pub connect_secs: u64,
    /// time-to-response-headers timeout in seconds (0 disables)
    #[serde(default = "default_request_secs")]
    pub request_secs: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect_secs: default_connect_secs(),
            request_secs: default_request_secs(),
        }
    }
}

/// Cross-dialect compatibility behavior applied when a request is translated
/// between the OpenAI and Anthropic wire formats. Both values were compiled-in
/// constants before the control plane could own them (#546); the defaults keep
/// the previous behavior.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompatibilityConfig {
    /// `anthropic-version` header sent to Anthropic upstreams
    #[serde(default = "default_anthropic_version")]
    pub anthropic_version: String,
    /// `max_tokens` injected when translating an OpenAI request that set
    /// neither `max_tokens` nor `max_completion_tokens` — the Anthropic
    /// Messages API rejects a request without it
    #[serde(default = "default_translation_max_tokens")]
    pub default_max_tokens: u32,
}

impl Default for CompatibilityConfig {
    fn default() -> Self {
        Self {
            anthropic_version: default_anthropic_version(),
            default_max_tokens: default_translation_max_tokens(),
        }
    }
}

/// The header carrying the end-to-end request id when `[client]
/// request_id_header` is not set.
pub const DEFAULT_REQUEST_ID_HEADER: &str = "x-request-id";

fn default_request_id_header() -> String {
    DEFAULT_REQUEST_ID_HEADER.to_string()
}

/// Header names that carry session state, refused as the request id header on
/// top of [`RESERVED_ROUTE_HEADERS`].
const SESSION_HEADERS: [&str; 2] = ["cookie", "set-cookie"];

/// Every name [`request_id_header_problem`] refuses for a reason other than
/// being malformed, sorted and without duplicates.
///
/// The control plane returns it with the client settings so the dashboard can
/// check the field inline instead of waiting for the save to be refused.
pub fn request_id_refused_headers() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = RESERVED_ROUTE_HEADERS
        .iter()
        .chain(SESSION_HEADERS.iter())
        .copied()
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Why `name` cannot be the request id header, or `None` when it can.
///
/// The gateway reads the caller's id from this header, writes the id it
/// mints back onto the request, echoes the id on the response and stores it
/// in the request log. A name in [`RESERVED_ROUTE_HEADERS`] carries a
/// credential or frames the message, and `cookie` / `set-cookie` carry
/// session state. Reading one would copy a secret into the log and onto the
/// response, and writing one would corrupt the message the id is written
/// over, so they are refused here. The control plane checks the same rule on
/// write and the gateway checks it again when it applies the setting, so all
/// three agree on what is usable.
pub fn request_id_header_problem(name: &str) -> Option<String> {
    if !is_header_name(name) {
        return Some(format!("'{name}' is not a valid header name"));
    }
    if is_reserved_route_header(name)
        || SESSION_HEADERS
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
    {
        return Some(format!(
            "'{name}' carries a credential, session state or message framing, so it cannot \
             carry the request id"
        ));
    }
    None
}

/// What the gateway does with headers on the upstream leg, plus the base URL
/// the dashboard advertises in its connection snippets (#564).
///
/// The defaults are empty, so a deployment that never touches the Client
/// Settings screen sends exactly the headers it sent before this existed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClientConfig {
    /// base URL shown to clients; `None` means none is saved, and the
    /// dashboard then shows no connection snippet rather than falling back to
    /// its own `/gw` proxy, which needs a dashboard session. Advisory only: the
    /// gateway never reads it. Every signed-in caller can read it through
    /// `gateway_base_url` on `GET /api/v1/auth/me`, though the rest of client
    /// settings stays superadmin-only
    #[serde(default)]
    pub public_base_url: Option<String>,
    /// inbound client headers forwarded verbatim to the upstream provider,
    /// lowercased. Trace-context headers propagate regardless of this list
    #[serde(default)]
    pub forwarded_headers: Vec<String>,
    /// static headers added to every upstream request, lowercased names
    #[serde(default)]
    pub injected_headers: BTreeMap<String, String>,
    /// header carrying the end-to-end request id: the gateway reads the
    /// caller's id from it, mints one when it is absent and echoes it on the
    /// response. Must pass [`request_id_header_problem`]
    #[serde(default = "default_request_id_header")]
    pub request_id_header: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            public_base_url: None,
            forwarded_headers: Vec::new(),
            injected_headers: BTreeMap::new(),
            request_id_header: default_request_id_header(),
        }
    }
}

/// Deployment-wide ingress policy, owned by the control plane's Security
/// screen and carried to every gateway in the snapshot (#1162).
///
/// There is no "enforce virtual keys" rule here (#2357). Whether a gateway
/// holding no keys refuses a request is decided by
/// [`ServerConfig::require_auth`] and by how the gateway was started: a
/// managed gateway fails closed, a file-configured one stays open for local
/// development. A dashboard switch only ever reached managed gateways, which
/// were already closed, so it changed nothing and was removed.
///
/// Everything here is a *refusal* rule: each field can only make the gateway
/// reject a request it would otherwise have served, or — for
/// [`Self::auth_bypass_routes`] — serve a path the operator explicitly named.
/// That is deliberate, and it is why the default, "no extra rules", must never
/// stand in for a value that could not be read: that would silently open a
/// deployment it was closing. The store therefore refuses to build a config at
/// all while `required_headers` is unreadable, so no snapshot goes out and a
/// gateway keeps the policy it last received (#2943).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityPolicyConfig {
    /// lowercase header name -> exact value every request must carry. A
    /// request missing one, or carrying a different value, is refused before
    /// authentication — this is an ingress filter (a mesh identity header, a
    /// WAF marker), not an authorization decision
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub required_headers: BTreeMap<String, String>,
    /// exact `/v1/...` paths served without authentication. Exact match only:
    /// no prefixes and no wildcards, so a bypass can never widen by accident
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_bypass_routes: Vec<String>,
}

impl SecurityPolicyConfig {
    /// Whether `path` was explicitly named as unauthenticated.
    pub fn bypasses_auth(&self, path: &str) -> bool {
        self.auth_bypass_routes.iter().any(|route| route == path)
    }

    /// The first required header this request does not satisfy, if any.
    /// Returns the header name so the refusal can name it — the *value* is
    /// never echoed, since an operator may well have put a shared secret there.
    pub fn missing_required_header<'a, F>(&'a self, get: F) -> Option<&'a str>
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        self.required_headers
            .iter()
            .find(|(name, expected)| get(name).is_none_or(|actual| actual != expected.as_str()))
            .map(|(name, _)| name.as_str())
    }
}

/// Inference parameters the gateway fills in when the client omitted them
/// (#564).
///
/// Every field is opt-in and only ever *fills a gap*: a value the client sent
/// is never overwritten, so enabling this cannot change the meaning of an
/// existing request that was already explicit.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct ModelDefaultsConfig {
    /// master switch; with it off the request body is forwarded untouched
    #[serde(default)]
    pub enabled: bool,
    /// `model` used when the request omits one
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

impl ModelDefaultsConfig {
    /// Whether applying this config could change any request body.
    pub fn is_active(&self) -> bool {
        self.enabled
            && (self.default_model.is_some()
                || self.temperature.is_some()
                || self.top_p.is_some()
                || self.max_tokens.is_some())
    }
}

/// Adaptive routing: a blend of observed latency, catalog cost and in-flight
/// load applied by the `adaptive` strategy (#544).
///
/// The whole feature is off by default. `enabled` is the kill switch: with it
/// off — or before a route has gathered enough evidence to rank its targets —
/// the `adaptive` strategy behaves exactly like the `pipeline` stack, so a
/// route can be moved onto it without shifting any traffic until an operator
/// says so.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct AdaptiveRoutingConfig {
    /// master kill switch; when false every `adaptive` route serves the
    /// deterministic fallback stack
    #[serde(default)]
    pub enabled: bool,
    /// weight of the observed-latency signal
    #[serde(default = "default_adaptive_latency_weight")]
    pub latency_weight: f32,
    /// weight of the catalog-cost signal
    #[serde(default = "default_adaptive_cost_weight")]
    pub cost_weight: f32,
    /// weight of the in-flight-load signal
    #[serde(default = "default_adaptive_load_weight")]
    pub load_weight: f32,
    /// share of picks made uniformly at random so a target the blend would
    /// otherwise starve keeps producing fresh latency samples
    #[serde(default = "default_adaptive_exploration_ratio")]
    pub exploration_ratio: f32,
    /// picks a route must have served before adaptive scoring engages
    #[serde(default = "default_adaptive_min_samples")]
    pub min_samples: u32,
}

impl Default for AdaptiveRoutingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            latency_weight: default_adaptive_latency_weight(),
            cost_weight: default_adaptive_cost_weight(),
            load_weight: default_adaptive_load_weight(),
            exploration_ratio: default_adaptive_exploration_ratio(),
            min_samples: default_adaptive_min_samples(),
        }
    }
}

impl AdaptiveRoutingConfig {
    /// Clamp operator input into the ranges the balancer assumes: weights are
    /// non-negative and exploration never takes more than half the traffic, so
    /// a typo cannot turn the route into a random balancer.
    pub fn sanitized(&self) -> Self {
        Self {
            enabled: self.enabled,
            latency_weight: self.latency_weight.max(0.0),
            cost_weight: self.cost_weight.max(0.0),
            load_weight: self.load_weight.max(0.0),
            exploration_ratio: self.exploration_ratio.clamp(0.0, MAX_EXPLORATION_RATIO),
            min_samples: self.min_samples,
        }
    }

    /// Whether the blend can rank anything at all. All-zero weights would make
    /// every candidate score identically, which is a random balancer wearing an
    /// adaptive name — treat it as "not configured" instead.
    pub fn has_signal(&self) -> bool {
        self.latency_weight > 0.0 || self.cost_weight > 0.0 || self.load_weight > 0.0
    }
}

/// Upper bound on [`AdaptiveRoutingConfig::exploration_ratio`].
pub const MAX_EXPLORATION_RATIO: f32 = 0.5;

fn default_adaptive_latency_weight() -> f32 {
    1.0
}

fn default_adaptive_cost_weight() -> f32 {
    0.5
}

fn default_adaptive_load_weight() -> f32 {
    0.25
}

fn default_adaptive_exploration_ratio() -> f32 {
    0.05
}

fn default_adaptive_min_samples() -> u32 {
    50
}

fn default_anthropic_version() -> String {
    // one source for the dated version, shared with the health probes;
    // spec-ref: anthropic-messages
    crate::probe::ANTHROPIC_VERSION.to_string()
}

fn default_translation_max_tokens() -> u32 {
    1024
}

fn default_connect_secs() -> u64 {
    10
}

fn default_request_secs() -> u64 {
    60
}

/// Action taken when a provider's bounded dispatch queue cannot accept a
/// request. `drop` and `error` are immediate; `block` waits up to
/// [`QueueConfig::block_timeout_ms`] for a worker to become available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackpressurePolicy {
    /// immediately shed the request with an overload response
    Drop,
    /// wait for capacity until `block_timeout_ms` expires
    Block,
    /// immediately return a structured `queue_full` error
    #[default]
    Error,
    /// a policy this build does not know (#3347); read as
    /// [`BackpressurePolicy::Error`], which sheds rather than waits
    #[serde(other)]
    Unknown,
}

/// Per-provider upstream dispatch controls. Rolter creates an independent
/// bounded queue and worker set for each configured provider, so a slow
/// provider cannot consume the admission capacity of healthy providers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct QueueConfig {
    /// master switch; disabling restores direct forwarding
    #[serde(default = "default_queue_enabled")]
    pub enabled: bool,
    /// maximum requests waiting for one provider, excluding active workers
    #[serde(default = "default_queue_capacity")]
    pub capacity: usize,
    /// concurrent upstream header requests allowed for each provider
    #[serde(default = "default_queue_workers")]
    pub workers: usize,
    /// behaviour when a provider queue has no free slot
    #[serde(default)]
    pub backpressure: BackpressurePolicy,
    /// maximum wait for `backpressure = "block"`; zero disables the wait
    #[serde(default = "default_queue_block_timeout_ms")]
    pub block_timeout_ms: u64,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            enabled: default_queue_enabled(),
            capacity: default_queue_capacity(),
            workers: default_queue_workers(),
            backpressure: BackpressurePolicy::default(),
            block_timeout_ms: default_queue_block_timeout_ms(),
        }
    }
}

fn default_queue_enabled() -> bool {
    true
}

fn default_queue_capacity() -> usize {
    256
}

fn default_queue_workers() -> usize {
    8
}

fn default_queue_block_timeout_ms() -> u64 {
    1_000
}

/// Active upstream health probing. When `enabled`, a background task periodically
/// issues a lightweight `GET {api_base}{path}` to each provider; a provider that
/// times out, fails to connect, or answers `5xx` is marked unhealthy and the
/// balancer skips its targets until a later probe recovers it. Reachability with
/// any non-5xx status (including 401/404) counts as healthy, since upstreams
/// rarely expose a dedicated health route. Disabled by default — probing adds
/// background traffic and is most useful for self-hosted pools. When every target
/// of a route is unhealthy the gateway fails open rather than rejecting.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HealthConfig {
    /// master switch for active probing
    #[serde(default)]
    pub enabled: bool,
    /// seconds between probe sweeps
    #[serde(default = "default_health_interval_secs")]
    pub interval_secs: u64,
    /// per-probe timeout in seconds
    #[serde(default = "default_health_timeout_secs")]
    pub timeout_secs: u64,
    /// request path appended to each provider's `api_base` when probing
    #[serde(default = "default_health_path")]
    pub path: String,
    /// maximum probes in flight at once during a sweep
    #[serde(default = "default_health_probe_concurrency")]
    pub probe_concurrency: usize,
    /// consecutive probe failures before a provider is marked unhealthy
    #[serde(default = "default_health_failure_threshold")]
    pub consecutive_failure_threshold: u32,
    /// consecutive probe successes before an unhealthy provider recovers
    #[serde(default = "default_health_recovery_threshold")]
    pub recovery_success_threshold: u32,
    /// seconds between provider status-page polls (a slow, secondary signal);
    /// only providers with a `status_page_url` are polled
    #[serde(default = "default_status_page_interval_secs")]
    pub status_page_interval_secs: u64,
    /// seconds between refreshes of each provider's model catalogue, which
    /// `GET /v1/models` lists as `provider-slug/model`; independent of
    /// `enabled`, `0` turns the refresh off
    #[serde(default = "default_catalogue_interval_secs")]
    pub catalogue_interval_secs: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_health_interval_secs(),
            timeout_secs: default_health_timeout_secs(),
            path: default_health_path(),
            probe_concurrency: default_health_probe_concurrency(),
            consecutive_failure_threshold: default_health_failure_threshold(),
            recovery_success_threshold: default_health_recovery_threshold(),
            status_page_interval_secs: default_status_page_interval_secs(),
            catalogue_interval_secs: default_catalogue_interval_secs(),
        }
    }
}

fn default_status_page_interval_secs() -> u64 {
    60
}

fn default_catalogue_interval_secs() -> u64 {
    900
}

fn default_health_interval_secs() -> u64 {
    10
}

fn default_health_timeout_secs() -> u64 {
    2
}

fn default_health_path() -> String {
    "/".to_string()
}

fn default_health_probe_concurrency() -> usize {
    2
}

fn default_health_failure_threshold() -> u32 {
    3
}

fn default_health_recovery_threshold() -> u32 {
    2
}

/// Background scrape of each upstream engine's Prometheus `/metrics`. When
/// enabled, a task periodically pulls per-provider queue depth (vLLM/SGLang/TGI
/// `num_requests_waiting`) into a lock-free snapshot the balancer folds into its
/// in-flight load view, so load-aware strategies steer away from backed-up
/// engines. Disabled by default; the snapshot reports zero depth when inert.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsScrapeConfig {
    /// master switch for background metrics scraping
    #[serde(default)]
    pub enabled: bool,
    /// seconds between scrape sweeps
    #[serde(default = "default_scrape_interval_secs")]
    pub interval_secs: u64,
    /// per-scrape timeout in seconds
    #[serde(default = "default_scrape_timeout_secs")]
    pub timeout_secs: u64,
    /// request path appended to each provider's `api_base` when scraping
    #[serde(default = "default_scrape_path")]
    pub path: String,
}

impl Default for MetricsScrapeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_scrape_interval_secs(),
            timeout_secs: default_scrape_timeout_secs(),
            path: default_scrape_path(),
        }
    }
}

fn default_scrape_interval_secs() -> u64 {
    5
}

fn default_scrape_timeout_secs() -> u64 {
    2
}

fn default_scrape_path() -> String {
    "/metrics".to_string()
}

/// Per-target circuit breaker. Complements the short per-failure [`CooldownConfig`]
/// with a longer-lived state machine: after `failure_threshold` consecutive
/// transient failures a target trips **open** and is skipped for `open_secs`;
/// the first request after that window probes it (**half-open**), and a success
/// closes the breaker while another failure re-opens it. Where a cooldown parks a
/// target for one wobble, the breaker sheds sustained load off a target that is
/// down hard. Disabled by default (`enabled = false`); when every target of a
/// route is open the gateway fails open rather than rejecting.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BreakerConfig {
    /// master switch for the circuit breaker
    #[serde(default)]
    pub enabled: bool,
    /// consecutive transient failures that trip a closed target open
    #[serde(default = "default_breaker_failure_threshold")]
    pub failure_threshold: u32,
    /// how long a tripped target stays open before a half-open probe, in seconds
    #[serde(default = "default_breaker_open_secs")]
    pub open_secs: u64,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_threshold: default_breaker_failure_threshold(),
            open_secs: default_breaker_open_secs(),
        }
    }
}

fn default_breaker_failure_threshold() -> u32 {
    5
}

fn default_breaker_open_secs() -> u64 {
    30
}

impl BreakerConfig {
    /// Whether the breaker is active. A zero `failure_threshold` also disables it,
    /// since a target could never accumulate enough failures to trip.
    pub fn enabled(&self) -> bool {
        self.enabled && self.failure_threshold > 0
    }
}

/// Where request and cost logs are written.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LoggingConfig {
    /// base url of the clickhouse http interface, e.g. `http://clickhouse:8123`;
    /// logging is disabled when unset
    #[serde(default)]
    pub clickhouse_url: Option<String>,
    /// flush a batch once it reaches this many records
    #[serde(default = "default_log_batch_max")]
    pub batch_max: usize,
    /// flush a partial batch at least this often, in milliseconds
    #[serde(default = "default_log_flush_ms")]
    pub flush_ms: u64,
    /// bounded in-flight queue; records are dropped (counted) when it is full so
    /// the request hot path never blocks on the log writer
    #[serde(default = "default_log_queue_capacity")]
    pub queue_capacity: usize,
    /// request-log sampling rate in `[0, 1]` applied before write. `1` keeps
    /// every record, `0` disables request-log rows entirely.
    #[serde(default = "default_log_sample_rate")]
    pub sample_rate: f64,
    /// optional raw request/response capture. Disabled by default because
    /// payloads can contain prompts, completions, and other sensitive data.
    #[serde(default)]
    pub payload_capture: PayloadCaptureConfig,
    /// dashboard UX events (#805): screen views, time-to-interactive, form
    /// abandons, error states. On by default, unlike `payload_capture`, because
    /// the `ui_events` schema is structural only — every column is a key, an
    /// enum, a duration or an id, so there is nowhere for a form value or
    /// prompt to land. Set `false` to drop them at the ingest endpoint; the
    /// stream is inert anyway without `clickhouse_url`.
    #[serde(default = "default_ui_events")]
    pub ui_events: bool,
}

fn default_ui_events() -> bool {
    true
}

/// Bounded queue that carries post-response budget spend and rate-limit token
/// recording off the response path.
///
/// Recording writes to Redis, so it must not run inline — but firing an
/// unbounded detached task per request is a queue with infinite depth: when the
/// counter store slows down the tasks pile up without limit, and the pile-up is
/// worst exactly when the system is already under stress. This bounds it the
/// same way the request-log queue is bounded, with an explicit drop policy and a
/// counter, so overflow is observable rather than silent (#1051).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct UsageRecordingConfig {
    /// records waiting to be written; the oldest are never blocked on, new
    /// records are dropped (and counted) once it is full
    #[serde(default = "default_usage_queue_capacity")]
    pub queue_capacity: usize,
    /// concurrent writers draining the queue. More than one because each record
    /// is a Redis round trip, so a single consumer would serialise them
    #[serde(default = "default_usage_workers")]
    pub workers: usize,
}

impl Default for UsageRecordingConfig {
    fn default() -> Self {
        Self {
            queue_capacity: default_usage_queue_capacity(),
            workers: default_usage_workers(),
        }
    }
}

fn default_usage_queue_capacity() -> usize {
    10_000
}

fn default_usage_workers() -> usize {
    4
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            clickhouse_url: None,
            batch_max: default_log_batch_max(),
            flush_ms: default_log_flush_ms(),
            queue_capacity: default_log_queue_capacity(),
            sample_rate: default_log_sample_rate(),
            payload_capture: PayloadCaptureConfig::default(),
            ui_events: default_ui_events(),
        }
    }
}

/// Guardrails for opt-in raw invocation payload capture.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PayloadCaptureConfig {
    /// master switch; capture is disabled unless this is explicitly true
    #[serde(default)]
    pub enabled: bool,
    /// maximum number of UTF-8 bytes retained for each request or response
    #[serde(default = "default_payload_capture_max_bytes")]
    pub max_bytes: usize,
    /// JSON object keys whose values are replaced before storage. Matching is
    /// case-insensitive and recursive, covering nested tool arguments too.
    #[serde(default = "default_payload_redact_fields")]
    pub redact_fields: Vec<String>,
    /// built-in value detectors (see [`crate::payload_redaction::detector_names`])
    /// that scan the text of every captured string and replace a match with a
    /// typed marker such as `[redacted:aws_key]` (#1835). All on by default;
    /// an empty list turns the built-ins off.
    #[serde(default = "crate::payload_redaction::detector_names")]
    pub redact_detectors: Vec<String>,
    /// operator-supplied value patterns applied after the built-in detectors
    #[serde(default)]
    pub redact_patterns: Vec<crate::payload_redaction::RedactPattern>,
    /// optional public-model allow-list. Empty captures every route when
    /// enabled; non-empty limits capture to the named routes.
    #[serde(default)]
    pub models: Vec<String>,
    /// optional database virtual-key id allow-list. Empty captures every key
    /// allowed by `models`; non-empty makes capture key-specific.
    #[serde(default)]
    pub virtual_key_ids: Vec<String>,
}

impl Default for PayloadCaptureConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: default_payload_capture_max_bytes(),
            redact_fields: default_payload_redact_fields(),
            redact_detectors: crate::payload_redaction::detector_names(),
            redact_patterns: Vec::new(),
            models: Vec::new(),
            virtual_key_ids: Vec::new(),
        }
    }
}

impl PayloadCaptureConfig {
    /// Compile the redaction policy. A value policy that does not compile (a
    /// hand-edited file; the control plane validates what it stores) degrades to
    /// the key rule alone rather than to no redaction, and the error is returned
    /// for the caller to log.
    pub fn redactor(
        &self,
    ) -> (
        crate::payload_redaction::PayloadRedactor,
        Option<crate::payload_redaction::RedactionPolicyError>,
    ) {
        match crate::payload_redaction::PayloadRedactor::compile(
            &self.redact_fields,
            &self.redact_detectors,
            &self.redact_patterns,
        ) {
            Ok(redactor) => (redactor, None),
            Err(err) => (
                crate::payload_redaction::PayloadRedactor::fields_only(&self.redact_fields),
                Some(err),
            ),
        }
    }
}

fn default_payload_capture_max_bytes() -> usize {
    32 * 1024
}

fn default_payload_redact_fields() -> Vec<String> {
    vec![
        "api_key".to_string(),
        "authorization".to_string(),
        "password".to_string(),
        "secret".to_string(),
        "token".to_string(),
    ]
}

fn default_log_batch_max() -> usize {
    1000
}

fn default_log_flush_ms() -> u64 {
    1000
}

fn default_log_queue_capacity() -> usize {
    10_000
}

fn default_log_sample_rate() -> f64 {
    1.0
}

/// Effective slug for a provider: an explicit slug wins, else derive from name.
fn provider_slug(p: &ProviderConfig) -> String {
    p.slug
        .clone()
        .unwrap_or_else(|| crate::slug::slugify(&p.name))
}

/// Remove the retired `security.virtual_key_required` key from a parsed
/// document, returning its value when it was there (#2357).
///
/// Shared by the loader, which honours a `true` as `server.require_auth`, and
/// by [`crate::config_lint`], which must not report the key as silently ignored
/// when the loader did act on it.
pub(crate) fn take_retired_virtual_key_required(doc: &mut toml::Table) -> Option<toml::Value> {
    doc.get_mut("security")?
        .as_table_mut()?
        .remove("virtual_key_required")
}

/// Carry a file's retired `security.virtual_key_required` forward.
///
/// The key used to close a file-configured gateway that held no keys, which is
/// exactly what `server.require_auth = true` does. Dropping it on the floor
/// would open such a gateway on upgrade, so a `true` becomes
/// `require_auth = true` unless the file already says otherwise there, and the
/// operator is told to move it either way.
fn fold_retired_virtual_key_required(server: &mut ServerConfig, value: &toml::Value) {
    if value.as_bool() == Some(true) && server.require_auth.is_none() {
        server.require_auth = Some(true);
        tracing::warn!(
            "security.virtual_key_required is retired (#2357); it is honoured as \
             server.require_auth = true. move the setting to [server] require_auth"
        );
    } else {
        // `false` was the default, and an explicit `require_auth` always won
        // over the key, so in both cases ignoring it changes nothing
        tracing::warn!(
            "security.virtual_key_required is retired (#2357) and has no effect here; \
             remove it. [server] require_auth decides whether a gateway holding no \
             keys refuses requests"
        );
    }
}

/// One problem line per enum value in a file config that this build does not
/// know (#3347). A file is written by hand, where an unknown value is a typo
/// and must not quietly take its fail-closed reading.
fn unknown_value<T: KnownValues>(
    problems: &mut Vec<String>,
    value: &T,
    what: impl FnOnce() -> String,
) {
    if value.is_unknown() {
        problems.push(format!(
            "{} is not a value this build knows; expected one of {}",
            what(),
            T::spellings()
        ));
    }
}

/// An enum of the snapshot with an `Unknown` fallback (#3347), and the values
/// a config file may spell, so a typo can be answered with the list.
trait KnownValues {
    /// whether this is the fallback rather than a value this build knows
    fn is_unknown(&self) -> bool;
    /// every value this build knows, as a file spells it
    fn spellings() -> String;
}

/// [`KnownValues`] for an enum, from the list of its known variants. The
/// `match` beside the list is exhaustive, so a variant added to the enum and
/// not to the list fails to compile.
macro_rules! known_values {
    ($ty:ident { $($variant:ident),+ $(,)? }) => {
        impl KnownValues for $ty {
            fn is_unknown(&self) -> bool {
                *self == $ty::Unknown
            }

            fn spellings() -> String {
                const KNOWN: &[$ty] = &[$($ty::$variant),+];
                spell(KNOWN)
            }
        }

        const _: fn($ty) = |value| match value {
            $($ty::$variant)|+ | $ty::Unknown => {}
        };
    };
}

/// The spellings of `values`, as serde writes them, joined for a sentence.
fn spell<T: Serialize>(values: &[T]) -> String {
    values
        .iter()
        .filter_map(|value| serde_json::to_value(value).ok())
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join(", ")
}

impl<T: KnownValues> KnownValues for Option<T> {
    fn is_unknown(&self) -> bool {
        self.as_ref().is_some_and(KnownValues::is_unknown)
    }

    fn spellings() -> String {
        T::spellings()
    }
}

impl KnownValues for ProviderKind {
    fn is_unknown(&self) -> bool {
        *self == ProviderKind::Unknown
    }

    fn spellings() -> String {
        // `ALL` is held to the enum by `kind_is_listed_in_all`
        spell(&ProviderKind::ALL)
    }
}

mod known_values {
    use super::*;
    use crate::guardrail_webhook::FailureMode;
    use crate::guardrail_webhook::GuardProviderKind;
    use crate::guardrails::{GuardAction, StreamingPostCall};
    use crate::pii_sanitizer::{RestorationPolicy, SanitizeDirection, StreamingResponse};
    use crate::semantic_guards::{StreamingStrategy, ViolationAction};

    known_values!(RoleProfile {
        Openai,
        SystemOnly,
        Anthropic
    });
    known_values!(BalancingStrategy {
        RoundRobin,
        Random,
        PowerOfTwo,
        ConsistentHash,
        CacheAware,
        Weighted,
        Pipeline,
        Cheapest,
        Fastest,
        PreciseCacheAware,
        LmcacheAware,
        Adaptive,
        LoraAware,
        PredictedLatency,
    });
    known_values!(OverrideMode { Allow, Deny });
    known_values!(BudgetScope {
        Org,
        Team,
        Project,
        Key,
        BusinessUnit,
        Customer,
        User
    });
    known_values!(BudgetPeriod {
        Monthly,
        Daily,
        Total
    });
    known_values!(UnpricedPolicy {
        Ignore,
        Warn,
        Block
    });
    known_values!(BackpressurePolicy { Drop, Block, Error });
    known_values!(FailureMode {
        FailOpen,
        FailClosed
    });
    known_values!(GuardAction {
        Annotate,
        Block,
        Redact
    });
    known_values!(StreamingPostCall {
        Reject,
        Passthrough
    });
    known_values!(SanitizeDirection {
        Request,
        Response,
        Both
    });
    known_values!(StreamingResponse {
        Reject,
        Passthrough,
        Restore
    });
    known_values!(RestorationPolicy {
        Never,
        TrustedDownstream,
        CallerAuthorized
    });
    known_values!(ViolationAction { Block, Annotate });
    known_values!(StreamingStrategy { Reject, Buffer });
    known_values!(GuardProviderKind { Webhook, Local });
}

/// Every `takes_over` class of `route` this build does not know, with the
/// provider of the target that carries it (#3390).
fn unknown_failure_classes(route: &ModelRoute) -> impl Iterator<Item = (&str, String)> {
    route
        .targets
        .iter()
        .chain(route.variants.iter().flat_map(|v| v.targets.iter()))
        .flat_map(|target| {
            target
                .takes_over
                .iter()
                .filter(|class| class.kind == crate::failover::FailureKind::Unknown)
                .map(move |class| (target.provider.as_str(), class.to_string()))
        })
}

/// Replace a value this build does not know with its fail-closed reading and
/// say so (#3347).
fn resolve_unknown<T: PartialEq + Copy>(
    value: &mut T,
    unknown: T,
    reading: T,
    problems: &mut UnknownValues,
    kind: &'static str,
    what: impl FnOnce() -> String,
) {
    if *value == unknown {
        *value = reading;
        problems.push(
            kind,
            format!(
                "{} has a value this gateway does not know; it is read as the most \
                 restrictive known value until the gateway is upgraded",
                what()
            ),
        );
    }
}

/// The values of a snapshot a gateway read fail-closed (#3347), from
/// [`GatewayConfig::fail_closed_unknown_report`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnknownValues {
    /// one line per value, plus one per entity a value made the sanitizer
    /// leave out, for the gateway's log. Names entities, which are tenant
    /// data, so it never becomes a metric label
    pub lines: Vec<String>,
    /// how many values of each kind were read fail-closed (#3424). A kind is
    /// the enum or entity a value belongs to, from a fixed vocabulary
    /// (`provider_kind`, `route_strategy`, `failure_mode`, ...), never the
    /// value itself, so it is safe as a bounded metric label
    pub kinds: std::collections::BTreeMap<&'static str, u32>,
}

impl UnknownValues {
    fn push(&mut self, kind: &'static str, line: String) {
        self.count(kind);
        self.lines.push(line);
    }

    fn count(&mut self, kind: &'static str) {
        *self.kinds.entry(kind).or_default() += 1;
    }
}

impl GatewayConfig {
    /// Read every enum value a newer control plane sent that this build does
    /// not know, failing closed for that entity only (#3347).
    ///
    /// A gateway older than its control plane decodes such a value as the
    /// enum's `Unknown` variant instead of refusing the whole snapshot, which
    /// would keep it on its last config, revoked keys included, until it is
    /// upgraded. What `Unknown` then means is decided here, in one place:
    ///
    /// - a provider of an unknown kind is never dialled: it is omitted, and
    ///   the routes, groups and keys that named it lose those references
    ///   through [`sanitize_for_snapshot`](Self::sanitize_for_snapshot)'s
    ///   cascade;
    /// - a route with an unknown strategy is omitted, and a provider group
    ///   with one keeps no members, so neither is balanced by a guess;
    /// - a setting with an unknown value takes its most restrictive known
    ///   reading (a failure mode fails closed, a guard action blocks, an
    ///   unpriced policy refuses, a restoration policy never restores);
    /// - a budget or rate limit of an unknown scope, and a budget of an unknown
    ///   period, stay `Unknown` and the gateway's enforcement treats them as
    ///   binding (see `BudgetScope::Unknown` and `BudgetPeriod::Unknown`).
    ///
    /// Returns one line per value read this way, for the gateway's log. The
    /// rest of the snapshot is untouched. Costs a walk of the config and
    /// nothing more when every value is known.
    pub fn fail_closed_unknown_values(&mut self) -> Vec<String> {
        self.fail_closed_unknown_report().lines
    }

    /// [`fail_closed_unknown_values`](Self::fail_closed_unknown_values) with
    /// a count of the values read per kind, which the gateway exports as
    /// `rolter_snapshot_unknown_values{kind}` (#3424). A budget or rate limit
    /// whose scope or period stays `Unknown` is counted, and logged, here too:
    /// it is no less a value the gateway does not know for being enforced as
    /// binding rather than rewritten.
    pub fn fail_closed_unknown_report(&mut self) -> UnknownValues {
        let mut problems = UnknownValues::default();
        self.resolve_unknown_settings(&mut problems);
        for budget in &self.budgets {
            if budget.scope == BudgetScope::Unknown {
                problems.push(
                    "budget_scope",
                    format!(
                        "budget '{}' has a scope this gateway does not know; it is enforced \
                         as binding until the gateway is upgraded",
                        budget.id
                    ),
                );
            }
            if budget.period == BudgetPeriod::Unknown {
                problems.push(
                    "budget_period",
                    format!(
                        "budget {} '{}' has a period this gateway does not know; it is \
                         enforced as binding until the gateway is upgraded",
                        budget.scope.key_label(),
                        budget.id
                    ),
                );
            }
        }
        for limit in &self.rate_limits {
            if limit.scope == BudgetScope::Unknown {
                problems.push(
                    "rate_limit_scope",
                    format!(
                        "rate limit '{}' has a scope this gateway does not know; it is \
                         enforced as binding until the gateway is upgraded",
                        limit.id
                    ),
                );
            }
        }
        // refused per request by the MCP proxy rather than rewritten, so it
        // is named here rather than in `resolve_unknown_settings`
        for server in &self.mcp_servers {
            if server.auth_kind == McpAuthKind::Unknown {
                problems.push(
                    "mcp_auth_kind",
                    format!(
                        "mcp server '{}' has an auth kind this gateway does not know; every \
                         call through it is refused until the gateway is upgraded",
                        server.slug
                    ),
                );
            }
        }
        let unknown_kinds = self
            .providers
            .iter()
            .filter(|p| p.kind == ProviderKind::Unknown)
            .count();
        let unknown_strategies = self
            .routes
            .iter()
            .filter(|r| r.strategy == BalancingStrategy::Unknown)
            .count();
        for _ in 0..unknown_kinds {
            problems.count("provider_kind");
        }
        for _ in 0..unknown_strategies {
            problems.count("route_strategy");
        }
        if unknown_kinds + unknown_strategies > 0 {
            problems.lines.extend(self.sanitize_for_snapshot());
        }
        problems
    }

    /// The settings half of
    /// [`fail_closed_unknown_values`](Self::fail_closed_unknown_values), also
    /// run first by the sanitizer so a control plane serves the same reading.
    fn resolve_unknown_settings(&mut self, problems: &mut UnknownValues) {
        use crate::guardrail_webhook::FailureMode;
        use crate::guardrails::{GuardAction, StreamingPostCall};
        use crate::pii_sanitizer::{RestorationPolicy, SanitizeDirection, StreamingResponse};
        use crate::semantic_guards::{StreamingStrategy, ViolationAction};

        resolve_unknown(
            &mut self.unpriced_policy,
            UnpricedPolicy::Unknown,
            UnpricedPolicy::Block,
            problems,
            "unpriced_policy",
            || "unpriced_policy".to_string(),
        );
        resolve_unknown(
            &mut self.queue.backpressure,
            BackpressurePolicy::Unknown,
            BackpressurePolicy::Error,
            problems,
            "queue_backpressure",
            || "queue.backpressure".to_string(),
        );
        for provider in &mut self.providers {
            let name = provider.name.clone();
            if let Some(profile) = provider.role_profile.as_mut() {
                resolve_unknown(
                    profile,
                    RoleProfile::Unknown,
                    RoleProfile::SystemOnly,
                    problems,
                    "role_profile",
                    || format!("provider '{name}' role_profile"),
                );
            }
            for (model, profile) in &mut provider.model_role_profiles {
                resolve_unknown(
                    profile,
                    RoleProfile::Unknown,
                    RoleProfile::SystemOnly,
                    problems,
                    "role_profile",
                    || format!("provider '{name}' role profile for model '{model}'"),
                );
            }
        }
        for group in &mut self.provider_groups {
            if group.strategy == BalancingStrategy::Unknown && !group.members.is_empty() {
                group.members.clear();
                problems.push(
                    "provider_group_strategy",
                    format!(
                        "provider group '{}' has a balancing strategy this gateway does not know; \
                     it keeps no members, so it resolves nothing until the gateway is upgraded",
                        group_slug(group)
                    ),
                );
            }
        }
        for route in &mut self.routes {
            let model = route.model.clone();
            resolve_unknown(
                &mut route.param_policy.mode,
                OverrideMode::Unknown,
                OverrideMode::Deny,
                problems,
                "param_policy_mode",
                || format!("route '{model}' param_policy.mode"),
            );
            // left in place: the class matches no failure, and keeping it keeps
            // the target out of the pool (#3390)
            for (provider, class) in unknown_failure_classes(route) {
                problems.push(
                    "failure_class",
                    format!(
                        "route '{model}' target '{provider}' takes over a failure class this \
                         gateway does not know ('{class}'); it takes over nothing for it until \
                         the gateway is upgraded"
                    ),
                );
            }
        }
        for budget in &mut self.budgets {
            let (scope, id) = (budget.scope.key_label(), budget.id.clone());
            if let Some(policy) = budget.unpriced_policy.as_mut() {
                resolve_unknown(
                    policy,
                    UnpricedPolicy::Unknown,
                    UnpricedPolicy::Block,
                    problems,
                    "unpriced_policy",
                    || format!("budget {scope} '{id}' unpriced_policy"),
                );
            }
        }
        for rule in &mut self.guardrails.rules {
            let name = rule.name.clone();
            resolve_unknown(
                &mut rule.action,
                GuardAction::Unknown,
                GuardAction::Block,
                problems,
                "guard_action",
                || format!("guardrail rule '{name}' action"),
            );
        }
        resolve_unknown(
            &mut self.guardrails.streaming_post_call,
            StreamingPostCall::Unknown,
            StreamingPostCall::Reject,
            problems,
            "streaming_post_call",
            || "guardrails.streaming_post_call".to_string(),
        );
        resolve_unknown(
            &mut self.guardrail_webhook.failure_mode,
            FailureMode::Unknown,
            FailureMode::FailClosed,
            problems,
            "failure_mode",
            || "guardrail_webhook.failure_mode".to_string(),
        );
        for provider in &mut self.semantic_guards.providers {
            let name = provider.name.clone();
            resolve_unknown(
                &mut provider.failure_mode,
                FailureMode::Unknown,
                FailureMode::FailClosed,
                problems,
                "failure_mode",
                || format!("semantic guard provider '{name}' failure_mode"),
            );
        }
        for policy in &mut self.semantic_guards.policies {
            let name = policy.name.clone();
            resolve_unknown(
                &mut policy.on_violation,
                ViolationAction::Unknown,
                ViolationAction::Block,
                problems,
                "violation_action",
                || format!("semantic guard policy '{name}' on_violation"),
            );
            resolve_unknown(
                &mut policy.streaming,
                StreamingStrategy::Unknown,
                StreamingStrategy::Reject,
                problems,
                "streaming_strategy",
                || format!("semantic guard policy '{name}' streaming"),
            );
        }
        // a guard provider of an unknown kind keeps its value: no known kind
        // is a safe reading of it, so it blocks its stage per request instead,
        // whatever its failure_mode (ADR-0039)
        let guard_providers = std::iter::once(&self.guardrail_webhook)
            .filter(|webhook| webhook.enabled)
            .chain(&self.semantic_guards.providers);
        for provider in guard_providers {
            if provider.kind == crate::guardrail_webhook::GuardProviderKind::Unknown {
                problems.push(
                    "guard_provider_kind",
                    format!(
                    "semantic guard provider '{}' kind has a value this gateway does not know; \
                     it blocks its stage until the gateway is upgraded",
                    if provider.name.trim().is_empty() {
                        "guardrail_webhook"
                    } else {
                        provider.name.trim()
                    }
                    ),
                );
            }
        }
        let pii = &mut self.pii_sanitizer;
        resolve_unknown(
            &mut pii.direction,
            SanitizeDirection::Unknown,
            SanitizeDirection::Both,
            problems,
            "pii_direction",
            || "pii_sanitizer.direction".to_string(),
        );
        resolve_unknown(
            &mut pii.restoration,
            RestorationPolicy::Unknown,
            RestorationPolicy::Never,
            problems,
            "pii_restoration",
            || "pii_sanitizer.restoration".to_string(),
        );
        resolve_unknown(
            &mut pii.streaming,
            StreamingResponse::Unknown,
            StreamingResponse::Reject,
            problems,
            "pii_streaming",
            || "pii_sanitizer.streaming".to_string(),
        );
        resolve_unknown(
            &mut pii.failure_mode,
            FailureMode::Unknown,
            FailureMode::FailClosed,
            problems,
            "failure_mode",
            || "pii_sanitizer.failure_mode".to_string(),
        );
        for instance in &mut self.plugins.instances {
            let slug = instance.slug.clone();
            resolve_unknown(
                &mut instance.failure_mode,
                FailureMode::Unknown,
                FailureMode::FailClosed,
                problems,
                "failure_mode",
                || format!("plugin '{slug}' failure_mode"),
            );
        }
    }

    /// Every enum value in a file config this build does not know (#3347).
    fn unknown_value_problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        unknown_value(&mut p, &self.unpriced_policy, || {
            "unpriced_policy".to_string()
        });
        unknown_value(&mut p, &self.queue.backpressure, || {
            "queue.backpressure".to_string()
        });
        for provider in self.providers.iter().chain(&self.provider_defaults) {
            let profiles = provider
                .role_profile
                .iter()
                .chain(provider.model_role_profiles.values());
            for profile in profiles {
                unknown_value(&mut p, profile, || {
                    format!("provider '{}' role profile", provider.name)
                });
            }
        }
        for provider in self.providers.iter().chain(&self.provider_defaults) {
            unknown_value(&mut p, &provider.kind, || {
                format!("provider '{}' kind", provider.name)
            });
        }
        for group in self
            .provider_groups
            .iter()
            .chain(&self.provider_group_defaults)
        {
            unknown_value(&mut p, &group.strategy, || {
                format!("provider group '{}' strategy", group.name)
            });
        }
        let routes = self
            .routes
            .iter()
            .chain(&self.models.readonly)
            .chain(&self.models.defaults);
        for route in routes {
            unknown_value(&mut p, &route.strategy, || {
                format!("route '{}' strategy", route.model)
            });
            unknown_value(&mut p, &route.param_policy.mode, || {
                format!("route '{}' param_policy.mode", route.model)
            });
            for (provider, class) in unknown_failure_classes(route) {
                p.push(format!(
                    "route '{}' target '{provider}' takes_over '{class}' is not a failure class \
                     this build knows; expected one of throttled, timeout, transport, \
                     server_error, refusal or refusal:<category>",
                    route.model
                ));
            }
        }
        for budget in &self.budgets {
            unknown_value(&mut p, &budget.scope, || {
                format!("budget '{}' scope", budget.id)
            });
            unknown_value(&mut p, &budget.period, || {
                format!("budget '{}' period", budget.id)
            });
            unknown_value(&mut p, &budget.unpriced_policy, || {
                format!("budget '{}' unpriced_policy", budget.id)
            });
        }
        for limit in &self.rate_limits {
            unknown_value(&mut p, &limit.scope, || {
                format!("rate limit '{}' scope", limit.id)
            });
        }
        for rule in &self.guardrails.rules {
            unknown_value(&mut p, &rule.action, || {
                format!("guardrail rule '{}' action", rule.name)
            });
        }
        unknown_value(&mut p, &self.guardrails.streaming_post_call, || {
            "guardrails.streaming_post_call".to_string()
        });
        unknown_value(&mut p, &self.guardrail_webhook.failure_mode, || {
            "guardrail_webhook.failure_mode".to_string()
        });
        unknown_value(&mut p, &self.guardrail_webhook.kind, || {
            "guardrail_webhook.kind".to_string()
        });
        for provider in &self.semantic_guards.providers {
            unknown_value(&mut p, &provider.kind, || {
                format!("semantic guard provider '{}' kind", provider.name)
            });
            unknown_value(&mut p, &provider.failure_mode, || {
                format!("semantic guard provider '{}' failure_mode", provider.name)
            });
        }
        for policy in &self.semantic_guards.policies {
            unknown_value(&mut p, &policy.on_violation, || {
                format!("semantic guard policy '{}' on_violation", policy.name)
            });
            unknown_value(&mut p, &policy.streaming, || {
                format!("semantic guard policy '{}' streaming", policy.name)
            });
        }
        let pii = &self.pii_sanitizer;
        unknown_value(&mut p, &pii.direction, || {
            "pii_sanitizer.direction".to_string()
        });
        unknown_value(&mut p, &pii.restoration, || {
            "pii_sanitizer.restoration".to_string()
        });
        unknown_value(&mut p, &pii.streaming, || {
            "pii_sanitizer.streaming".to_string()
        });
        unknown_value(&mut p, &pii.failure_mode, || {
            "pii_sanitizer.failure_mode".to_string()
        });
        for instance in &self.plugins.instances {
            unknown_value(&mut p, &instance.failure_mode, || {
                format!("plugin '{}' failure_mode", instance.slug)
            });
        }
        p
    }
}

/// Effective slug for a provider group.
fn group_slug(g: &ProviderGroupConfig) -> String {
    g.slug
        .clone()
        .unwrap_or_else(|| crate::slug::slugify(&g.name))
}

impl GatewayConfig {
    /// Parse a configuration from a TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self> {
        // pre-extract the tiered sections (providers, provider_groups) from the
        // raw document so a single key accepts both the deprecated flat array
        // (`[[providers]]`) and the tiered table (`[providers] readonly/default`,
        // ADR-0022). the remaining document deserializes normally; the `providers`
        // / `provider_groups` struct fields default to empty and are set below.
        let mut doc: toml::Value = toml::from_str(s)?;
        // carry an older file forward before anything reads it (#326). this runs
        // ahead of the tiered-section extraction below on purpose: a migration
        // may rewrite `providers` / `provider_groups`, and it has to do so while
        // they are still part of the document
        if let Some(table) = doc.as_table_mut() {
            let report = crate::config_migrate::migrate(table);
            if report.ahead {
                // a file from a newer build. loading it unchanged is the
                // deliberate choice — see config_migrate's module docs on why a
                // rollback must not be the thing the stamp breaks
                tracing::warn!(
                    file_version = report.from,
                    understood_version = crate::config_migrate::CURRENT_SCHEMA_VERSION,
                    "config schema_version is newer than this build understands; \
                     loading it as-is. keys this build does not know are ignored"
                );
            }
        }
        let mut provider_readonly = Vec::new();
        let mut provider_defaults = Vec::new();
        let mut group_readonly = Vec::new();
        let mut group_defaults = Vec::new();
        if let Some(table) = doc.as_table_mut() {
            if let Some(v) = table.remove("providers") {
                (provider_readonly, provider_defaults) = split_section(v)?;
            }
            if let Some(v) = table.remove("provider_groups") {
                (group_readonly, group_defaults) = split_section(v)?;
            }
        }
        let retired_vk = doc
            .as_table_mut()
            .and_then(take_retired_virtual_key_required);
        let mut config: Self = doc.try_into()?;
        config.provider_defaults = provider_defaults;
        config.providers = provider_readonly;
        config.provider_groups = group_readonly;
        config.provider_group_defaults = group_defaults;
        // a file is written by hand, so an enum value this build does not know
        // is a typo there rather than a value from a newer release, and is
        // refused here as it was before the enums gained `Unknown` (#3347)
        let unknown = config.unknown_value_problems();
        if !unknown.is_empty() {
            return Err(crate::Error::Config(unknown.join("; ")));
        }
        if let Some(value) = retired_vk {
            fold_retired_virtual_key_required(&mut config.server, &value);
        }

        // a default-tier provider/group may not collide with a readonly one: the
        // readonly entry is immutable and would shadow the seeded DB row (ADR-0022)
        let readonly_provider_slugs: std::collections::HashSet<String> =
            config.providers.iter().map(provider_slug).collect();
        for p in &config.provider_defaults {
            if readonly_provider_slugs.contains(&provider_slug(p)) {
                return Err(crate::Error::Config(format!(
                    "providers.default slug '{}' collides with a readonly provider",
                    provider_slug(p)
                )));
            }
        }
        let readonly_group_slugs: std::collections::HashSet<String> =
            config.provider_groups.iter().map(group_slug).collect();
        for g in &config.provider_group_defaults {
            if readonly_group_slugs.contains(&group_slug(g)) {
                return Err(crate::Error::Config(format!(
                    "provider_groups.default slug '{}' collides with a readonly group",
                    group_slug(g)
                )));
            }
        }

        let readonly = std::mem::take(&mut config.models.readonly);
        let mut names: std::collections::HashSet<String> = config
            .routes
            .iter()
            .map(|route| route.model.clone())
            .collect();
        for route in readonly {
            if !names.insert(route.model.clone()) {
                return Err(crate::Error::Config(format!(
                    "models.readonly model '{}' duplicates an existing route",
                    route.model
                )));
            }
            config.routes.push(route);
        }
        for route in &config.models.defaults {
            if names.contains(&route.model) {
                return Err(crate::Error::Config(format!(
                    "models.default model '{}' conflicts with a read-only/config route",
                    route.model
                )));
            }
        }
        Ok(config)
    }

    /// Load a configuration from a TOML file on disk.
    ///
    /// Keys the config model does not recognise are logged as warnings and then
    /// ignored (#1434). The lint lives here rather than in each binary so no
    /// load path can be added without it: a typo that silently leaves a setting
    /// at its default is a startup-time problem everywhere the file is read,
    /// not only under `rolter check`.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        crate::config_lint::warn_unknown_keys_once(path, &raw);
        let config = Self::from_toml_str(&raw)?;
        config.validate_ca_bundles().map_err(|problems| {
            crate::Error::Config(format!(
                "invalid CA bundle configuration: {}",
                problems.join("; ")
            ))
        })?;
        Ok(config)
    }

    /// One warning per provider or group that sets `project_scoped = true`.
    ///
    /// A file names no projects, so the flag means "the project this file is
    /// imported into" and only `rolter-seed --import` acts on it. It is kept out
    /// of [`GatewayConfig::load`] on purpose: the seeder loads through the same
    /// function and must stay silent. Callers that serve the file directly
    /// (the gateway, `rolter check`) log these lines themselves (#2468).
    pub fn project_scoped_warnings(&self) -> Vec<String> {
        let providers = self
            .providers
            .iter()
            .filter(|p| p.project_scoped)
            .map(|p| ("provider", p.name.as_str()));
        let groups = self
            .provider_groups
            .iter()
            .filter(|g| g.project_scoped)
            .map(|g| ("provider group", g.name.as_str()));
        providers
            .chain(groups)
            .map(|(what, name)| {
                format!(
                    "{what} '{name}' sets project_scoped = true, which a gateway reading this \
                     file directly ignores (a file names no projects); it only takes effect \
                     through the control plane, via `rolter-seed --import`"
                )
            })
            .collect()
    }

    /// One warning per route whose targets mix decision-only providers with
    /// ones that are not (#3154).
    ///
    /// The gateway refuses a decision for a route none of whose targets serves
    /// decisions, and any other endpoint for a route whose targets are all
    /// decision-only. A route that mixes the two is deliberately left to
    /// balance (ADR-0035), so some chat requests reach a decision server and
    /// some decisions reach a target that does not answer them. Nothing is
    /// refused or dropped; the line names the targets on each side so the
    /// operator can split the route. A target whose provider is unknown is not
    /// counted on either side.
    pub fn mixed_decision_route_warnings(&self) -> Vec<String> {
        let mut kinds = crate::namespace::Namespaced::new();
        for p in &self.providers {
            let _ = kinds.insert(
                crate::namespace::org_of(p.tenancy.as_ref()),
                &p.name,
                p.kind,
            );
        }
        let mut warnings = Vec::new();
        for route in &self.routes {
            let org = crate::namespace::org_of(route.tenancy.as_ref());
            let mut decision: Vec<&str> = Vec::new();
            let mut generative: Vec<&str> = Vec::new();
            for target in route.every_target() {
                let Some(kind) = kinds.get_ref(org, &target.provider) else {
                    continue;
                };
                let side = if kind.decision_only() {
                    &mut decision
                } else {
                    &mut generative
                };
                if !side.contains(&target.provider.as_str()) {
                    side.push(target.provider.as_str());
                }
            }
            if decision.is_empty() || generative.is_empty() {
                continue;
            }
            warnings.push(format!(
                "route '{}' mixes decision-only targets ({}) with targets that are not ({}); \
                 requests are balanced across both, so some chat and embedding requests reach \
                 a decision server and some decisions reach a target that may not answer them. \
                 Split it into one route per kind",
                route.model,
                decision.join(", "),
                generative.join(", ")
            ));
        }
        warnings
    }

    /// Return the CA bundles a provider should trust. An explicit provider
    /// value replaces the global setting. `ROLTER_CA_BUNDLE` replaces the
    /// global config value for container-friendly deployment.
    pub fn ca_bundles_for(&self, provider: &ProviderConfig) -> Vec<PathBuf> {
        if let Some(paths) = &provider.ca_bundles {
            return paths.clone();
        }
        std::env::var_os("ROLTER_CA_BUNDLE")
            .map(PathBuf::from)
            .map(|path| vec![path])
            .unwrap_or_else(|| self.tls.ca_bundles.clone())
    }

    /// Validate every effective CA bundle path and PEM payload.
    pub fn validate_ca_bundles(&self) -> std::result::Result<(), Vec<String>> {
        let mut problems = Vec::new();
        let mut checked = std::collections::HashSet::new();
        for provider in &self.providers {
            for path in self.ca_bundles_for(provider) {
                if checked.insert(path.clone()) {
                    if let Err(problem) = validate_ca_bundle(&path) {
                        problems.push(problem);
                    }
                }
            }
        }
        // validate global trust even when no providers are present yet
        let global = std::env::var_os("ROLTER_CA_BUNDLE")
            .map(PathBuf::from)
            .map(|path| vec![path])
            .unwrap_or_else(|| self.tls.ca_bundles.clone());
        for path in global {
            if checked.insert(path.clone()) {
                if let Err(problem) = validate_ca_bundle(&path) {
                    problems.push(problem);
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// Advisory notes about providers that opted out of their kind's host pin
    /// with `allow_custom_api_base`.
    ///
    /// The opt-out is legitimate — an egress proxy, a vendor-compatible
    /// endpoint, a local fake used to exercise the dialect — but it removes the
    /// guard that stops a config edit from redirecting a hosted vendor's key to
    /// another host, so `rolter check` reports every provider that sets it. It
    /// is never fatal: nothing here blocks a boot.
    pub fn custom_api_base_advisories(&self) -> Vec<String> {
        self.providers
            .iter()
            .filter(|provider| provider.allow_custom_api_base)
            .map(|provider| {
                format!(
                    "provider '{}' ({:?}) sets allow_custom_api_base and points at {}, \
                     so its key travels to a host rolter cannot vouch for",
                    provider.name, provider.kind, provider.api_base
                )
            })
            .collect()
    }

    /// Find a provider by name.
    pub fn resolve_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Drop the public example key from a file config. Callers decide *when*:
    /// only a deployment that is not in open mode has any business refusing it,
    /// since `easy-up` serves it on purpose. Adds the problem line and the
    /// record to `report` for each key dropped; `report` is the one
    /// [`sanitize_for_snapshot_report`](Self::sanitize_for_snapshot_report)
    /// returned, whose key positions keep the record's index true to the
    /// loaded list.
    pub fn prune_public_example_key(&mut self, report: &mut SnapshotReport) {
        let origin = std::mem::take(&mut report.file_key_origin);
        let mut kept = Vec::with_capacity(self.virtual_keys.len());
        let mut position = 0;
        let mut dropped = Vec::new();
        self.virtual_keys.retain(|k| {
            let at = origin.get(position).copied().unwrap_or(position);
            position += 1;
            if k.is_public_example_key() {
                dropped.push((
                    at,
                    k.name.clone().unwrap_or_else(|| UNNAMED_KEY.to_string()),
                ));
                false
            } else {
                kept.push(at);
                true
            }
        });
        report.file_key_origin = kept;
        for (index, name) in dropped {
            let reason = "it is the public example key from the bundled rolter.example.toml and \
                          allows every model, so a deployment with auth enforced must not hand it \
                          to gateways. Remove the [[virtual_keys]] entry from the control plane's \
                          config file";
            report.problems.push(format!(
                "virtual key '{PUBLIC_EXAMPLE_KEY}' omitted from the snapshot: {reason}"
            ));
            report.omitted.push(OmittedEntry {
                kind: OmittedKind::PublicExampleKey,
                name,
                reason: reason.to_string(),
                index,
            });
        }
    }

    /// Validate internal consistency and surface every problem at once so an
    /// operator can fix a whole config in one pass rather than one error per
    /// restart. Checks: unique/non-empty provider names, well-formed provider
    /// `api_base` and `egress_proxy` URLs, unique/non-empty route models, each
    /// route having at least one target that references a known provider with a
    /// positive weight, unique/non-empty virtual keys, positive budget limits and
    /// rate limits that actually cap something. Returns every problem found, so
    /// callers can log/report them all.
    /// Drop entries that are unservable *on their own*, returning one warning
    /// per dropped entry.
    ///
    /// Snapshot validation is otherwise all-or-nothing: a single half-built row
    /// makes [`validate`](Self::validate) fail and `/internal/snapshot` 500,
    /// which freezes config propagation for **every** tenant, not just the one
    /// that owns the row. A route created before its targets are added is an
    /// ordinary transient state in a "create route → then add targets" flow, so
    /// it must not take the whole data plane's config with it.
    ///
    /// Only row-local defects are pruned here. Problems that are structural or
    /// span rows — duplicate names, a bad `metrics_path`, unreadable CA bundles
    /// — are deliberately left for `validate` to reject, since silently
    /// dropping one of two colliding rows would be worse than refusing.
    pub fn sanitize_for_snapshot(&mut self) -> Vec<String> {
        self.sanitize_for_snapshot_report().problems
    }

    /// [`sanitize_for_snapshot`](Self::sanitize_for_snapshot) with a record for
    /// every provider, route and file key it dropped whole, so a client does
    /// not have to read the sentences to learn which.
    pub fn sanitize_for_snapshot_report(&mut self) -> SnapshotReport {
        let mut unknown = UnknownValues::default();
        self.resolve_unknown_settings(&mut unknown);
        let mut warnings = unknown.lines;
        let mut omitted_entries = Vec::new();
        self.prune_invalid_prompt_templates(&mut warnings);
        self.prune_invalid_model_prices(&mut warnings);

        // a provider whose own definition is invalid cannot serve traffic, but
        // it is exactly one row: withholding the other fourteen providers and
        // eleven routes because of it is the wrong blast radius (#926). Drop it
        // and say so; the routes that depended on it fall out below, since they
        // are then left with no target pointing at a known provider.
        //
        // Cross-provider defects (empty or duplicated names) are deliberately
        // not considered here — silently dropping one of two colliding rows
        // would be worse than refusing, so `validate` still rejects those.
        self.strip_secret_env_credentials(&mut warnings);
        self.strip_unusable_telemetry(&mut warnings);
        // every provider as `(org, name)` before any is dropped, so a reference
        // still resolves to the row it named once that row is gone (#1857)
        let mut every_provider = crate::namespace::Namespaced::new();
        for provider in &self.providers {
            let org = crate::namespace::org_of(provider.tenancy.as_ref());
            let _ = every_provider.insert(org, &provider.name, ());
        }
        let mut omitted: Vec<(String, String)> = Vec::new();
        let invalid: Vec<(usize, String, String, Vec<String>)> = self
            .providers
            .iter()
            .enumerate()
            .filter_map(|(idx, provider)| {
                let problems = self.provider_problems(provider);
                let org = crate::namespace::org_of(provider.tenancy.as_ref()).to_string();
                (!problems.is_empty()).then(|| (idx, org, provider.name.clone(), problems))
            })
            .collect();
        if !invalid.is_empty() {
            let dropped: std::collections::HashSet<usize> =
                invalid.iter().map(|(i, _, _, _)| *i).collect();
            let mut idx = 0;
            self.providers.retain(|_| {
                let keep = !dropped.contains(&idx);
                idx += 1;
                keep
            });
            for (index, org, name, problems) in invalid {
                let reason = problems.join("; ");
                warnings.push(format!(
                    "provider '{name}' omitted from the snapshot: {reason}"
                ));
                omitted_entries.push(OmittedEntry {
                    kind: OmittedKind::Provider,
                    name: name.clone(),
                    reason,
                    index,
                });
                omitted.push((org, name));
            }
        }
        // a name another provider of the same org still carries is not
        // omitted: two rows sharing a name is a cross-row defect `validate`
        // reports once one of them is gone, and pruning the references would
        // take the surviving row's traffic with it
        omitted.retain(|(org, name)| {
            !self
                .providers
                .iter()
                .any(|p| &p.name == name && crate::namespace::org_of(p.tenancy.as_ref()) == org)
        });
        // a provider the store loader left out never reached `providers`, so
        // the passes above could not see it (#2950); it is omitted all the same
        let omitted = OmittedProviders::new(every_provider, omitted, &self.left_out_providers);

        self.report_denied_operator_urls(&mut warnings);
        // the omission above leaves every reference to that provider dangling,
        // and `validate_snapshot` refuses the whole snapshot over a dangling
        // one (#2945). prune each before the route pass below, which then
        // drops a route left with nothing to serve
        self.prune_omitted_provider_targets(&omitted, &mut warnings);
        self.prune_cross_project_targets(&mut warnings);

        let mut provider_names = crate::namespace::Namespaced::new();
        for provider in self.providers.iter().filter(|p| !p.name.trim().is_empty()) {
            let org = crate::namespace::org_of(provider.tenancy.as_ref());
            let _ = provider_names.insert(org, &provider.name, ());
        }

        // a target can serve traffic only when it points at a known provider
        // with a non-zero weight; a variant needs at least one such target
        // tracked by index, not model name: a duplicated model must not let an
        // unservable row take its servable twin down with it (validate still
        // rejects the duplication itself)
        let mut drops: Vec<(usize, String, &'static str)> = Vec::new();
        for (idx, route) in self.routes.iter().enumerate() {
            // a strategy from a newer release (#3347): the route refuses
            // rather than being balanced by a guess
            if route.strategy == BalancingStrategy::Unknown {
                drops.push((
                    idx,
                    route.model.clone(),
                    "its balancing strategy is not one this build knows",
                ));
                continue;
            }
            let org = crate::namespace::org_of(route.tenancy.as_ref());
            let known = |provider: &str| provider_names.get_ref(org, provider).is_some();
            let servable_target = route
                .targets
                .iter()
                .any(|t| known(&t.provider) && t.weight > 0);
            let servable_variant = route
                .variants
                .iter()
                .any(|v| !v.name.trim().is_empty() && v.targets.iter().any(|t| known(&t.provider)));
            if !servable_target && !servable_variant {
                drops.push((
                    idx,
                    route.model.clone(),
                    "it has no target that references a known provider with a positive weight",
                ));
            } else if route.only_restricted_targets_dialled() {
                // what is left to dial is only fallbacks, which `validate_snapshot`
                // refuses the whole snapshot over (#3431)
                drops.push((
                    idx,
                    route.model.clone(),
                    "every target it dials sets takes_over, so it could never serve a request \
                     (all_targets_restricted)",
                ));
            }
        }
        if !drops.is_empty() {
            let dropped: std::collections::HashSet<usize> =
                drops.iter().map(|(i, _, _)| *i).collect();
            let mut idx = 0;
            self.routes.retain(|_| {
                let keep = !dropped.contains(&idx);
                idx += 1;
                keep
            });
            for (index, model, reason) in drops {
                warnings.push(format!(
                    "route '{model}' omitted from the snapshot: {reason}"
                ));
                omitted_entries.push(OmittedEntry {
                    kind: OmittedKind::Route,
                    name: model,
                    reason: reason.to_string(),
                    index,
                });
            }
        }

        // a mixed route still serves, so this only says what it will do (#3154)
        warnings.extend(self.mixed_decision_route_warnings());

        // what is left to repair names an omitted provider from outside a
        // route's targets, and is settled after the route pass so a route
        // that was dropped does not also report a cache it no longer has
        let file_key_origin =
            self.prune_omitted_provider_settings(&omitted, &mut warnings, &mut omitted_entries);

        // a route override naming a guardrail rule the effective set lacks is
        // pruned here, not refused: the store loads only enabled rows, so a
        // rule paused, renamed or deleted in the dashboard leaves its overrides
        // dangling, and refusing would freeze propagation of every config
        // change (#2306). Pruning is safe in both directions: an `enable` for a
        // missing rule has nothing to enable, and a `disable` for a rule that is
        // not running has nothing to switch off. `validate` keeps rejecting an
        // unknown name in a file config, where it is a typo and a typo in
        // `disable` must not read as "this rule is off here"; only the snapshot
        // path, which is store-sourced, prunes
        let rule_names: Vec<String> = self
            .guardrails
            .rules
            .iter()
            .map(|rule| rule.name.trim().to_string())
            .collect();
        for route in &mut self.routes {
            let unknown = route.advanced.guardrails.unknown_rules(&rule_names);
            if unknown.is_empty() {
                continue;
            }
            route
                .advanced
                .guardrails
                .disable
                .retain(|name| rule_names.iter().any(|known| known == name.trim()));
            route
                .advanced
                .guardrails
                .enable
                .retain(|name| rule_names.iter().any(|known| known == name.trim()));
            let mut seen = std::collections::HashSet::new();
            for name in unknown.into_iter().filter(|name| seen.insert(name.clone())) {
                warnings.push(format!(
                    "route '{}' override of guardrail rule '{name}' omitted from the snapshot: \
                     no enabled rule has that name (paused, renamed or deleted)",
                    route.model
                ));
            }
        }

        // a header the wire cannot carry would fail every call the route makes,
        // and one that names a credential would replace the provider's. the
        // control plane refuses both on write, so this covers a row written
        // before it did or by sql. the route keeps serving without the header
        // rather than withholding the snapshot for every tenant (#2924)
        for route in &mut self.routes {
            for problem in route.advanced.drop_unsendable_headers() {
                warnings.push(format!(
                    "route '{}': {problem}; it is omitted from the snapshot and the route \
                     serves without it",
                    route.model
                ));
            }
        }

        // a zero or oversized throughput cap would refuse every request to the
        // route. the control plane refuses one on write; a row written around
        // it serves without that cap rather than being shut (#2936)
        for route in &mut self.routes {
            for problem in route.advanced.limits.drop_unusable_throughput() {
                warnings.push(format!(
                    "route '{}': {problem}; it is omitted from the snapshot and the route \
                     serves without that cap",
                    route.model
                ));
            }
        }

        // the control plane refuses an unusable request id header on write, so
        // this covers a row written before it did or by sql. falling back to
        // the default keeps correlation working instead of withholding the
        // snapshot from every tenant over one header name (#2925)
        if let Some(problem) = request_id_header_problem(&self.client.request_id_header) {
            warnings.push(format!(
                "client.request_id_header: {problem}; gateways use \
                 '{DEFAULT_REQUEST_ID_HEADER}' instead"
            ));
            self.client.request_id_header = default_request_id_header();
        }
        SnapshotReport {
            problems: warnings,
            omitted: omitted_entries,
            file_key_origin,
        }
    }

    /// Clear a provider's `status_page_url`, `kv_events` or `lmcache` setting when the egress
    /// policy denies its endpoint or its shape is invalid, keeping the provider.
    /// Cache telemetry is optional, so losing it must not cost the provider its
    /// traffic (#2997); `validate` still refuses the same setting in a file.
    /// Drop a credential variable that names one of the deployment's own
    /// secrets. The gateway would otherwise read it and send it to the
    /// provider's `api_base`; a row saved before the control plane refused such
    /// names, or imported by `rolter-seed`, can still carry one. The provider
    /// stays and serves without that credential.
    fn strip_secret_env_credentials(&mut self, warnings: &mut Vec<String>) {
        for provider in &mut self.providers {
            let name = provider.name.clone();
            if let Some(env) = provider.api_key_env.as_deref() {
                if crate::secret_env::is_control_plane_secret(env) {
                    warnings.push(format!(
                        "provider '{name}' api_key_env '{env}' names a deployment secret and \
                         was omitted from the snapshot; the provider serves without that credential"
                    ));
                    provider.api_key_env = None;
                }
            }
            for key in &mut provider.api_keys {
                if let Some(env) = key.env.as_deref() {
                    if crate::secret_env::is_control_plane_secret(env) {
                        warnings.push(format!(
                            "provider '{name}' api_keys env '{env}' names a deployment secret and \
                             was omitted from the snapshot; that key is not used"
                        ));
                        key.env = None;
                    }
                }
            }
        }
    }

    fn strip_unusable_telemetry(&mut self, warnings: &mut Vec<String>) {
        let egress = self.egress.clone();
        for provider in &mut self.providers {
            let name = provider.name.clone();
            if let Some(url) = &provider.status_page_url {
                if let Err(p) = egress.check_url(url, &format!("provider '{name}' status_page_url"))
                {
                    warnings.push(format!(
                        "provider '{name}' status_page_url omitted from the snapshot and the \
                         provider serves without it: {p}"
                    ));
                    provider.status_page_url = None;
                }
            }
            if let Some(kv) = &provider.kv_events {
                let mut problems = Vec::new();
                if let Err(p) = egress.check_url(
                    &kv.endpoint,
                    &format!("provider '{name}' kv_events.endpoint"),
                ) {
                    problems.push(p);
                }
                problems.extend(kv.shape_problems(&name));
                if !problems.is_empty() {
                    warnings.push(format!(
                        "provider '{name}' kv_events omitted from the snapshot and the provider \
                         serves without it: {}",
                        problems.join("; ")
                    ));
                    provider.kv_events = None;
                }
            }
            if let Some(lm) = &provider.lmcache {
                let mut problems = Vec::new();
                if let Err(p) =
                    egress.check_url(&lm.endpoint, &format!("provider '{name}' lmcache.endpoint"))
                {
                    problems.push(p);
                }
                problems.extend(lm.shape_problems(&name));
                if !problems.is_empty() {
                    warnings.push(format!(
                        "provider '{name}' lmcache omitted from the snapshot and the provider \
                         serves without it: {}",
                        problems.join("; ")
                    ));
                    provider.lmcache = None;
                }
            }
        }
    }

    /// Report each gateway-side URL the egress policy denies in `warnings`,
    /// without removing or switching off anything.
    ///
    /// The webhook, the sanitizer and a plugin are guards with a failure mode.
    /// Dropping one would turn a fail-closed control into a silent fail-open,
    /// so it stays: the gateway refuses the call at request time and the
    /// block's own failure mode decides what the request does. Nothing here
    /// can withhold `/internal/snapshot`, since
    /// [`validate_snapshot`](Self::validate_snapshot) does not repeat the
    /// check; `validate` still rejects the same URLs in a file config.
    fn report_denied_operator_urls(&self, warnings: &mut Vec<String>) {
        for problem in self.operator_url_problems() {
            warnings.push(format!(
                "{problem}: calls will fail and follow the failure_mode of the control that uses it"
            ));
        }
    }

    /// Drop each route target and group member that reaches a provider scoped
    /// to a project other than its owner's, saying why in `warnings` (#1919).
    ///
    /// The control plane refuses to write such a row, so one only exists when
    /// it came from SQL, a seed or a scope narrowed around existing rows. It
    /// must not be served, because the provider would then be reachable from
    /// outside its project through the route or group. It is also not a reason
    /// to withhold the whole snapshot, so only the offending target goes: a
    /// route left with no servable target is dropped by the pass that follows,
    /// and a group left with no member never resolves (#926 / #2306 / #2279).
    ///
    /// A route's owner is its project; a group's owner is its own scope, and an
    /// org-wide group owns no project, so it may not hold a scoped provider. A
    /// route or group from a file carries no tenancy, which is org-wide too.
    fn prune_cross_project_targets(&mut self, warnings: &mut Vec<String>) {
        // owned, since the prune below borrows the whole config mutably
        let mut scoped = crate::namespace::Namespaced::new();
        let mut any_scoped = false;
        for p in &self.providers {
            let org = crate::namespace::org_of(p.tenancy.as_ref()).to_string();
            let scope = p.tenancy.as_ref().and_then(|t| t.project_id.clone());
            any_scoped |= scope.is_some();
            let _ = scoped.insert(&org, &p.name, scope);
        }
        if !any_scoped {
            return;
        }
        self.prune_targets_where(warnings, |provider, owner_org, owner, owner_kind| {
            let scope = scoped.get_ref(owner_org, provider)?.as_ref()?;
            (owner != Some(scope.as_str())).then(|| {
                format!("the provider is scoped to a project other than the {owner_kind}'s")
            })
        });
    }

    /// Drop each route target, variant target and group member that names a
    /// provider [`sanitize_for_snapshot`](Self::sanitize_for_snapshot) just
    /// omitted, saying why in `warnings` (#2945).
    ///
    /// Omitting the provider is only half of removing it: every reference left
    /// behind is a name `validate_snapshot` cannot resolve, and it refuses the
    /// whole snapshot over one, which hands one provider's defect to every
    /// tenant. The route pass that follows drops a route left with no target,
    /// so a route is only ever lost when nothing else on it can serve.
    fn prune_omitted_provider_targets(
        &mut self,
        omitted: &OmittedProviders,
        warnings: &mut Vec<String>,
    ) {
        // no early return on an empty omission: a reference to another org's
        // provider is pruned here too
        self.prune_targets_where(warnings, |provider, owner_org, _owner, _owner_kind| {
            omitted.refusal(owner_org, provider).map(str::to_string)
        });
    }

    /// The pass behind both target prunes: remove every target, variant target
    /// and group member for which
    /// `refuses(provider, owner_org, owner_project, owner_kind)` gives a reason,
    /// and report each one.
    ///
    /// `owner_org` is the org the route or group belongs to, which is where its
    /// provider names resolve first (#1857), `owner_project` its project, and
    /// `owner_kind` is `"route"` or `"group"` so a reason can name which.
    ///
    /// A variant left with no target is removed whole, because `validate` reads
    /// a targetless variant as a defect of its own and the snapshot would be
    /// refused after all. Its traffic share goes to the variants that remain,
    /// the same as if an operator had deleted it.
    fn prune_targets_where(
        &mut self,
        warnings: &mut Vec<String>,
        refuses: impl Fn(&str, &str, Option<&str>, &str) -> Option<String>,
    ) {
        // a free function rather than a closure, since the variant pass below
        // needs the warnings while this one would still hold them
        fn prune(
            targets: &mut Vec<Target>,
            place: &str,
            warnings: &mut Vec<String>,
            refuse: &impl Fn(&str) -> Option<String>,
        ) {
            targets.retain(|t| match refuse(&t.provider) {
                Some(reason) => {
                    warnings.push(format!(
                        "{place} target '{}' omitted from the snapshot: {reason}",
                        t.provider
                    ));
                    false
                }
                None => true,
            });
        }
        for route in &mut self.routes {
            let owner = route.tenancy.as_ref().and_then(|t| t.project_id.clone());
            let org = crate::namespace::org_of(route.tenancy.as_ref()).to_string();
            let model = route.model.clone();
            let refuse = |provider: &str| refuses(provider, &org, owner.as_deref(), "route");
            prune(
                &mut route.targets,
                &format!("route '{model}'"),
                warnings,
                &refuse,
            );
            route.variants.retain_mut(|variant| {
                let had_targets = !variant.targets.is_empty();
                let place = format!("route '{model}' variant '{}'", variant.name);
                prune(&mut variant.targets, &place, warnings, &refuse);
                let emptied = had_targets && variant.targets.is_empty();
                if emptied {
                    warnings.push(format!(
                        "{place} omitted from the snapshot: it has no target left to serve"
                    ));
                }
                !emptied
            });
        }
        for group in &mut self.provider_groups {
            let owner = group.tenancy.as_ref().and_then(|t| t.project_id.clone());
            let org = crate::namespace::org_of(group.tenancy.as_ref()).to_string();
            let slug = group
                .slug
                .clone()
                .unwrap_or_else(|| crate::slug::slugify(&group.name));
            group.members.retain(
                |m| match refuses(&m.provider, &org, owner.as_deref(), "group") {
                    Some(reason) => {
                        warnings.push(format!(
                            "provider group '{slug}' member '{}' omitted from the snapshot: \
                             {reason}",
                            m.provider
                        ));
                        false
                    }
                    None => true,
                },
            );
        }
    }

    /// Repair every other reference to a provider
    /// [`sanitize_for_snapshot`](Self::sanitize_for_snapshot) omitted, so none
    /// is left for `validate_snapshot` to refuse the snapshot over (#2945).
    ///
    /// - A route's semantic cache whose embeddings provider is gone is switched
    ///   off for that route. The cache is an optimisation in front of the
    ///   route, so the route keeps serving, with its exact-match cache if it
    ///   has one.
    /// - A virtual key's `providers` allow-list loses the omitted names, but
    ///   only while it stays non-empty. An empty list means every provider (see
    ///   `KeyMeta::provider_allowed` in the gateway), so a key that allowed
    ///   only omitted providers would be widened to all of them by the same
    ///   edit; that key is left out of the snapshot instead, which denies it.
    ///   This covers the file's keys and the store's, since neither is checked
    ///   by the other's rules.
    ///
    /// Returns the loaded position of each file key that remains.
    fn prune_omitted_provider_settings(
        &mut self,
        omitted: &OmittedProviders,
        warnings: &mut Vec<String>,
        records: &mut Vec<OmittedEntry>,
    ) -> Vec<usize> {
        // before the early return: a route naming another org's embeddings
        // provider is repaired even when nothing was omitted
        for route in &mut self.routes {
            let Some(cache) = route.cache.as_mut() else {
                continue;
            };
            let Some(semantic) = cache.semantic.as_ref() else {
                continue;
            };
            let org = crate::namespace::org_of(route.tenancy.as_ref());
            if let Some(reason) = omitted.refusal(org, &semantic.provider) {
                warnings.push(format!(
                    "route '{}' semantic cache omitted from the snapshot: its embeddings \
                     provider '{}' cannot be used ({reason}), so the route serves without \
                     semantic lookups",
                    route.model, semantic.provider
                ));
                cache.semantic = None;
            }
        }
        if omitted.is_empty() {
            return (0..self.virtual_keys.len()).collect();
        }
        let mut origin = Vec::with_capacity(self.virtual_keys.len());
        let mut index = 0;
        self.virtual_keys.retain_mut(|vk| {
            let at = index;
            index += 1;
            let label = vk.name.clone().unwrap_or_else(|| UNNAMED_KEY.to_string());
            let omitted = omitted.names_for(crate::namespace::GLOBAL, &vk.providers);
            match prune_key_allow_list(&mut vk.providers, &label, &omitted, warnings) {
                Some(reason) => {
                    records.push(OmittedEntry {
                        kind: OmittedKind::VirtualKey,
                        name: label,
                        reason,
                        index: at,
                    });
                    false
                }
                None => {
                    origin.push(at);
                    true
                }
            }
        });
        // a store key is not in the dashboard's config read, so it has no row
        // to point at and stays a line only
        self.db_virtual_keys.retain_mut(|vk| {
            let omitted = omitted.names_for(&vk.org_id, &vk.providers);
            prune_key_allow_list(&mut vk.providers, &vk.id, &omitted, warnings).is_none()
        });
        origin
    }

    /// Drop each prompt template that fails validation on its own, saying why
    /// in `warnings` (#2279).
    ///
    /// `validate` rejects the whole config over one malformed template, which
    /// made a single bad published version stop `/internal/snapshot` for every
    /// tenant. Granularity is one `(id, version)` entry, which is what the
    /// snapshot carries. Nothing else references a template (scopes live on the
    /// template itself and `validate` does not cross-check them against
    /// routes), so dropping one leaves no dangling reference. A duplicated
    /// `(id, version)` is a cross-entry defect and is left for `validate`.
    fn prune_invalid_prompt_templates(&mut self, warnings: &mut Vec<String>) {
        let before = self.prompt_templates.templates.len();
        self.prompt_templates.templates.retain(|template| {
            let problems =
                crate::prompt_templates::PromptTemplatesConfig::template_problems(template);
            if problems.is_empty() {
                return true;
            }
            warnings.push(format!(
                "prompt template '{}' version {} omitted from the snapshot: {}",
                template.id.trim(),
                template.version,
                problems.join("; ")
            ));
            false
        });
        if before != 0 && self.prompt_templates.templates.is_empty() {
            self.prompt_templates.enabled = false;
        }
    }

    /// Drop a price whose rates cannot be charged, with a warning naming the
    /// model.
    ///
    /// A negative rate makes the request a credit against its budget, and
    /// repairing it to zero would turn a priced model into a free one; neither
    /// is a guess to make for the operator. Omitting the row leaves the model
    /// unpriced, which `unpriced_policy` already has an answer for, and it is
    /// exactly one row, so it must not 500 the snapshot for every tenant
    /// (#2889). The control plane refuses such a rate on write, so this covers
    /// a row stored before that check, or written some other way.
    fn prune_invalid_model_prices(&mut self, warnings: &mut Vec<String>) {
        self.model_prices.retain(|price| {
            let problems = price.rate_problems();
            if problems.is_empty() {
                return true;
            }
            warnings.push(format!(
                "model price '{}' omitted from the snapshot: {}; the model is unpriced \
                 until the rate is corrected",
                price.model,
                problems.join("; ")
            ));
            false
        });
    }

    /// Every problem with `provider` considered on its own — everything
    /// [`validate`](Self::validate) checks per provider except cross-provider
    /// facts (an empty or duplicated name), which are not properties of the
    /// entry in isolation.
    ///
    /// Split out so [`sanitize_for_snapshot`](Self::sanitize_for_snapshot) can
    /// ask the question one provider at a time: a provider nobody routes to
    /// should not be able to withhold the whole fleet's config (#926).
    pub fn provider_problems(&self, provider: &ProviderConfig) -> Vec<String> {
        let mut problems = Vec::new();
        // a kind from a newer release (#3347): in a snapshot this omits the
        // provider, so it is never dialled; in a file it is a typo
        if provider.kind == ProviderKind::Unknown {
            problems.push(format!(
                "provider '{}' has a kind this build does not know",
                provider.name
            ));
        }
        if !is_http_url(&provider.api_base) {
            problems.push(format!(
                "provider '{}' has an invalid api_base '{}' (expected http:// or https:// url)",
                provider.name, provider.api_base
            ));
        }
        let secret_envs = provider
            .api_key_env
            .iter()
            .chain(provider.api_keys.iter().filter_map(|k| k.env.as_ref()));
        for env in secret_envs {
            if crate::secret_env::is_control_plane_secret(env) {
                problems.push(format!(
                    "provider '{}' credential variable '{env}' names a deployment secret",
                    provider.name
                ));
            }
        }
        if provider.kind == ProviderKind::Ollama
            && provider.api_base.trim_end_matches('/').ends_with("/v1")
        {
            problems.push(format!(
                "ollama provider '{}' api_base must be the daemon origin without /v1",
                provider.name
            ));
        }
        if provider.kind == ProviderKind::OllamaCloud {
            if provider.api_key_env_name().is_none_or(str::is_empty) {
                problems.push(format!(
                    "ollama_cloud provider '{}' requires api_key_env",
                    provider.name
                ));
            }
            if provider.has_non_env_credential() {
                problems.push(format!(
                    "ollama_cloud provider '{}' must source its key from api_key_env",
                    provider.name
                ));
            }
        }
        if provider.kind == ProviderKind::Openrouter {
            if provider.api_key_env_name().is_none_or(str::is_empty) {
                problems.push(format!(
                    "openrouter provider '{}' requires api_key_env",
                    provider.name
                ));
            }
            if provider.has_non_env_credential() {
                problems.push(format!(
                    "openrouter provider '{}' must source its key from api_key_env",
                    provider.name
                ));
            }
            if let Some(problem) = provider
                .kind
                .host_pin_problem(&provider.api_base, provider.allow_custom_api_base)
            {
                problems.push(format!("openrouter provider '{}' {problem}", provider.name));
            }
        }
        // hosted adapters authenticated with an env-sourced key enforce the
        // same secret hygiene as openrouter
        if provider.kind.requires_env_api_key()
            && provider.kind != ProviderKind::Openrouter
            && provider.kind != ProviderKind::OllamaCloud
        {
            let kind = match provider.kind {
                ProviderKind::Gemini => "gemini",
                ProviderKind::GeminiNative => "gemini_native",
                ProviderKind::GeminiInteractions => "gemini_interactions",
                ProviderKind::Mistral => "mistral",
                ProviderKind::Groq => "groq",
                ProviderKind::Xai => "xai",
                ProviderKind::MetaLlamaApi => "meta_llama_api",
                ProviderKind::Cohere => "cohere",
                ProviderKind::Perplexity => "perplexity",
                ProviderKind::Together => "together",
                ProviderKind::Fireworks => "fireworks",
                ProviderKind::Databricks => "databricks",
                ProviderKind::AlephAlpha => "aleph_alpha",
                ProviderKind::Nebius => "nebius",
                ProviderKind::Ovhcloud => "ovhcloud",
                ProviderKind::Scaleway => "scaleway",
                ProviderKind::Deepseek => "deepseek",
                ProviderKind::Qwen => "qwen",
                ProviderKind::Zhipu => "zhipu",
                ProviderKind::Kimi => "kimi",
                ProviderKind::Ernie => "ernie",
                ProviderKind::Doubao => "doubao",
                ProviderKind::Hunyuan => "hunyuan",
                ProviderKind::Yi => "yi",
                ProviderKind::Minimax => "minimax",
                ProviderKind::Baichuan => "baichuan",
                ProviderKind::Gigachat => "gigachat",
                ProviderKind::YandexGpt => "yandex_gpt",
                ProviderKind::CloudRu => "cloud_ru",
                ProviderKind::MtsAi => "mts_ai",
                ProviderKind::Naver => "naver",
                ProviderKind::Upstage => "upstage",
                ProviderKind::Rinna => "rinna",
                ProviderKind::Rakuten => "rakuten",
                ProviderKind::Sarvam => "sarvam",
                ProviderKind::Krutrim => "krutrim",
                ProviderKind::Falcon => "falcon",
                _ => "unknown",
            };
            if provider.api_key_env_name().is_none_or(str::is_empty) {
                problems.push(format!(
                    "{kind} provider '{}' requires api_key_env",
                    provider.name
                ));
            }
            if provider.has_non_env_credential() {
                problems.push(format!(
                    "{kind} provider '{}' must source its key from api_key_env",
                    provider.name
                ));
            }
        }
        // a provider's api_base is attacker-influenced whenever the admin
        // surface is: without this the gateway is an SSRF primitive aimed at
        // whatever its network position can reach
        if let Err(problem) = self.egress.check_url(
            &provider.api_base,
            &format!("provider '{}' api_base", provider.name),
        ) {
            problems.push(problem);
        }
        for proxy in provider.egress_proxy_pool() {
            if !is_proxy_reference(proxy) {
                problems.push(format!(
                    "provider '{}' has an invalid egress_proxy/egress_proxies entry '{}' (expected http(s)/socks5(h) URL or a whole-value ${{ENV_VAR}} reference without inline credentials)",
                    provider.name, proxy
                ));
            }
            if let Err(problem) = self
                .egress
                .check_url(proxy, &format!("provider '{}' egress proxy", provider.name))
            {
                problems.push(problem);
            }
        }
        // fetched by the gateway (the status poller, the lmcache refresh, the
        // kv-event subscriber) from a URL an operator wrote, so each is as
        // much an SSRF target as api_base
        if let Some(url) = &provider.status_page_url {
            if let Err(problem) = self.egress.check_url(
                url,
                &format!("provider '{}' status_page_url", provider.name),
            ) {
                problems.push(problem);
            }
        }
        if let Some(kv) = &provider.kv_events {
            if let Err(problem) = self.egress.check_url(
                &kv.endpoint,
                &format!("provider '{}' kv_events.endpoint", provider.name),
            ) {
                problems.push(problem);
            }
            problems.extend(kv.shape_problems(&provider.name));
        }
        if let Some(lmcache) = &provider.lmcache {
            if let Err(problem) = self.egress.check_url(
                &lmcache.endpoint,
                &format!("provider '{}' lmcache.endpoint", provider.name),
            ) {
                problems.push(problem);
            }
            problems.extend(lmcache.shape_problems(&provider.name));
        }
        problems
    }

    /// Egress-policy problems with the gateway-side URLs an operator writes
    /// outside a provider: the guardrail webhook, the PII sanitizer and each
    /// plugin endpoint. A disabled block is never a problem, matching the
    /// block's own `validate`. Provider URLs are covered by
    /// [`provider_problems`](Self::provider_problems).
    pub fn operator_url_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        // a local provider has no url: it runs in the gateway process (#3329)
        let webhook = crate::guardrail_webhook::GuardProviderKind::Webhook;
        if self.guardrail_webhook.enabled && self.guardrail_webhook.kind == webhook {
            problems.extend(
                self.egress_problem(self.guardrail_webhook.url.trim(), "guardrail_webhook.url"),
            );
        }
        for provider in self
            .semantic_guards
            .providers
            .iter()
            .filter(|p| p.enabled && p.kind == webhook)
        {
            problems.extend(self.egress_problem(
                provider.url.trim(),
                &format!("semantic_guards.providers '{}' url", provider.name),
            ));
        }
        if self.pii_sanitizer.enabled {
            problems
                .extend(self.egress_problem(self.pii_sanitizer.url.trim(), "pii_sanitizer.url"));
            problems.extend(self.egress_problem(
                self.pii_sanitizer.restore_url.trim(),
                "pii_sanitizer.restore_url",
            ));
            problems.extend(self.egress_problem(
                self.pii_sanitizer.restore_stream_url.trim(),
                "pii_sanitizer.restore_stream_url",
            ));
        }
        for instance in &self.plugins.instances {
            problems.extend(self.egress_problem(
                instance.endpoint.trim(),
                &format!("plugin '{}' endpoint", instance.slug),
            ));
        }
        problems
    }

    fn egress_problem(&self, url: &str, what: &str) -> Option<String> {
        self.egress.check_url(url, what).err()
    }

    pub fn validate(&self) -> std::result::Result<(), Vec<String>> {
        self.validate_with(true)
    }

    /// [`validate`](Self::validate) for a store-sourced snapshot, which keeps a
    /// guardrail webhook, PII sanitizer or plugin whose URL the egress policy denies
    /// (see [`sanitize_for_snapshot`](Self::sanitize_for_snapshot)). A file
    /// config goes through the strict `validate`.
    pub fn validate_snapshot(&self) -> std::result::Result<(), Vec<String>> {
        self.validate_with(false)
    }

    fn validate_with(&self, strict_urls: bool) -> std::result::Result<(), Vec<String>> {
        let mut problems = Vec::new();

        if let Err(mut ca_problems) = self.validate_ca_bundles() {
            problems.append(&mut ca_problems);
        }

        problems.extend(self.currency.problems());
        // a price whose currency has no rate cannot be converted into the base,
        // and there is no safe way to charge it: pricing it at face value bills
        // the wrong number, pricing it at zero bills nothing. reject the config
        // instead, so the failure lands on the operator rather than on spend
        for price in &self.model_prices {
            // a store-sourced snapshot has had these pruned already (see
            // `sanitize_for_snapshot`), so only a file config can fail here
            for problem in price.rate_problems() {
                problems.push(format!("model_prices['{}'] {problem}", price.model));
            }
            if self.currency.rate(&price.currency).is_none() {
                problems.push(format!(
                    "model_prices['{}'] is priced in '{}', which has no rate in [currency.rates] \
                     (base is '{}'); add a rate or price it in the base currency",
                    price.model, price.currency, self.currency.base
                ));
            }
        }

        // the metrics path must be a rooted path that does not shadow a built-in
        // request route
        let metrics_path = self.server.metrics_path.as_str();
        if !metrics_path.starts_with('/') {
            problems.push(format!(
                "server.metrics_path '{metrics_path}' must start with '/'"
            ));
        }
        if RESERVED_PATHS.contains(&metrics_path) {
            problems.push(format!(
                "server.metrics_path '{metrics_path}' collides with a built-in route"
            ));
        }

        // names are unique per org, never across orgs (#1857, ADR-0040)
        let mut provider_names = crate::namespace::Namespaced::new();
        for provider in &self.providers {
            let org = crate::namespace::org_of(provider.tenancy.as_ref());
            if provider.name.trim().is_empty() {
                problems.push("provider has an empty name".to_string());
            } else if provider_names.insert(org, &provider.name, ()).is_err() {
                problems.push(duplicate_name("provider name", &provider.name, org));
            }
            problems.extend(self.provider_problems(provider));
        }

        let mut mcp_server_keys = std::collections::HashSet::new();
        let mut mcp_server_ids = std::collections::HashSet::new();
        let mut mcp_servers_by_id = std::collections::HashMap::new();
        for server in &self.mcp_servers {
            if server.id.trim().is_empty()
                || server.org_id.trim().is_empty()
                || server.slug.trim().is_empty()
            {
                problems.push("MCP server id, org_id and slug must be non-empty".to_string());
            }
            if !mcp_server_keys.insert((server.org_id.as_str(), server.slug.as_str())) {
                problems.push(format!(
                    "duplicate MCP server slug '{}' in org '{}'",
                    server.slug, server.org_id
                ));
            }
            if !mcp_server_ids.insert(server.id.as_str()) {
                problems.push(format!("duplicate MCP server id '{}'", server.id));
            }
            if !is_http_url(&server.url) {
                problems.push(format!(
                    "MCP server '{}' has an invalid URL '{}'",
                    server.slug, server.url
                ));
            }
            if let Err(problem) = self
                .egress
                .check_url(&server.url, &format!("MCP server '{}' URL", server.slug))
            {
                problems.push(problem);
            }
            if !MCP_TRANSPORTS.contains(&server.transport.as_str()) {
                problems.push(format!(
                    "MCP server '{}' has unsupported transport '{}'",
                    server.slug, server.transport
                ));
            }
            let mut scopes = std::collections::HashSet::new();
            for scope in &server.required_scopes {
                if scope.trim().is_empty() || !scopes.insert(scope.as_str()) {
                    problems.push(format!(
                        "MCP server '{}' required scopes must be non-empty and unique",
                        server.slug
                    ));
                }
            }
            mcp_servers_by_id.insert(server.id.as_str(), server);
        }

        let mut mcp_session_keys = std::collections::HashSet::new();
        for session in &self.mcp_oauth_sessions {
            if session.id.trim().is_empty()
                || session.user_id.trim().is_empty()
                || session.access_token.is_empty()
            {
                problems.push(
                    "MCP OAuth session id, user_id and access_token must be non-empty".to_string(),
                );
            }
            let Some(server) = mcp_servers_by_id.get(session.server_id.as_str()) else {
                problems.push(format!(
                    "MCP OAuth session '{}' references unknown server '{}'",
                    session.id, session.server_id
                ));
                continue;
            };
            if !mcp_session_keys.insert((session.server_id.as_str(), session.user_id.as_str())) {
                problems.push(format!(
                    "multiple live MCP OAuth sessions exist for server '{}' and user '{}'",
                    session.server_id, session.user_id
                ));
            }
            if server
                .required_scopes
                .iter()
                .any(|scope| !session.scopes.contains(scope))
            {
                problems.push(format!(
                    "MCP OAuth session '{}' does not cover server '{}' required scopes",
                    session.id, server.slug
                ));
            }
        }

        let mut route_models = crate::namespace::Namespaced::new();
        for route in &self.routes {
            // a route's targets name providers of its own org first, then global
            let org = crate::namespace::org_of(route.tenancy.as_ref());
            let known = |provider: &str| provider_names.get_ref(org, provider).is_some();
            if route.model.trim().is_empty() {
                problems.push("route has an empty model name".to_string());
            } else if route_models.insert(org, &route.model, ()).is_err() {
                problems.push(duplicate_name("route model", &route.model, org));
            }
            if route.targets.is_empty() && route.variants.is_empty() {
                problems.push(format!(
                    "route '{}' has neither targets nor variants",
                    route.model
                ));
            }
            problems.extend(route.takes_over_problems());
            for target in &route.targets {
                if !known(&target.provider) {
                    problems.push(format!(
                        "route '{}' targets unknown provider '{}'",
                        route.model, target.provider
                    ));
                }
                if target.weight == 0 {
                    problems.push(format!(
                        "route '{}' target '{}' has zero weight",
                        route.model, target.provider
                    ));
                }
            }
            for variant in &route.variants {
                if variant.name.trim().is_empty() {
                    problems.push(format!(
                        "route '{}' has a variant with an empty name",
                        route.model
                    ));
                }
                if variant.targets.is_empty() {
                    problems.push(format!(
                        "route '{}' variant '{}' has no targets",
                        route.model, variant.name
                    ));
                }
                for target in &variant.targets {
                    if !known(&target.provider) {
                        problems.push(format!(
                            "route '{}' variant '{}' targets unknown provider '{}'",
                            route.model, variant.name, target.provider
                        ));
                    }
                }
            }
            if let Some(semantic) = route.cache.as_ref().and_then(|c| c.semantic.as_ref()) {
                if !known(&semantic.provider) {
                    problems.push(format!(
                        "route '{}' semantic cache uses unknown embeddings provider '{}'",
                        route.model, semantic.provider
                    ));
                }
                if semantic.model.trim().is_empty() {
                    problems.push(format!(
                        "route '{}' semantic cache has an empty embeddings model",
                        route.model
                    ));
                }
                if !(0.0..=1.0).contains(&semantic.threshold) || semantic.threshold == 0.0 {
                    problems.push(format!(
                        "route '{}' semantic cache threshold must be in (0, 1]",
                        route.model
                    ));
                }
                if semantic.max_candidates == 0 {
                    problems.push(format!(
                        "route '{}' semantic cache max_candidates must be greater than zero",
                        route.model
                    ));
                }
            }
        }

        let mut key_values = std::collections::HashSet::new();
        for vk in &self.virtual_keys {
            if vk.key.trim().is_empty() {
                problems.push("virtual key has an empty key value".to_string());
            } else if !key_values.insert(vk.key.as_str()) {
                problems.push("duplicate virtual key value".to_string());
            }
            // a file key has no org, so it names global providers, or the one
            // org's provider when only one org has that name
            for provider in &vk.providers {
                if !provider_names.contains_name(provider) {
                    problems.push(format!(
                        "virtual key '{}' allows unknown provider '{}'",
                        vk.name.as_deref().unwrap_or("<unnamed>"),
                        provider
                    ));
                }
            }
        }

        for budget in &self.budgets {
            // a Decimal is never NaN, so the emptiness check is now just sign
            if budget.limit_usd <= Decimal::ZERO {
                problems.push(format!(
                    "budget for {:?} '{}' has a non-positive limit_usd",
                    budget.scope, budget.id
                ));
            }
        }

        for limit in &self.rate_limits {
            if limit.rpm.is_none() && limit.tpm.is_none() {
                problems.push(format!(
                    "rate limit for {:?} '{}' sets neither rpm nor tpm (never caps)",
                    limit.scope, limit.id
                ));
            }
        }
        if !self.logging.sample_rate.is_finite() || !(0.0..=1.0).contains(&self.logging.sample_rate)
        {
            problems.push("logging.sample_rate must be in [0, 1]".to_string());
        }

        if self.queue.enabled && self.queue.capacity == 0 {
            problems.push(
                "queue.capacity must be greater than zero when queueing is enabled".to_string(),
            );
        }
        if self.queue.enabled && self.queue.workers == 0 {
            problems.push(
                "queue.workers must be greater than zero when queueing is enabled".to_string(),
            );
        }
        if self.queue.backpressure == BackpressurePolicy::Block && self.queue.block_timeout_ms == 0
        {
            problems.push(
                "queue.block_timeout_ms must be greater than zero when backpressure is block"
                    .to_string(),
            );
        }

        // compile guardrail rules under the safe-regex limits so an invalid or
        // unbounded pattern fails here at startup/snapshot validation, never on
        // the request path
        problems.append(&mut self.guardrails.validate());

        // a route override naming a rule that does not exist is a typo, and a
        // typo in `disable` reads as "this rule is off here" while the rule
        // keeps running — exactly the wrong way for a security control to fail
        let rule_names: Vec<String> = self
            .guardrails
            .rules
            .iter()
            .map(|rule| rule.name.trim().to_string())
            .collect();
        for route in &self.routes {
            for unknown in route.advanced.guardrails.unknown_rules(&rule_names) {
                problems.push(format!(
                    "route '{}' overrides guardrail rule '{}', which is not defined in [guardrails]",
                    route.model, unknown
                ));
            }
        }

        // a file config is strict about route headers: one the wire cannot
        // carry, or that would replace the provider's credential, is a mistake
        // to fix, where the snapshot path drops it and says so (#2924)
        for route in &self.routes {
            for problem in route.advanced.header_problems() {
                problems.push(format!("route '{}': {problem}", route.model));
            }
        }
        for route in &self.routes {
            for problem in route.advanced.limits.throughput_problems() {
                problems.push(format!("route '{}': {problem}", route.model));
            }
            if route.advanced.context_length == Some(0) {
                problems.push(format!(
                    "route '{}': context_length must be at least 1",
                    route.model
                ));
            }
        }
        // the same split for the request id header: a file naming a credential
        // or framing header is refused, the snapshot path resets it (#2925)
        if let Some(problem) = request_id_header_problem(&self.client.request_id_header) {
            problems.push(format!("client.request_id_header: {problem}"));
        }

        // validate the custom guardrail webhook (url/timeout/auth) at load time
        problems.append(&mut self.guardrail_webhook.validate());
        problems.append(&mut self.semantic_guards.validate(&self.guardrail_webhook));
        problems.append(&mut self.pii_sanitizer.validate());

        // validate every enabled plugin instance's endpoint at load time
        problems.append(&mut self.plugins.validate());
        if strict_urls {
            problems.append(&mut self.operator_url_problems());
        }

        // validate prompt templates: unique versions, well-formed variables, and
        // decorator placeholders that reference only declared variables
        problems.append(&mut self.prompt_templates.validate());

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

/// The providers [`GatewayConfig::sanitize_for_snapshot`] left out, and how a
/// reference to one is recognised: it is resolved the way the gateway would,
/// in the referencing row's org first, then globally, against every provider
/// the config held before any was dropped (#1857). A name that resolves to
/// nothing is left out when the store loader left out a provider by that name
/// (#2950).
struct OmittedProviders {
    every: crate::namespace::Namespaced<()>,
    omitted: crate::namespace::Namespaced<()>,
}

impl OmittedProviders {
    /// `every` is each provider as `(org, name)` before any was dropped,
    /// `omitted` the ones the sanitizer dropped, and `left_out` the ones the
    /// store loader never returned (#2950). A left-out provider joins both
    /// tables, so a reference resolves to it by the same `(org, name)` rule as
    /// to any other row and is pruned, rather than falling through to a
    /// namesake: another org's (#3464), or a global row its own org's row
    /// would have shadowed.
    fn new(
        mut every: crate::namespace::Namespaced<()>,
        omitted: Vec<(String, String)>,
        left_out: &std::collections::HashSet<(String, String)>,
    ) -> Self {
        let mut set = crate::namespace::Namespaced::new();
        for (org, name) in &omitted {
            let _ = set.insert(org, name, ());
        }
        for (org, name) in left_out {
            // a provider the config still carries as `(org, name)` is that
            // provider, not the row the store could not read
            if every.owned_by(org, name).is_some() {
                continue;
            }
            let _ = every.insert(org, name, ());
            let _ = set.insert(org, name, ());
        }
        Self {
            every,
            omitted: set,
        }
    }

    fn is_empty(&self) -> bool {
        self.omitted.is_empty()
    }

    /// Why a row of `org` may not use the provider it calls `name`, if it
    /// may not: the provider it resolves to (its org's, then a global one)
    /// was left out, or the only providers by that name belong to other orgs,
    /// which a row never reaches (ADR-0040 section 1). The second case is a
    /// deployment-wide row from the bootstrap file naming a database provider,
    /// pruned rather than refused so the snapshot keeps building.
    fn refusal(&self, org: &str, name: &str) -> Option<&'static str> {
        match self.every.resolve_ref(org, name) {
            Some((owner, ())) => self
                .omitted
                .owned_by(owner, name)
                .map(|()| "the provider is itself omitted from the snapshot"),
            None if self.every.contains_name(name) && org.is_empty() => Some(
                "a deployment-wide row uses deployment-wide providers only, and this provider \
                 belongs to an organization",
            ),
            None if self.every.contains_name(name) => {
                Some("the provider belongs to another organization, which this row cannot use")
            }
            None => None,
        }
    }

    /// The names in a key's allow-list that reach a provider left out, for a
    /// key of `org`. A key is a caller, so it resolves the way the gateway
    /// resolves one: the sole org's provider is a match for a key with no org.
    fn names_for(&self, org: &str, names: &[String]) -> std::collections::HashSet<String> {
        names
            .iter()
            .filter(|name| match self.every.resolve(org, name) {
                Some((owner, ())) => self.omitted.owned_by(owner, name).is_some(),
                None => false,
            })
            .cloned()
            .collect()
    }
}

/// The problem line for a name its namespace holds twice: `what` is the kind,
/// such as `provider name`, and `org` is empty for the global namespace.
fn duplicate_name(what: &str, name: &str, org: &str) -> String {
    if org.is_empty() {
        format!("duplicate {what} '{name}'")
    } else {
        format!("duplicate {what} '{name}' in org '{org}'")
    }
}

/// Take the omitted providers out of one virtual key's allow-list, returning
/// why the key must leave the snapshot, or `None` while it stays (#2945).
///
/// `label` names the key in the problem line and must never be the secret.
/// The list is edited only while it keeps an entry: emptying it would let the
/// key reach every provider, so a key that allowed nothing but omitted
/// providers is denied by leaving it out.
fn prune_key_allow_list(
    providers: &mut Vec<String>,
    label: &str,
    omitted: &std::collections::HashSet<String>,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let named: Vec<String> = providers
        .iter()
        .filter(|name| omitted.contains(name.as_str()))
        .cloned()
        .collect();
    if named.is_empty() {
        return None;
    }
    let every_entry = named.len() == providers.len();
    let named = named.join("', '");
    if every_entry {
        let reason = format!(
            "it allows only omitted providers ('{named}'), and an empty allow-list would let \
             it reach every provider"
        );
        warnings.push(format!(
            "virtual key '{label}' omitted from the snapshot: {reason}"
        ));
        return Some(reason);
    }
    providers.retain(|name| !omitted.contains(name.as_str()));
    warnings.push(format!(
        "virtual key '{label}' allow-list entries '{named}' omitted from the snapshot: the \
         providers are themselves omitted, and the key stays limited to its other providers"
    ));
    None
}

fn validate_ca_bundle(path: &Path) -> std::result::Result<(), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("CA bundle '{}' cannot be read: {error}", path.display()))?;
    let certs: std::result::Result<Vec<_>, _> = CertificateDer::pem_slice_iter(&bytes).collect();
    match certs {
        Ok(certs) if !certs.is_empty() => Ok(()),
        Ok(_) => Err(format!(
            "CA bundle '{}' contains no PEM certificates",
            path.display()
        )),
        Err(error) => Err(format!(
            "CA bundle '{}' contains malformed PEM: {error}",
            path.display()
        )),
    }
}

/// Whether `s` is a plausible `http`/`https` URL with a non-empty host.
/// Where the gateway is allowed to send upstream traffic.
///
/// rolter's primary deployment is self-hosted and frequently air-gapped, so
/// upstreams legitimately live on loopback, RFC1918 and container networks —
/// blanket-denying private destinations would break the core use case. What no
/// legitimate LLM upstream ever needs is the **link-local** range, which is
/// where cloud instance-metadata services live (`169.254.169.254`,
/// `fd00:ec2::254`). That is the SSRF prize, so it is denied by default and the
/// broader ranges are opt-in.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EgressPolicy {
    /// deny link-local destinations (`169.254.0.0/16`, `fe80::/10`), which
    /// includes cloud instance metadata. on by default
    #[serde(default = "default_true")]
    pub block_link_local: bool,
    /// deny loopback destinations (`127.0.0.0/8`, `::1`). off by default:
    /// sidecar and single-host deployments serve models on localhost
    #[serde(default)]
    pub block_loopback: bool,
    /// deny private/unique-local destinations (`10/8`, `172.16/12`,
    /// `192.168/16`, `fc00::/7`). off by default: on-prem clusters are the
    /// common case
    #[serde(default)]
    pub block_private: bool,
    /// hosts exempt from every check above, matched on the URL host verbatim
    /// (an ip literal or hostname). the escape hatch for a deployment that must
    /// reach an otherwise-denied address
    #[serde(default)]
    pub allow_hosts: Vec<String>,
}

impl Default for EgressPolicy {
    fn default() -> Self {
        Self {
            block_link_local: true,
            block_loopback: false,
            block_private: false,
            allow_hosts: Vec::new(),
        }
    }
}

/// Extract the host portion of a URL by hand: strips the scheme, any
/// userinfo, then the path/query, then a port (or brackets for an IPv6
/// literal). Only the fallback for a URL the WHATWG parser rejects; see
/// [`url_destination`].
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())?;
    // userinfo@host
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(rest) = authority.strip_prefix('[') {
        // [ipv6]:port
        return rest.split(']').next().filter(|h| !h.is_empty());
    }
    authority.split(':').next().filter(|h| !h.is_empty())
}

/// Parse one host the way a special (`http`, `https`) URL's host is parsed:
/// percent-decoded, IDNA-mapped, and read as an IPv4 address in any of the
/// WHATWG spellings (`2852039166`, `0xa9fea9fe`, `169.254.43518`, octal parts,
/// a trailing dot). Accepts an IPv6 literal with or without brackets.
fn parse_host(host: &str) -> Option<url::Host> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(v6) = bare.parse::<std::net::Ipv6Addr>() {
        return Some(url::Host::Ipv6(v6));
    }
    url::Host::parse(host).ok()
}

/// The host a request to `url` is sent to, read the way the HTTP client reads
/// it.
///
/// reqwest parses with the WHATWG `url` crate, which normalizes the host
/// before anything connects: `http://2852039166/` and
/// `http://169.254.169.254./` both dial `169.254.169.254`, and a backslash
/// ends the authority, so the host of `http://169.254.169.254\@example.com/`
/// is the address, not `example.com`. Classifying anything other than that
/// parse checks a host the request never goes to.
fn url_destination(url: &str) -> Option<url::Host> {
    match url::Url::parse(url) {
        Ok(parsed) => match parsed.host()? {
            // a scheme outside the WHATWG special set (socks5, socks5h) keeps
            // its host opaque, so an IPv4 literal in any spelling arrives as a
            // domain; the client or the system resolver reads it as an address
            url::Host::Domain(domain) => {
                Some(parse_host(domain).unwrap_or_else(|| url::Host::Domain(domain.to_string())))
            }
            host => Some(host.to_owned()),
        },
        // nothing can send to a URL the parser rejects, but the hand-rolled
        // read keeps every literal the check caught before it had a parser
        Err(_) => url_host(url)
            .map(|host| parse_host(host).unwrap_or_else(|| url::Host::Domain(host.to_string()))),
    }
}

/// The address a host literal dials: an IPv4-mapped IPv6 address
/// (`::ffff:a.b.c.d`) is dialled as the IPv4 address it carries.
fn host_ip(host: &url::Host) -> Option<std::net::IpAddr> {
    match host {
        url::Host::Ipv4(v4) => Some(std::net::IpAddr::V4(*v4)),
        url::Host::Ipv6(v6) => Some(std::net::IpAddr::V6(*v6).to_canonical()),
        url::Host::Domain(_) => None,
    }
}

impl EgressPolicy {
    /// Reject `url` when its host is an IP literal in a denied range.
    ///
    /// The URL is parsed with the WHATWG parser reqwest uses, so the host
    /// classified is the one the client connects to and every spelling of a
    /// denied address is denied: `http://2852039166/`, `http://0xa9fea9fe/`,
    /// `http://169.254.169.254./` and `http://[::ffff:169.254.169.254]/` are
    /// all `169.254.169.254`. Only literals are classified: resolving a hostname here would make
    /// config validation depend on live DNS, which would reintroduce exactly
    /// the "one bad row freezes every gateway" failure mode that
    /// [`GatewayConfig::sanitize_for_snapshot`] exists to avoid (and would be
    /// bypassable by rebinding anyway). Hostname resolution belongs at connect
    /// time; see the connect-time follow-up.
    pub fn check_url(&self, url: &str, what: &str) -> std::result::Result<(), String> {
        match self.url_deny_reason(url) {
            Some(range) => Err(format!(
                "{what} '{url}' resolves to a {range} address, which the egress policy denies \
                 (allow it explicitly via egress.allow_hosts if this is intentional)"
            )),
            None => Ok(()),
        }
    }

    /// Why this policy denies `url`, or `None` when it is permitted: the
    /// decision [`Self::check_url`] makes, without the URL in the message, for
    /// a caller that must not log an endpoint that may carry a token.
    pub fn url_deny_reason(&self, url: &str) -> Option<&'static str> {
        let host = url_destination(url)?;
        let ip = host_ip(&host)?;
        if self.allows(&host, ip) {
            return None;
        }
        self.deny_reason(ip)
    }

    /// Whether an `allow_hosts` entry names the address `url` reaches. An entry
    /// is read the same way the URL is, so `169.254.169.254` also covers
    /// `0xa9fea9fe`, the same destination.
    fn allows(&self, host: &url::Host, ip: std::net::IpAddr) -> bool {
        self.allow_hosts.iter().any(|entry| {
            parse_host(entry).is_some_and(|allowed| {
                allowed == *host || host_ip(&allowed).is_some_and(|allowed| allowed == ip)
            })
        })
    }

    /// Drop the resolved addresses this policy denies, for a connect-time
    /// resolver: `host` is the name that was resolved and `addrs` what DNS
    /// answered.
    ///
    /// A partial denial still connects: a multi-homed upstream with one denied
    /// address is reachable on the others, and refusing the whole name would
    /// take down a legitimate provider. Only a name left with *nothing* is an
    /// error, and rebinding to a denied address leaves exactly nothing. An
    /// empty answer is not a denial; that is the resolver's own failure to
    /// report.
    pub fn filter_resolved(
        &self,
        host: &str,
        addrs: Vec<std::net::SocketAddr>,
    ) -> std::result::Result<Vec<std::net::SocketAddr>, String> {
        if self.host_is_allowed(host) {
            return Ok(addrs);
        }
        let mut denied: Option<&'static str> = None;
        let allowed: Vec<std::net::SocketAddr> = addrs
            .into_iter()
            .filter(|addr| match self.deny_reason(addr.ip()) {
                Some(reason) => {
                    denied = Some(reason);
                    false
                }
                None => true,
            })
            .collect();
        if allowed.is_empty() {
            if let Some(reason) = denied {
                return Err(format!(
                    "'{host}' resolves only to {reason} addresses, which the egress policy denies \
                     (allow it explicitly via egress.allow_hosts if this is intentional)"
                ));
            }
        }
        Ok(allowed)
    }

    /// Whether `host` is exempt from every check, matched against
    /// `allow_hosts` verbatim or as the same normalized host (an IP literal or
    /// a hostname).
    pub fn host_is_allowed(&self, host: &str) -> bool {
        let normalized = parse_host(host);
        self.allow_hosts
            .iter()
            .any(|entry| entry == host || (normalized.is_some() && parse_host(entry) == normalized))
    }

    /// Why this policy denies `ip`, or `None` when it is permitted.
    ///
    /// Split out from [`Self::check_url`] so a connect-time check can classify
    /// an address DNS produced, which is the only rebinding-safe point to do
    /// it (see the connect-time resolver in `rolter-proxy`). An IPv4-mapped
    /// IPv6 address is classified as the IPv4 address it carries, and the
    /// unspecified address counts as loopback, since connecting to it reaches
    /// the local host.
    pub fn deny_reason(&self, ip: std::net::IpAddr) -> Option<&'static str> {
        match ip.to_canonical() {
            std::net::IpAddr::V4(v4) => {
                if v4.is_link_local() && self.block_link_local {
                    Some("link-local (cloud instance metadata lives here)")
                } else if (v4.is_loopback() || v4.is_unspecified()) && self.block_loopback {
                    Some("loopback")
                } else if v4.is_private() && self.block_private {
                    Some("private")
                } else {
                    None
                }
            }
            std::net::IpAddr::V6(v6) => {
                // unicast link-local is fe80::/10; unique-local is fc00::/7
                let link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
                let unique_local = (v6.segments()[0] & 0xfe00) == 0xfc00;
                if link_local && self.block_link_local {
                    Some("link-local (cloud instance metadata lives here)")
                } else if (v6.is_loopback() || v6.is_unspecified()) && self.block_loopback {
                    Some("loopback")
                } else if unique_local && self.block_private {
                    Some("private")
                } else {
                    None
                }
            }
        }
    }
}

fn is_http_url(s: &str) -> bool {
    for scheme in ["http://", "https://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            return !rest.is_empty() && !rest.starts_with('/');
        }
    }
    false
}

/// Whether `s` is a plausible outbound-proxy URL (`http`, `https`, `socks5`, or
/// `socks5h` scheme with a non-empty host).
fn is_proxy_url(s: &str) -> bool {
    for scheme in ["http://", "https://", "socks5://", "socks5h://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            return !rest.is_empty() && !rest.starts_with('/');
        }
    }
    false
}

fn is_proxy_reference(s: &str) -> bool {
    if let Some(name) = s.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
        return !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    }
    if !is_proxy_url(s) {
        return false;
    }
    let authority = s.split_once("://").map(|(_, v)| v).unwrap_or_default();
    !authority
        .split('/')
        .next()
        .unwrap_or_default()
        .contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #3332: a fixture's `..Default::default()` must mean what an omitted key
    /// means in a file. A derived default would give weight 0, which no
    /// balancer ever picks.
    #[test]
    fn a_default_target_is_what_a_target_with_only_a_provider_parses_to() {
        let parsed: Target = toml::from_str("provider = \"p\"").unwrap();
        let default = Target {
            provider: "p".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&default).unwrap(),
            serde_json::to_value(&parsed).unwrap()
        );
        assert_eq!(Target::default().weight, 1);
    }

    /// #2936: a route's throughput caps need a value that can be met. Zero
    /// would refuse every request, so a file is told and a stored row is
    /// served without that cap.
    #[test]
    fn a_zero_or_oversized_route_throughput_cap_is_a_problem_and_is_dropped_from_a_snapshot() {
        let mut limits = ModelLimits {
            rpm: Some(0),
            tpm: Some(500),
            concurrency: Some(MAX_ROUTE_THROUGHPUT + 1),
            ..Default::default()
        };
        let problems = limits.throughput_problems();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("limits.rpm"), "{problems:?}");
        assert!(
            problems[1].starts_with("limits.concurrency"),
            "{problems:?}"
        );

        assert_eq!(limits.drop_unusable_throughput().len(), 2);
        assert_eq!(
            (limits.rpm, limits.tpm, limits.concurrency),
            (None, Some(500), None)
        );
        assert!(limits.throughput_problems().is_empty());
    }

    #[test]
    fn route_throughput_caps_round_trip_and_older_blobs_read_as_unset() {
        let limits: ModelLimits =
            serde_json::from_str(r#"{"rpm":60,"tpm":9000,"concurrency":4}"#).unwrap();
        assert_eq!(
            (limits.rpm, limits.tpm, limits.concurrency),
            (Some(60), Some(9000), Some(4))
        );
        let back = serde_json::to_value(&limits).unwrap();
        assert_eq!(back["rpm"], 60);

        // a blob from before the keys existed, and one that only sets a timeout
        let older: ModelLimits = serde_json::from_str(r#"{"timeout_secs":30}"#).unwrap();
        assert_eq!(
            (older.rpm, older.tpm, older.concurrency),
            (None, None, None)
        );
        assert!(serde_json::to_value(&older).unwrap().get("rpm").is_none());
    }

    #[test]
    fn a_zero_context_length_is_a_config_problem() {
        let config = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com"

            [[routes]]
            model = "chat"
            [routes.advanced]
            context_length = 0
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        let problems = config.validate().unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("context_length")),
            "{problems:?}"
        );
    }

    #[test]
    fn only_context_window_is_still_a_retired_limit_key() {
        let mut advanced = serde_json::json!({
            "limits": {"rpm": 1, "tpm": 2, "concurrency": 3, "context_window": 4}
        });
        assert_eq!(
            strip_retired_advanced_keys(&mut advanced),
            ["limits.context_window"]
        );
        assert_eq!(advanced["limits"]["rpm"], 1);
    }

    #[test]
    fn a_file_config_with_a_zero_route_cap_fails_validation_and_a_snapshot_prunes_it() {
        let mut config = GatewayConfig::default();
        config.providers.push(ProviderConfig {
            name: "up".into(),
            api_base: "http://127.0.0.1:1".into(),
            ..Default::default()
        });
        let mut route: ModelRoute = serde_json::from_value(serde_json::json!({
            "model": "m", "strategy": "round_robin",
            "targets": [{"provider": "up", "weight": 1}]
        }))
        .unwrap();
        route.advanced.limits.concurrency = Some(0);
        config.routes.push(route);

        let problems = config
            .validate()
            .expect_err("a zero cap is refused in a file");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("route 'm'") && p.contains("limits.concurrency")),
            "{problems:?}"
        );
        let report = config.sanitize_for_snapshot();
        assert!(
            report.iter().any(|w| w.contains("limits.concurrency")),
            "{report:?}"
        );
        assert_eq!(config.routes[0].advanced.limits.concurrency, None);
    }

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(y, m, d, h, 30, 0)
            .single()
            .expect("a valid time")
    }

    #[test]
    fn a_window_resets_at_the_start_of_the_next_one() {
        assert_eq!(
            BudgetPeriod::Daily.next_reset(at(2026, 10, 11, 23)),
            Some(at(2026, 10, 12, 0) - chrono::Duration::minutes(30))
        );
        assert_eq!(
            BudgetPeriod::Monthly.next_reset(at(2026, 12, 31, 23)),
            Some(at(2027, 1, 1, 0) - chrono::Duration::minutes(30))
        );
        assert_eq!(BudgetPeriod::Total.next_reset(at(2026, 1, 1, 0)), None);
    }

    #[test]
    fn a_narrowed_user_cap_keeps_its_own_counter() {
        let cap = |within: Option<ScopeWithin>| BudgetConfig {
            scope: BudgetScope::User,
            id: "u-1".into(),
            limit_usd: Default::default(),
            period: BudgetPeriod::Monthly,
            unpriced_policy: None,
            within,
        };
        let now = at(2026, 10, 11, 1);
        let keys = [
            cap(None).spend_key(now),
            cap(Some(ScopeWithin::Org("o".into()))).spend_key(now),
            cap(Some(ScopeWithin::Project("o".into()))).spend_key(now),
        ];
        assert_eq!(keys[0], "rolter:budget:user:u-1:202610");
        assert_eq!(keys[1], "rolter:budget:user:u-1:org:o:202610");
        assert_ne!(keys[1], keys[2], "an org and a project of one id differ");
    }

    #[test]
    fn the_spend_key_names_scope_id_and_window() {
        let budget = BudgetConfig {
            scope: BudgetScope::Key,
            id: "abc".into(),
            limit_usd: Default::default(),
            period: BudgetPeriod::Monthly,
            unpriced_policy: None,
            within: None,
        };
        assert_eq!(
            budget.spend_key(at(2026, 10, 11, 1)),
            "rolter:budget:key:abc:202610"
        );
        assert_eq!(
            BudgetScope::from_scope_type("virtual_key"),
            Some(BudgetScope::Key)
        );
        assert_eq!(BudgetScope::from_scope_type("nope"), None);
    }

    /// A decimal literal for tests. `rust_decimal`'s `dec!` macro would read
    /// slightly better, but its `macros` feature pulls `rust_decimal_macros`,
    /// `proc-macro-crate`, `toml_edit` and `borsh` into the dependency graph in
    /// production position, which is a poor trade for test ergonomics (#967).
    fn d(literal: &str) -> rust_decimal::Decimal {
        literal.parse().expect("a valid decimal literal")
    }

    // --- api_base / /v1 semantics (#947) ---

    /// `ProviderKind::ALL` is hand-written, so it can fall behind the enum.
    /// This match is exhaustive: a new variant fails to compile until it is
    /// added here, and the assertion then catches the missing `ALL` entry.
    #[test]
    fn kind_is_listed_in_all() {
        for kind in ProviderKind::ALL {
            // exhaustive by construction — the compiler enforces the arms
            let named = match kind {
                ProviderKind::Openai => "openai",
                ProviderKind::Anthropic => "anthropic",
                ProviderKind::OpenaiCompatible => "openai_compatible",
                ProviderKind::Ollama => "ollama",
                ProviderKind::OllamaCloud => "ollama_cloud",
                ProviderKind::LlamaCpp => "llama_cpp",
                ProviderKind::Openrouter => "openrouter",
                ProviderKind::Tei => "tei",
                ProviderKind::AzureOpenai => "azure_openai",
                ProviderKind::Bedrock => "bedrock",
                ProviderKind::Vertex => "vertex",
                ProviderKind::Gemini => "gemini",
                ProviderKind::GeminiNative => "gemini_native",
                ProviderKind::GeminiInteractions => "gemini_interactions",
                ProviderKind::Mistral => "mistral",
                ProviderKind::Groq => "groq",
                ProviderKind::Xai => "xai",
                ProviderKind::MetaLlamaApi => "meta_llama_api",
                ProviderKind::Cohere => "cohere",
                ProviderKind::Perplexity => "perplexity",
                ProviderKind::Together => "together",
                ProviderKind::Fireworks => "fireworks",
                ProviderKind::Databricks => "databricks",
                ProviderKind::AlephAlpha => "aleph_alpha",
                ProviderKind::Nebius => "nebius",
                ProviderKind::Ovhcloud => "ovhcloud",
                ProviderKind::Scaleway => "scaleway",
                ProviderKind::Deepseek => "deepseek",
                ProviderKind::Qwen => "qwen",
                ProviderKind::Zhipu => "zhipu",
                ProviderKind::Kimi => "kimi",
                ProviderKind::Ernie => "ernie",
                ProviderKind::Doubao => "doubao",
                ProviderKind::Hunyuan => "hunyuan",
                ProviderKind::Yi => "yi",
                ProviderKind::Minimax => "minimax",
                ProviderKind::Baichuan => "baichuan",
                ProviderKind::Gigachat => "gigachat",
                ProviderKind::YandexGpt => "yandex_gpt",
                ProviderKind::CloudRu => "cloud_ru",
                ProviderKind::MtsAi => "mts_ai",
                ProviderKind::Naver => "naver",
                ProviderKind::Upstage => "upstage",
                ProviderKind::Rinna => "rinna",
                ProviderKind::Rakuten => "rakuten",
                ProviderKind::Sarvam => "sarvam",
                ProviderKind::Krutrim => "krutrim",
                ProviderKind::Falcon => "falcon",
                ProviderKind::SystemOne => "system_one",
                ProviderKind::Infinity => "infinity",
                // deliberately absent from `ALL`: it names no kind anyone can
                // configure, only one a newer control plane sent (#3347)
                ProviderKind::Unknown => unreachable!("Unknown is not listed in ALL"),
            };
            assert!(!named.is_empty());
        }
        assert!(!ProviderKind::ALL.contains(&ProviderKind::Unknown));
        assert_eq!(ProviderKind::ALL.len(), 50);
    }

    /// #2806: the scheme per kind, checked across every kind so a new variant
    /// that needs a raw header instead of the bearer default has to be named.
    #[test]
    fn every_kind_presents_its_key_the_way_its_api_reads_it() {
        for kind in ProviderKind::ALL {
            let (name, value) = kind.auth_header("sk-secret");
            match kind {
                ProviderKind::Anthropic => {
                    assert_eq!((name, &*value), ("x-api-key", "sk-secret"), "{kind:?}")
                }
                ProviderKind::AzureOpenai => {
                    assert_eq!((name, &*value), ("api-key", "sk-secret"), "{kind:?}")
                }
                ProviderKind::GeminiNative | ProviderKind::GeminiInteractions => {
                    assert_eq!((name, &*value), ("x-goog-api-key", "sk-secret"), "{kind:?}")
                }
                _ => assert_eq!(
                    (name, &*value),
                    ("authorization", "Bearer sk-secret"),
                    "{kind:?}"
                ),
            }
        }
    }

    #[test]
    fn a_raw_scheme_borrows_the_key_and_only_bearer_allocates() {
        let key = "sk-secret";
        assert!(matches!(
            ProviderKind::Anthropic.auth_header(key).1,
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            ProviderKind::Openai.auth_header(key).1,
            std::borrow::Cow::Owned(_)
        ));
    }

    #[test]
    fn an_openai_shaped_base_must_not_carry_v1() {
        let kind = ProviderKind::Openai;
        assert!(!kind.base_includes_v1());
        assert_eq!(
            kind.resolve_upstream_url("https://api.openai.com", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        // the exact failure from #947, seen against a real GPUStack instance
        assert_eq!(
            kind.resolve_upstream_url("https://gpustack.localhost/v1", "/v1/chat/completions"),
            "https://gpustack.localhost/v1/v1/chat/completions"
        );
    }

    #[test]
    fn a_stripping_kind_needs_v1_in_the_base() {
        let kind = ProviderKind::Mistral;
        assert!(kind.base_includes_v1());
        assert_eq!(
            kind.resolve_upstream_url("https://api.mistral.ai/v1", "/v1/chat/completions"),
            "https://api.mistral.ai/v1/chat/completions"
        );
    }

    #[test]
    fn a_trailing_slash_never_doubles_the_separator() {
        assert_eq!(
            ProviderKind::Openai.resolve_upstream_url("https://host/", "/v1/models"),
            "https://host/v1/models"
        );
    }

    #[test]
    fn the_doubling_problem_is_reported_only_where_it_is_one() {
        // openai-shaped: a trailing /v1 is the bug
        let problem = ProviderKind::Openai
            .api_base_problem("https://gpustack.localhost/v1")
            .expect("doubled base is a problem");
        assert!(problem.contains("/v1/v1/chat/completions"), "{problem}");

        // same spelling, stripping kind: required, not a bug
        assert!(ProviderKind::Mistral
            .api_base_problem("https://api.mistral.ai/v1")
            .is_none());

        // well-formed and empty bases are quiet
        assert!(ProviderKind::Openai
            .api_base_problem("https://api.openai.com")
            .is_none());
        assert!(ProviderKind::Openai.api_base_problem("").is_none());
        // a trailing slash must not hide the defect
        assert!(ProviderKind::Openai
            .api_base_problem("https://gpustack.localhost/v1/")
            .is_some());
        // /v1beta is not /v1
        assert!(ProviderKind::Openai
            .api_base_problem("https://host/v1beta")
            .is_none());
    }

    /// The contract the dashboard's live preview reimplements in TypeScript:
    /// for every kind, resolution is exactly "append the path" or "append the
    /// path minus its /v1", chosen by `base_includes_v1`. A kind that resolved
    /// some third way would make the preview lie.
    #[test]
    fn every_kind_resolves_by_the_documented_rule() {
        for kind in ProviderKind::ALL {
            let resolved = kind.resolve_upstream_url("https://host", "/v1/chat/completions");
            let expected = if kind.base_includes_v1() {
                "https://host/chat/completions"
            } else {
                "https://host/v1/chat/completions"
            };
            assert_eq!(resolved, expected, "{kind:?} resolves off-rule");
        }
    }

    fn route_with(params: &[(&str, serde_json::Value)], policy: ParamPolicy) -> ModelRoute {
        ModelRoute {
            model: "m".to_string(),
            strategy: BalancingStrategy::default(),
            targets: vec![],
            params: params
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            param_policy: policy,
            advanced: Default::default(),
            cache: None,
            variants: vec![],
            tenancy: None,
        }
    }

    #[test]
    fn advanced_model_configuration_roundtrips() {
        let route: ModelRoute = serde_json::from_value(serde_json::json!({
            "model": "vision",
            "targets": [],
            "advanced": {
                "model_type": "image",
                "capabilities": ["vision"],
                "limits": {"output_tokens": 2048, "timeout_secs": 30, "retries": 1},
                "headers": {"x-model-region": "eu"},
                "locked_headers": ["x-model-region"],
                "visibility": {"allowed_key_ids": ["key-1"]}
            }
        }))
        .unwrap();
        assert_eq!(route.advanced.model_type.as_deref(), Some("image"));
        assert_eq!(route.advanced.limits.output_tokens, Some(2048));
        assert_eq!(route.advanced.limits.timeout_secs, Some(30));
        assert_eq!(route.advanced.limits.retries, Some(1));
        assert_eq!(
            serde_json::to_value(route).unwrap()["advanced"]["headers"]["x-model-region"],
            "eu"
        );
    }

    /// A stored `advanced` blob written before #2924 still loads.
    ///
    /// `base_url`, `insecure_tls`, `pricing` and `context_window` were
    /// persisted into route rows and read by nothing, and are gone from the
    /// struct. A row that carries them has to keep deserializing whole, with
    /// the keys ignored instead of stopping the parse (so the fields around
    /// them survive) and with nothing writing them back out. Without this a
    /// route saved by the dashboard last month would load as the default and
    /// lose its visibility restriction.
    #[test]
    fn an_advanced_block_carrying_the_retired_keys_still_loads() {
        let route: ModelRoute = serde_json::from_value(serde_json::json!({
            "model": "vision",
            "targets": [],
            "advanced": {
                "base_url": "https://models.example/v1",
                "insecure_tls": true,
                "pricing": {"image_per_unit": 0.04, "audio_input_per_minute": 0.1},
                "limits": {
                    "context_window": 128000,
                    "timeout_secs": 45, "retries": 3, "output_tokens": 2048
                },
                "headers": {"x-model-region": "eu"},
                "visibility": {"allowed_key_ids": ["key-1"]}
            }
        }))
        .expect("an old stored blob must still deserialize");
        assert_eq!(route.advanced.limits.timeout_secs, Some(45));
        assert_eq!(route.advanced.limits.retries, Some(3));
        assert_eq!(route.advanced.limits.output_tokens, Some(2048));
        assert_eq!(route.advanced.visibility.allowed_key_ids, ["key-1"]);

        let written = serde_json::to_value(&route).unwrap();
        for key in ["base_url", "insecure_tls", "pricing"] {
            assert!(
                written["advanced"].get(key).is_none(),
                "the retired key {key} must not be re-emitted: {written}"
            );
        }
        assert!(
            written["advanced"]["limits"]
                .get("context_window")
                .is_none(),
            "the retired limit context_window must not be re-emitted: {written}"
        );
    }

    /// A file that still sets a retired key is told so: the loader's unknown
    /// key report is how `rolter check` and the gateway's startup log say that
    /// `insecure_tls = true` is not doing what the file's author believes.
    #[test]
    fn a_file_setting_a_retired_advanced_key_is_reported_as_unknown() {
        let toml = r#"
            [[providers]]
            name = "p"
            kind = "openai_compatible"
            api_base = "http://127.0.0.1:1"

            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "p"
            [routes.advanced]
            base_url = "https://elsewhere.example"
            insecure_tls = true
            [routes.advanced.limits]
            context_window = 128000
            rpm = 60
            timeout_secs = 30
        "#;
        let paths: Vec<String> = crate::unknown_keys(toml)
            .unwrap()
            .into_iter()
            .map(|key| key.path)
            .collect();
        for expected in [
            "routes[0].advanced.base_url",
            "routes[0].advanced.insecure_tls",
            "routes[0].advanced.limits.context_window",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "{expected} in {paths:?}"
            );
        }
        assert!(
            !paths
                .iter()
                .any(|p| p.ends_with("timeout_secs") || p.ends_with("limits.rpm")),
            "a limit the gateway applies is not unknown: {paths:?}"
        );
    }

    #[test]
    fn stripping_the_retired_keys_leaves_everything_else() {
        let mut blob = serde_json::json!({
            "base_url": "https://models.example/v1",
            "insecure_tls": true,
            "pricing": {"image_per_unit": 0.04},
            "model_type": "chat",
            "limits": {"context_window": 8, "timeout_secs": 30, "retries": 1,
                       "output_tokens": 9, "rpm": 60},
            "headers": {"x-model-region": "eu"}
        });
        let dropped = strip_retired_advanced_keys(&mut blob);
        assert_eq!(
            dropped,
            [
                "base_url",
                "insecure_tls",
                "pricing",
                "limits.context_window"
            ]
        );
        assert_eq!(
            blob,
            serde_json::json!({
                "model_type": "chat",
                "limits": {"timeout_secs": 30, "retries": 1, "output_tokens": 9, "rpm": 60},
                "headers": {"x-model-region": "eu"}
            })
        );
        assert!(
            strip_retired_advanced_keys(&mut blob).is_empty(),
            "idempotent"
        );
        // a blob that is not an object is not this function's to judge
        let mut not_an_object = serde_json::json!([1]);
        assert!(strip_retired_advanced_keys(&mut not_an_object).is_empty());
    }

    #[test]
    fn a_route_retry_budget_replaces_the_default_up_to_the_ceiling() {
        let limits = |retries| ModelLimits {
            retries,
            ..Default::default()
        };
        assert_eq!(limits(None).max_retries(2), 2, "unset inherits");
        assert_eq!(limits(Some(0)).max_retries(2), 0, "zero turns retries off");
        assert_eq!(limits(Some(4)).max_retries(2), 4);
        assert_eq!(
            limits(Some(10_000_000)).max_retries(2),
            MAX_ROUTE_RETRIES,
            "a stored value above the ceiling is applied as the ceiling"
        );
    }

    fn route_with_headers(headers: &[(&str, &str)], locked: &[&str]) -> ModelRoute {
        let mut route = route_with(&[], ParamPolicy::default());
        route.advanced.headers = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        route.advanced.locked_headers = locked.iter().map(|n| n.to_string()).collect();
        route
    }

    #[test]
    fn route_headers_that_cannot_be_sent_are_named() {
        let route = route_with_headers(
            &[
                ("x-model-region", "eu"),
                ("Authorization", "Bearer sk-other"),
                ("X-Api-Key", "k"),
                ("bad name", "v"),
                ("x-newline", "a\nb"),
            ],
            &["x-model-region", "no good"],
        );
        let problems = route.advanced.header_problems().join("\n");
        assert!(
            problems.contains("'Authorization' cannot be set"),
            "{problems}"
        );
        assert!(problems.contains("'X-Api-Key' cannot be set"), "{problems}");
        assert!(
            problems.contains("header name 'bad name' is not valid"),
            "{problems}"
        );
        assert!(problems.contains("'x-newline' has a value"), "{problems}");
        assert!(
            problems.contains("locked header name 'no good'"),
            "{problems}"
        );
        assert!(!problems.contains("x-model-region"), "{problems}");
    }

    #[test]
    fn a_file_with_a_credential_header_on_a_route_fails_validation() {
        let mut cfg = GatewayConfig::default();
        cfg.providers.push(ProviderConfig {
            name: "p".to_string(),
            api_base: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        let mut route = route_with_headers(&[("authorization", "Bearer x")], &[]);
        route.model = "m".to_string();
        route.targets = vec![Target {
            provider: "p".to_string(),
            ..Default::default()
        }];
        cfg.routes.push(route);
        let problems = cfg.validate().unwrap_err().join("\n");
        assert!(
            problems.contains("route 'm': header 'authorization' cannot be set"),
            "{problems}"
        );
    }

    /// #3097: a provider row that names a deployment secret as its credential
    /// variable is served without it, with a line naming the row; the gateway
    /// never reads the variable.
    #[test]
    fn the_snapshot_drops_a_secret_credential_variable_and_keeps_the_provider() {
        let mut cfg = GatewayConfig::default();
        cfg.providers.push(ProviderConfig {
            name: "leaky".to_string(),
            api_base: "http://127.0.0.1:1".to_string(),
            api_key_env: Some("ROLTER_KEK".to_string()),
            api_keys: vec![ApiKeyConfig {
                env: Some("DATABASE_URL".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        });
        cfg.providers.push(ProviderConfig {
            name: "fine".to_string(),
            api_base: "http://127.0.0.1:2".to_string(),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            ..Default::default()
        });
        let report = cfg.sanitize_for_snapshot_report();
        let lines = report.problems.join("\n");
        assert!(
            lines.contains("provider 'leaky' api_key_env 'ROLTER_KEK'"),
            "{lines}"
        );
        assert!(
            lines.contains("provider 'leaky' api_keys env 'DATABASE_URL'"),
            "{lines}"
        );
        assert!(!lines.contains("fine"), "{lines}");
        assert!(report.omitted.is_empty(), "{lines}");
        assert_eq!(cfg.providers.len(), 2);
        assert_eq!(cfg.providers[0].api_key_env, None);
        assert_eq!(cfg.providers[0].api_keys[0].env, None);
        assert_eq!(
            cfg.providers[1].api_key_env.as_deref(),
            Some("OPENAI_API_KEY")
        );
    }

    /// Defence in depth for a config that never went through the snapshot
    /// pass: resolving the key refuses the name too, and a file config is
    /// rejected outright.
    #[test]
    fn a_secret_credential_variable_is_never_read() {
        // SAFETY: test-local env var, no concurrent access to this key elsewhere
        unsafe { std::env::set_var("ROLTER_TEST_3097_SECRET", "leaked") };
        let provider = ProviderConfig {
            name: "leaky".to_string(),
            api_base: "http://127.0.0.1:1".to_string(),
            api_key_env: Some("ROLTER_TEST_3097_SECRET".to_string()),
            ..Default::default()
        };
        assert_eq!(provider.resolve_api_key(), None);
        assert!(provider.resolve_api_keys().is_empty());
        let mut cfg = GatewayConfig::default();
        cfg.providers.push(provider);
        let problems = cfg.validate().unwrap_err().join("\n");
        assert!(problems.contains("names a deployment secret"), "{problems}");
        // SAFETY: cleanup of the test-local var set above
        unsafe { std::env::remove_var("ROLTER_TEST_3097_SECRET") };
    }

    /// One route's unsendable header must not 500 `/internal/snapshot` for
    /// every tenant: the header is dropped, the route keeps serving and the
    /// rest of its headers survive.
    #[test]
    fn the_snapshot_drops_a_route_header_it_cannot_send_and_keeps_the_route() {
        let mut cfg = GatewayConfig::default();
        cfg.providers.push(ProviderConfig {
            name: "p".to_string(),
            api_base: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        });
        let mut route = route_with_headers(
            &[("authorization", "Bearer x"), ("x-model-region", "eu")],
            &["x-model-region", "no good"],
        );
        route.model = "m".to_string();
        route.targets = vec![Target {
            provider: "p".to_string(),
            ..Default::default()
        }];
        cfg.routes.push(route);

        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(cfg.routes.len(), 1, "the route is kept");
        let advanced = &cfg.routes[0].advanced;
        assert_eq!(advanced.headers.len(), 1);
        assert_eq!(advanced.headers["x-model-region"], "eu");
        assert_eq!(advanced.locked_headers, ["x-model-region"]);
        let warnings = warnings.join("\n");
        assert!(warnings.contains("route 'm'"), "{warnings}");
        assert!(
            warnings.contains("header 'authorization' cannot be set"),
            "{warnings}"
        );
        cfg.validate_snapshot()
            .expect("what is left must validate, or the snapshot would 500");
    }

    #[test]
    fn every_listed_refused_request_id_header_is_refused() {
        let names = request_id_refused_headers();
        assert!(names.contains(&"x-goog-api-key") && names.contains(&"set-cookie"));
        assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
        for name in names {
            assert!(request_id_header_problem(name).is_some(), "{name}");
        }
    }

    #[test]
    fn a_request_id_header_must_be_a_header_that_carries_nothing_else() {
        for usable in ["x-request-id", "x-correlation-id", "X-Trace-Ref"] {
            assert_eq!(request_id_header_problem(usable), None, "{usable}");
        }
        for unusable in [
            "Authorization",
            "x-api-key",
            "x-goog-api-key",
            "content-length",
            "transfer-encoding",
            "host",
            "cookie",
            "Set-Cookie",
        ] {
            let problem = request_id_header_problem(unusable)
                .unwrap_or_else(|| panic!("{unusable} must be refused"));
            assert!(problem.contains("cannot carry the request id"), "{problem}");
        }
        for malformed in ["", "x request id", "x-id\n"] {
            let problem = request_id_header_problem(malformed)
                .unwrap_or_else(|| panic!("{malformed:?} must be refused"));
            assert!(problem.contains("not a valid header name"), "{problem}");
        }
    }

    /// The gateway echoes the id on the response and logs it, so a file that
    /// names a credential header would copy the caller's key into both.
    #[test]
    fn a_file_with_a_credential_request_id_header_fails_validation() {
        let mut cfg = GatewayConfig::default();
        cfg.client.request_id_header = "authorization".to_string();
        let problems = cfg.validate().unwrap_err().join("\n");
        assert!(
            problems.contains("client.request_id_header: 'authorization'"),
            "{problems}"
        );

        cfg.client.request_id_header = "x-correlation-id".to_string();
        cfg.validate().expect("an ordinary header name is accepted");
    }

    /// One unusable header name must not withhold the snapshot from every
    /// tenant: it falls back to the default and says so.
    #[test]
    fn the_snapshot_resets_an_unusable_request_id_header_to_the_default() {
        let mut cfg = GatewayConfig::default();
        cfg.client.request_id_header = "content-length".to_string();

        let warnings = cfg.sanitize_for_snapshot().join("\n");
        assert_eq!(cfg.client.request_id_header, DEFAULT_REQUEST_ID_HEADER);
        assert!(
            warnings.contains("client.request_id_header: 'content-length'"),
            "{warnings}"
        );
        cfg.validate_snapshot()
            .expect("the reset header must validate, or the snapshot would 500");

        let mut kept = GatewayConfig::default();
        kept.client.request_id_header = "x-correlation-id".to_string();
        assert!(kept.sanitize_for_snapshot().is_empty());
        assert_eq!(kept.client.request_id_header, "x-correlation-id");
    }

    /// A stored `advanced` blob written before #1665 still loads.
    ///
    /// `additional_fields` was persisted into every route's advanced JSON, so
    /// dropping it from the struct is only safe while the rows that carry it
    /// keep deserializing — and keep deserializing *whole*, with the key
    /// ignored rather than stopping the parse at it. That holds because none of
    /// these types deny unknown fields, which is the same promise
    /// [`crate::config_lint`] rests on; this pins it for the one key we removed
    /// on purpose.
    #[test]
    fn an_advanced_block_carrying_the_removed_additional_fields_key_still_loads() {
        let route: ModelRoute = serde_json::from_value(serde_json::json!({
            "model": "vision",
            "targets": [],
            "advanced": {
                "model_type": "image",
                "additional_fields": {"safety_identifier": "acme-prod"},
                "limits": {"output_tokens": 2048},
                "headers": {"x-model-region": "eu"}
            }
        }))
        .expect("an old stored blob must still deserialize");
        assert_eq!(route.advanced.model_type.as_deref(), Some("image"));
        assert_eq!(route.advanced.limits.output_tokens, Some(2048));
        assert_eq!(
            route
                .advanced
                .headers
                .get("x-model-region")
                .map(String::as_str),
            Some("eu"),
            "fields after the removed key must survive it"
        );

        // and it does not come back out: nothing rolter serializes emits it
        let written = serde_json::to_value(&route).unwrap();
        assert!(
            written["advanced"].get("additional_fields").is_none(),
            "the removed key must not be re-emitted: {written}"
        );
    }

    fn variant(name: &str, weight: u32) -> Variant {
        Variant {
            name: name.to_string(),
            weight,
            targets: vec![],
            params: HashMap::new(),
        }
    }

    fn provider_with_keys(api_keys: Vec<ApiKeyConfig>) -> ProviderConfig {
        ProviderConfig {
            name: "p".to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: "http://x".to_string(),
            api_key: Some("legacy".to_string()),
            api_keys,
            ..Default::default()
        }
    }

    #[test]
    fn the_env_name_comes_from_either_spelling() {
        let legacy = ProviderConfig {
            api_key_env: Some("LEGACY_KEY".to_string()),
            ..Default::default()
        };
        assert_eq!(legacy.api_key_env_name(), Some("LEGACY_KEY"));

        let plural = ProviderConfig {
            api_keys: vec![ApiKeyConfig {
                env: Some("PLURAL_KEY".to_string()),
                weight: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            plural.api_key_env_name(),
            Some("PLURAL_KEY"),
            "a plural-form provider looked credential-less to the seed (#1514)"
        );

        // api_keys outranks the legacy pair, matching resolve_api_key
        let both = ProviderConfig {
            api_key_env: Some("LEGACY_KEY".to_string()),
            api_keys: vec![ApiKeyConfig {
                env: Some("PLURAL_KEY".to_string()),
                weight: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(both.api_key_env_name(), Some("PLURAL_KEY"));

        // the first entry that actually names a variable wins, not the first entry
        let inline_first = ProviderConfig {
            api_keys: vec![
                ApiKeyConfig {
                    key: Some("sk-inline".to_string()),
                    weight: 1,
                    ..Default::default()
                },
                ApiKeyConfig {
                    env: Some("SECOND_KEY".to_string()),
                    weight: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(inline_first.api_key_env_name(), Some("SECOND_KEY"));
    }

    #[test]
    fn the_env_name_never_returns_a_secret() {
        // this value is written to a database column and rendered into an
        // exported document, so an inline key must never surface through it
        let inline = ProviderConfig {
            api_key: Some("sk-legacy-secret".to_string()),
            api_keys: vec![ApiKeyConfig {
                key: Some("sk-plural-secret".to_string()),
                weight: 3,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(inline.api_key_env_name(), None);
        assert_eq!(inline.inline_api_key(), Some("sk-plural-secret"));
    }

    #[test]
    fn the_inline_key_comes_from_either_spelling() {
        let legacy = ProviderConfig {
            api_key: Some("sk-legacy".to_string()),
            ..Default::default()
        };
        assert_eq!(legacy.inline_api_key(), Some("sk-legacy"));

        let env_only = ProviderConfig {
            api_keys: vec![ApiKeyConfig {
                env: Some("PLURAL_KEY".to_string()),
                weight: 1,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            env_only.inline_api_key(),
            None,
            "an env-only plural provider has no literal to seal"
        );
    }

    #[test]
    fn the_surplus_count_is_what_a_single_credential_store_drops() {
        assert_eq!(ProviderConfig::default().surplus_api_key_count(), 0);
        assert_eq!(
            provider_with_keys(vec![ApiKeyConfig {
                env: Some("A".to_string()),
                weight: 1,
                ..Default::default()
            }])
            .surplus_api_key_count(),
            0
        );
        assert_eq!(
            provider_with_keys(vec![
                ApiKeyConfig {
                    env: Some("A".to_string()),
                    weight: 1,
                    ..Default::default()
                },
                ApiKeyConfig {
                    env: Some("B".to_string()),
                    weight: 1,
                    ..Default::default()
                },
                ApiKeyConfig {
                    env: Some("C".to_string()),
                    weight: 1,
                    ..Default::default()
                },
            ])
            .surplus_api_key_count(),
            2
        );
    }

    #[test]
    fn a_defaulted_provider_names_no_vendor_and_reaches_nothing() {
        // the point of this Default is that a fixture can omit every field it
        // does not care about (#1150), so it has to be inert: nothing here may
        // send a request, pin a host, or burn tokens on a health sweep
        let p = ProviderConfig::default();
        assert!(p.name.is_empty());
        assert!(p.api_base.is_empty());
        assert_eq!(p.kind, ProviderKind::OpenaiCompatible);
        assert!(p.slug.is_none());
        assert!(p.api_key.is_none());
        assert!(p.api_key_env.is_none());
        assert!(p.api_keys.is_empty());
        assert!(p.egress_proxy.is_none());
        assert!(p.egress_proxies.is_empty());
        assert!(!p.also_track_via_llm_call);
        assert!(!p.allow_custom_api_base);
        assert!(p.status_page_url.is_none());
        assert!(p.model_role_profiles.is_empty());
    }

    #[test]
    fn a_provider_kind_is_still_required_when_deserializing() {
        // `ProviderKind: Default` must not let a config or a stored row omit
        // `kind` and silently become openai-compatible
        let err = toml::from_str::<ProviderConfig>(
            r#"
            name = "p"
            api_base = "https://example.test"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("kind"), "{err}");
    }

    #[test]
    fn single_key_provider_resolves_one_element_list() {
        // legacy configs behave exactly as before: one key, weight 1
        let p = provider_with_keys(Vec::new());
        assert_eq!(p.resolve_api_key().as_deref(), Some("legacy"));
        assert_eq!(p.resolve_api_keys(), vec![("legacy".to_string(), 1)]);
    }

    #[test]
    fn queue_defaults_are_safe_and_enabled() {
        let queue = QueueConfig::default();
        assert!(queue.enabled);
        assert_eq!(queue.capacity, 256);
        assert_eq!(queue.workers, 8);
        assert_eq!(queue.backpressure, BackpressurePolicy::Error);
    }

    #[test]
    fn validate_rejects_invalid_queue_settings() {
        let mut cfg = GatewayConfig::default();
        cfg.queue.capacity = 0;
        cfg.queue.workers = 0;
        cfg.queue.backpressure = BackpressurePolicy::Block;
        cfg.queue.block_timeout_ms = 0;
        let problems = cfg.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("queue.capacity")));
        assert!(problems.iter().any(|p| p.contains("queue.workers")));
        assert!(problems
            .iter()
            .any(|p| p.contains("queue.block_timeout_ms")));
    }

    #[test]
    fn validate_rejects_invalid_logging_sample_rate() {
        let mut cfg = GatewayConfig::default();
        cfg.logging.sample_rate = 1.5;
        let problems = cfg.validate().unwrap_err();
        assert!(problems
            .iter()
            .any(|p| p.contains("logging.sample_rate must be in [0, 1]")));
    }

    #[test]
    fn ollama_cloud_requires_only_an_environment_key_reference() {
        let mut cfg = GatewayConfig::default();
        let mut cloud = provider_with_keys(Vec::new());
        cloud.kind = ProviderKind::OllamaCloud;
        cloud.api_key = None;
        cfg.providers.push(cloud.clone());
        let problems = cfg.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("requires api_key_env")));
        cloud.api_key_env = Some("OLLAMA_API_KEY".to_string());
        cfg.providers[0] = cloud;
        assert!(cfg.validate().is_ok());
        cfg.providers[0].api_key = Some("inline-secret".to_string());
        let problems = cfg.validate().unwrap_err();
        assert!(problems
            .iter()
            .any(|p| p.contains("must source its key from api_key_env")));
    }

    #[test]
    fn multi_key_list_takes_precedence_and_keeps_weights() {
        let p = provider_with_keys(vec![
            ApiKeyConfig {
                key: Some("k1".to_string()),
                env: None,
                weight: 3,
            },
            ApiKeyConfig {
                key: Some("k2".to_string()),
                env: None,
                weight: 0,
            },
        ]);
        // the legacy pair is ignored once api_keys is non-empty
        assert_eq!(p.resolve_api_key().as_deref(), Some("k1"));
        assert_eq!(
            p.resolve_api_keys(),
            vec![("k1".to_string(), 3), ("k2".to_string(), 0)]
        );
    }

    #[test]
    fn unresolvable_key_entries_are_skipped() {
        let p = provider_with_keys(vec![
            ApiKeyConfig {
                key: None,
                env: Some("ROLTER_TEST_KEY_THAT_DOES_NOT_EXIST".to_string()),
                weight: 5,
            },
            ApiKeyConfig {
                key: Some("k2".to_string()),
                env: None,
                weight: 2,
            },
        ]);
        assert_eq!(p.resolve_api_key().as_deref(), Some("k2"));
        assert_eq!(p.resolve_api_keys(), vec![("k2".to_string(), 2)]);
    }

    #[test]
    fn pick_api_key_respects_weights() {
        let p = provider_with_keys(vec![
            ApiKeyConfig {
                key: Some("k1".to_string()),
                env: None,
                weight: 3,
            },
            ApiKeyConfig {
                key: Some("k2".to_string()),
                env: None,
                weight: 1,
            },
        ]);
        // weights 3:1 over [0,1): first 3/4 of the draw space maps to k1
        assert_eq!(p.pick_api_key(0.0).as_deref(), Some("k1"));
        assert_eq!(p.pick_api_key(0.74).as_deref(), Some("k1"));
        assert_eq!(p.pick_api_key(0.76).as_deref(), Some("k2"));
        // a single-key provider always yields its one key
        let single = provider_with_keys(Vec::new());
        assert_eq!(single.pick_api_key(0.99).as_deref(), Some("legacy"));
        // no resolvable key at all
        let none = ProviderConfig {
            api_key: None,
            ..provider_with_keys(Vec::new())
        };
        assert_eq!(none.pick_api_key(0.5), None);
    }

    #[test]
    fn indexed_pick_skips_blocked_keys_and_fails_open() {
        let p = provider_with_keys(vec![
            ApiKeyConfig {
                key: Some("k1".to_string()),
                env: None,
                weight: 3,
            },
            ApiKeyConfig {
                key: Some("k2".to_string()),
                env: None,
                weight: 1,
            },
        ]);
        // k1 blocked: every draw lands on k2
        let picked = p.pick_api_key_indexed(0.1, |i| i == 0).unwrap();
        assert_eq!(picked, (1, "k2".to_string()));
        // all keys blocked: fail open to the full weighted pool
        let picked = p.pick_api_key_indexed(0.1, |_| true).unwrap();
        assert_eq!(picked, (0, "k1".to_string()));
        // single-key providers ignore blocking entirely
        let single = provider_with_keys(Vec::new());
        assert_eq!(
            single.pick_api_key_indexed(0.9, |_| true).unwrap(),
            (0, "legacy".to_string())
        );
    }

    #[test]
    fn a_zero_weight_key_is_picked_only_when_no_other_key_can_be() {
        let keys = |weights: [u32; 2]| {
            provider_with_keys(
                weights
                    .iter()
                    .enumerate()
                    .map(|(i, &weight)| ApiKeyConfig {
                        key: Some(format!("k{i}")),
                        env: None,
                        weight,
                    })
                    .collect(),
            )
        };
        let draws = || (0..=100).map(|i| f64::from(i) / 100.0);
        // [0, 9]: key 0 never takes a first pick
        let skewed = keys([0, 9]);
        for r in draws() {
            assert_eq!(skewed.pick_api_key_indexed(r, |_| false).unwrap().0, 1);
        }
        // [0, 0]: uniform
        let flat = keys([0, 0]);
        assert_eq!(flat.pick_api_key_indexed(0.1, |_| false).unwrap().0, 0);
        assert_eq!(flat.pick_api_key_indexed(0.9, |_| false).unwrap().0, 1);
        // the 9-weight key parked on a cooldown: the zero-weight key is used
        for r in draws() {
            assert_eq!(skewed.pick_api_key_indexed(r, |i| i == 1).unwrap().0, 0);
        }
    }

    #[test]
    fn api_keys_parse_from_toml_with_default_weight() {
        let cfg: ProviderConfig = toml::from_str(
            r#"
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com"
            [[api_keys]]
            key = "a"
            [[api_keys]]
            key = "b"
            weight = 4
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.resolve_api_keys(),
            vec![("a".to_string(), 1), ("b".to_string(), 4)]
        );
    }

    #[test]
    fn weighted_index_respects_proportions() {
        // weights 3:1 over [0,1): the first 3/4 map to index 0, the last 1/4 to 1
        let w = || [3u32, 1].into_iter();
        assert_eq!(weighted_index(w(), 0.0), Some(0));
        assert_eq!(weighted_index(w(), 0.74), Some(0));
        assert_eq!(weighted_index(w(), 0.75), Some(1));
        assert_eq!(weighted_index(w(), 0.99), Some(1));
    }

    #[test]
    fn weighted_index_clamps_out_of_range_and_empty() {
        assert_eq!(weighted_index([1u32, 1].into_iter(), 1.5), Some(1));
        assert_eq!(weighted_index(std::iter::empty::<u32>(), 0.5), None);
        // an all-zero set falls back to uniform
        assert_eq!(weighted_index([0u32, 0].into_iter(), 0.0), Some(0));
        assert_eq!(weighted_index([0u32, 0].into_iter(), 0.5), Some(1));
    }

    #[test]
    fn a_zero_weight_entry_takes_no_first_attempts_while_another_is_positive() {
        let w = || [0u32, 9, 0].into_iter();
        for i in 0..=100 {
            assert_eq!(weighted_index(w(), f64::from(i) / 100.0), Some(1));
        }
        // 9:0 never samples the zero-weight variant, which still falls back
        let mut route = route_with(&[], ParamPolicy::default());
        route.variants = vec![variant("a", 9), variant("b", 0)];
        for i in 0..100 {
            assert_eq!(route.sample_variant(f64::from(i) / 100.0), Some(0));
        }
        assert_eq!(route.fallback_order(0), vec![0, 1]);
    }

    #[test]
    fn cache_config_defaults_are_off_with_sane_ttl() {
        let c = CacheConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.default_ttl_secs, 60);
        assert_eq!(c.namespace, "rolter:cache");
        assert_eq!(c.max_entry_bytes, 1024 * 1024);
    }

    #[test]
    fn route_cache_helpers_gate_and_resolve_ttl() {
        let mut route = route_with(&[], ParamPolicy::default());
        // no cache block => caching off, ttl falls back to the global default
        assert!(!route.cache_enabled());
        assert!(!route.cache_per_key());
        assert_eq!(route.cache_ttl_secs(300), 300);

        // an explicitly-disabled block still reads as off
        route.cache = Some(RouteCache {
            enabled: false,
            ttl_secs: Some(10),
            per_key: true,
            semantic: None,
        });
        assert!(!route.cache_enabled());
        // ttl override applies regardless of the enabled flag
        assert_eq!(route.cache_ttl_secs(300), 10);

        // enabled with per-key isolation and no ttl override
        route.cache = Some(RouteCache {
            enabled: true,
            ttl_secs: None,
            per_key: true,
            semantic: None,
        });
        assert!(route.cache_enabled());
        assert!(route.cache_per_key());
        assert_eq!(route.cache_ttl_secs(300), 300);
    }

    #[test]
    fn route_cache_parses_from_toml() {
        let route: ModelRoute = toml::from_str(
            r#"
            model = "gpt-4o"
            [cache]
            enabled = true
            ttl_secs = 600
            per_key = true
            "#,
        )
        .unwrap();
        assert!(route.cache_enabled());
        assert!(route.cache_per_key());
        assert_eq!(route.cache_ttl_secs(60), 600);
        // an absent [cache] table leaves caching off and enabled defaults true
        let bare: ModelRoute = toml::from_str(r#"model = "gpt-4o""#).unwrap();
        assert!(bare.cache.is_none());
        assert!(!bare.cache_enabled());
    }

    #[test]
    fn virtual_key_cache_override_parses_tri_state() {
        // absent => None (inherit the route decision)
        let inherit: VirtualKeyConfig = toml::from_str(r#"key = "sk-a""#).unwrap();
        assert_eq!(inherit.cache, None);
        // explicit opt-out and opt-in survive as Some(false)/Some(true)
        let off: VirtualKeyConfig = toml::from_str(
            r#"
            key = "sk-b"
            cache = false
            "#,
        )
        .unwrap();
        assert_eq!(off.cache, Some(false));
        let on: VirtualKeyConfig = toml::from_str(
            r#"
            key = "sk-c"
            cache = true
            "#,
        )
        .unwrap();
        assert_eq!(on.cache, Some(true));
    }

    #[test]
    fn virtual_key_provider_allow_list_parses_and_defaults_to_all() {
        let unrestricted: VirtualKeyConfig = toml::from_str(r#"key = "sk-a""#).unwrap();
        assert!(unrestricted.providers.is_empty());
        let restricted: VirtualKeyConfig = toml::from_str(
            r#"
            key = "sk-b"
            providers = ["openai", "anthropic"]
            "#,
        )
        .unwrap();
        assert_eq!(restricted.providers, ["openai", "anthropic"]);
    }

    #[test]
    fn sample_variant_and_fallback_order() {
        let mut route = route_with(&[], ParamPolicy::default());
        route.variants = vec![variant("a", 3), variant("b", 1), variant("c", 1)];
        assert!(route.has_variants());
        assert_eq!(route.sample_variant(0.0), Some(0));
        assert_eq!(route.sample_variant(0.9), Some(2));
        // fallback chain: primary first, then the rest in declared order
        assert_eq!(route.fallback_order(1), vec![1, 0, 2]);
        assert_eq!(route.fallback_order(0), vec![0, 1, 2]);
    }

    #[test]
    fn apply_variant_params_layers_variant_over_route() {
        // route default temperature=0; variant overrides it to 0.7 and adds top_p
        let mut route = route_with(
            &[("temperature", serde_json::json!(0.0))],
            ParamPolicy::default(),
        );
        let mut v = variant("canary", 1);
        v.params
            .insert("temperature".to_string(), serde_json::json!(0.7));
        v.params.insert("top_p".to_string(), serde_json::json!(0.9));

        // caller sends nothing: merged admin defaults are injected
        let mut body = serde_json::json!({"messages": []});
        route.apply_variant_params(&v, &mut body);
        assert_eq!(body["temperature"], serde_json::json!(0.7));
        assert_eq!(body["top_p"], serde_json::json!(0.9));

        // default allow policy lets the caller override the merged temperature
        let mut body = serde_json::json!({"temperature": 0.2, "messages": []});
        route.apply_variant_params(&v, &mut body);
        assert_eq!(body["temperature"], serde_json::json!(0.2));
        assert_eq!(body["top_p"], serde_json::json!(0.9));

        // deny policy pins it: the variant default wins over the caller
        route.param_policy = ParamPolicy {
            mode: OverrideMode::Deny,
            allow: vec![],
            deny: vec![],
        };
        let mut body = serde_json::json!({"temperature": 0.2, "messages": []});
        route.apply_variant_params(&v, &mut body);
        assert_eq!(body["temperature"], serde_json::json!(0.7));
    }

    #[test]
    fn variants_round_trip_through_toml() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[routes]]
            model = "chat"
            [[routes.variants]]
            name = "control"
            weight = 9
            [[routes.variants.targets]]
            provider = "openai"
            model = "gpt-4o"
            [[routes.variants]]
            name = "canary"
            weight = 1
            [[routes.variants.targets]]
            provider = "anthropic"
            model = "claude-sonnet-4-20250514"
            "#,
        )
        .unwrap();
        let route = &cfg.routes[0];
        assert_eq!(route.variants.len(), 2);
        assert_eq!(route.variants[0].name, "control");
        assert_eq!(route.variants[0].weight, 9);
        assert_eq!(route.variants[0].targets[0].provider, "openai");
        assert_eq!(route.variants[1].name, "canary");
    }

    #[test]
    fn injects_default_when_caller_omits() {
        let route = route_with(
            &[("temperature", serde_json::json!(0.0))],
            ParamPolicy::default(),
        );
        let mut body = serde_json::json!({"model": "m", "messages": []});
        route.apply_params(&mut body);
        assert_eq!(body["temperature"], serde_json::json!(0.0));
    }

    #[test]
    fn allow_mode_keeps_caller_value() {
        // default mode is allow: caller override survives
        let route = route_with(
            &[("temperature", serde_json::json!(0.0))],
            ParamPolicy::default(),
        );
        let mut body = serde_json::json!({"temperature": 0.9});
        route.apply_params(&mut body);
        assert_eq!(body["temperature"], serde_json::json!(0.9));
    }

    #[test]
    fn allow_mode_deny_exception_forces_default() {
        let policy = ParamPolicy {
            mode: OverrideMode::Allow,
            allow: vec![],
            deny: vec!["temperature".to_string()],
        };
        let route = route_with(&[("temperature", serde_json::json!(0.0))], policy);
        let mut body = serde_json::json!({"temperature": 0.9});
        route.apply_params(&mut body);
        // override denied for this param -> admin default silently wins
        assert_eq!(body["temperature"], serde_json::json!(0.0));
    }

    #[test]
    fn deny_mode_forces_default_but_allow_exception_passes() {
        let policy = ParamPolicy {
            mode: OverrideMode::Deny,
            allow: vec!["max_tokens".to_string()],
            deny: vec![],
        };
        let route = route_with(
            &[
                ("temperature", serde_json::json!(0.0)),
                ("max_tokens", serde_json::json!(256)),
            ],
            policy,
        );
        let mut body = serde_json::json!({"temperature": 0.9, "max_tokens": 999});
        route.apply_params(&mut body);
        // temperature override denied -> default wins; max_tokens allowed -> kept
        assert_eq!(body["temperature"], serde_json::json!(0.0));
        assert_eq!(body["max_tokens"], serde_json::json!(999));
    }

    #[test]
    fn no_params_leaves_body_untouched() {
        let route = route_with(&[], ParamPolicy::default());
        let mut body = serde_json::json!({"temperature": 0.9});
        route.apply_params(&mut body);
        assert_eq!(body["temperature"], serde_json::json!(0.9));
    }

    #[test]
    fn params_round_trip_through_toml() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[routes]]
            model = "gpt-4o"
            [routes.params]
            temperature = 0.0
            max_tokens = 512
            [routes.param_policy]
            mode = "deny"
            allow = ["max_tokens"]
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        let route = &cfg.routes[0];
        assert_eq!(route.params["temperature"], serde_json::json!(0.0));
        assert_eq!(route.param_policy.mode, OverrideMode::Deny);
        assert!(route.param_policy.may_override("max_tokens"));
        assert!(!route.param_policy.may_override("temperature"));
    }

    #[test]
    fn parses_minimal_config() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com"

            [[routes]]
            model = "gpt-4o"
            strategy = "round_robin"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.server.port, 4000);
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.routes[0].strategy, BalancingStrategy::RoundRobin);
        assert_eq!(cfg.routes[0].targets[0].weight, 1);
        assert!(cfg.validate().is_ok());
    }

    // #2357: the key left the security policy, but a file that set it to close
    // a keyless gateway must not come back open after an upgrade
    #[test]
    fn a_retired_virtual_key_required_still_closes_a_file_gateway() {
        let cfg =
            GatewayConfig::from_toml_str("[security]\nvirtual_key_required = true\n").unwrap();
        assert_eq!(cfg.server.require_auth, Some(true));

        // an explicit require_auth always won over the key, and still does
        let cfg = GatewayConfig::from_toml_str(
            "[server]\nrequire_auth = false\n[security]\nvirtual_key_required = true\n",
        )
        .unwrap();
        assert_eq!(cfg.server.require_auth, Some(false));

        // `false` was the default, so it leaves the deployment default alone
        let cfg =
            GatewayConfig::from_toml_str("[security]\nvirtual_key_required = false\n").unwrap();
        assert_eq!(cfg.server.require_auth, None);
    }

    #[test]
    fn the_retired_virtual_key_required_is_not_linted_as_ignored() {
        let src =
            "[security]\nvirtual_key_required = true\nauth_bypass_routes = [\"/v1/models\"]\n";
        assert!(crate::config_lint::unknown_keys(src).unwrap().is_empty());
    }

    #[test]
    fn parses_keyless_ollama_provider_and_rejects_v1_suffix() {
        let raw = r#"
            [[providers]]
            name = "ollama"
            kind = "ollama"
            api_base = "http://localhost:11434"

            [[routes]]
            model = "local-model"
            [[routes.targets]]
            provider = "ollama"
        "#;
        let mut cfg = GatewayConfig::from_toml_str(raw).unwrap();
        assert_eq!(cfg.providers[0].kind, ProviderKind::Ollama);
        assert!(cfg.providers[0].resolve_api_key().is_none());
        assert!(cfg.validate().is_ok());

        cfg.providers[0].api_base = "http://localhost:11434/v1".to_string();
        assert!(cfg.validate().unwrap_err()[0].contains("without /v1"));
    }

    #[test]
    fn validates_openrouter_endpoint_and_environment_key_reference() {
        let raw = r#"
            [[providers]]
            name = "openrouter"
            kind = "openrouter"
            api_base = "https://openrouter.ai/api/v1"
            api_key_env = "OPENROUTER_API_KEY"

            [[routes]]
            model = "router-chat"
            [[routes.targets]]
            provider = "openrouter"
            model = "anthropic/claude-sonnet-4"
        "#;
        let mut cfg = GatewayConfig::from_toml_str(raw).unwrap();
        assert!(cfg.validate().is_ok());

        cfg.providers[0].api_base = "https://openrouter.ai/api".to_string();
        assert!(cfg
            .validate()
            .unwrap_err()
            .iter()
            .any(|problem| problem.contains("api_base must be")));
    }

    #[test]
    fn openrouter_may_point_elsewhere_only_with_an_explicit_opt_out() {
        let raw = r#"
            [[providers]]
            name = "openrouter-edge"
            kind = "openrouter"
            api_base = "http://127.0.0.1:18002"
            api_key_env = "OPENROUTER_API_KEY"
            allow_custom_api_base = true

            [[routes]]
            model = "router-chat"
            [[routes.targets]]
            provider = "openrouter-edge"
        "#;
        let mut cfg = GatewayConfig::from_toml_str(raw).unwrap();
        assert!(cfg.providers[0].allow_custom_api_base);
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());

        // the opt-out is reported, so a reviewer sees the redirected key
        let advisories = cfg.custom_api_base_advisories();
        assert_eq!(advisories.len(), 1, "{advisories:?}");
        assert!(advisories[0].contains("openrouter-edge"), "{advisories:?}");
        assert!(advisories[0].contains("127.0.0.1:18002"), "{advisories:?}");

        // it relaxes the host pin and nothing else: the key must still come
        // from the environment, which is what stops the exfiltration path
        cfg.providers[0].api_key = Some("sk-inline".to_string());
        let problems = cfg.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("must source its key from api_key_env")),
            "{problems:?}"
        );

        // and without the opt-out the same api_base is still refused
        cfg.providers[0].api_key = None;
        cfg.providers[0].allow_custom_api_base = false;
        let problems = cfg.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("allow_custom_api_base")),
            "the error has to name the way out: {problems:?}"
        );
    }

    #[test]
    fn a_pinned_provider_raises_no_advisory() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openrouter"
            kind = "openrouter"
            api_base = "https://openrouter.ai/api/v1"
            api_key_env = "OPENROUTER_API_KEY"

            [[routes]]
            model = "router-chat"
            [[routes.targets]]
            provider = "openrouter"
        "#,
        )
        .unwrap();
        assert!(cfg.validate().is_ok());
        assert!(cfg.custom_api_base_advisories().is_empty());
    }

    #[test]
    fn validate_reports_all_problems() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com"

            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://dup.example.com"

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "missing"
            [[routes.targets]]
            provider = "openai"
            weight = 0

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        let problems = cfg.validate().unwrap_err();
        assert_eq!(problems.len(), 4);
        assert!(problems.iter().any(|p| p.contains("duplicate provider")));
        assert!(problems.iter().any(|p| p.contains("duplicate route")));
        assert!(problems.iter().any(|p| p.contains("unknown provider")));
        assert!(problems.iter().any(|p| p.contains("zero weight")));
    }

    #[test]
    fn validate_flags_urls_targets_keys_and_limits() {
        let mut cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "ftp://nope"

            [[providers]]
            name = "proxied"
            kind = "openai_compatible"
            api_base = "http://localhost:8000"
            egress_proxy = "not-a-url"

            [[routes]]
            model = "r1"
            strategy = "round_robin"
            [[routes.targets]]
            provider = "openai"

            [[virtual_keys]]
            key = "dup"
            [[virtual_keys]]
            key = "dup"

            [[budgets]]
            scope = "org"
            id = "o1"
            limit_usd = 0.0

            [[rate_limits]]
            scope = "key"
            id = "k1"
            "#,
        )
        .unwrap();
        // targets is a required toml field; clear it post-parse to exercise the
        // "no targets" check
        cfg.routes[0].targets.clear();
        let problems = cfg.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("invalid api_base")));
        assert!(problems.iter().any(|p| p.contains("invalid egress_proxy")));
        assert!(problems
            .iter()
            .any(|p| p.contains("has neither targets nor variants")));
        assert!(problems.iter().any(|p| p.contains("duplicate virtual key")));
        assert!(problems
            .iter()
            .any(|p| p.contains("non-positive limit_usd")));
        assert!(problems.iter().any(|p| p.contains("neither rpm nor tpm")));
    }

    #[test]
    fn metrics_path_defaults_and_validates() {
        // default is /metrics and passes validation
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.server.metrics_path, "/metrics");

        // unrooted path is rejected
        let mut bad = GatewayConfig::default();
        bad.server.metrics_path = "metrics".to_string();
        let problems = bad.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("must start with '/'")));

        // colliding with a built-in route is rejected
        let mut collide = GatewayConfig::default();
        collide.server.metrics_path = "/v1/models".to_string();
        let problems = collide.validate().unwrap_err();
        assert!(problems
            .iter()
            .any(|p| p.contains("collides with a built-in route")));

        // a custom rooted path is accepted
        let mut ok = GatewayConfig::default();
        ok.server.metrics_path = "/internal/metrics".to_string();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn url_helpers_accept_and_reject() {
        assert!(is_http_url("https://api.openai.com"));
        assert!(is_http_url("http://localhost:8001"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url("ftp://x"));
        assert!(is_proxy_url("socks5h://proxy:1080"));
        assert!(!is_proxy_url("proxy:1080"));
    }

    #[test]
    fn proxy_references_require_env_for_credentials() {
        assert!(is_proxy_reference("${PROVIDER_PROXY_EU}"));
        assert!(is_proxy_reference("socks5h://proxy.internal:1080"));
        assert!(!is_proxy_reference("${lowercase}"));
        assert!(!is_proxy_reference("http://user:secret@proxy:3128"));
    }

    #[test]
    fn legacy_proxy_is_a_one_element_pool() {
        let mut provider = provider_with_keys(Vec::new());
        provider.egress_proxy = Some("http://legacy:3128".to_string());
        assert_eq!(provider.egress_proxy_pool(), vec!["http://legacy:3128"]);
        provider.egress_proxies = vec![
            "http://first:3128".to_string(),
            "socks5h://second:1080".to_string(),
        ];
        assert_eq!(
            provider.egress_proxy_pool(),
            vec!["http://first:3128", "socks5h://second:1080"]
        );
    }

    #[test]
    fn cost_from_token_counts() {
        let price = ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: d("2.5"),
            output_per_mtok: d("10.0"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        };
        // 1000 * 2.5/1e6 + 500 * 10/1e6 = 0.0025 + 0.005 = 0.0075, and now
        // exactly that rather than within an epsilon of it (#967)
        assert_eq!(price.cost(1000, 500, 0, 0, 0), d("0.0075"));
    }

    /// The failure #967 is actually about: at `f64`, a price whose decimal
    /// expansion is not a binary fraction accumulates error, and the classic
    /// demonstration is that a tenth plus two tenths is not three tenths.
    /// Decimal makes the same sum exact.
    #[test]
    fn accumulating_cost_is_exact_where_f64_drifts() {
        let price = ModelPriceConfig {
            model: "drifty".to_string(),
            // 0.1 and 0.2 are both unrepresentable in binary floating point
            input_per_mtok: d("0.1"),
            output_per_mtok: d("0.2"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        };
        // one million of each: 0.1 + 0.2, which in f64 is 0.30000000000000004
        assert_eq!(price.cost(1_000_000, 1_000_000, 0, 0, 0), d("0.300000"));
        assert_ne!(0.1_f64 + 0.2_f64, 0.3_f64, "the f64 this replaces");

        // and a thousand small requests sum to exactly what one big one costs,
        // which is the property a budget counter is comparing against
        let one_big = price.cost(1_000_000, 0, 0, 0, 0);
        let summed: rust_decimal::Decimal = (0..1_000).map(|_| price.cost(1_000, 0, 0, 0, 0)).sum();
        assert_eq!(summed, one_big);
    }

    /// A settlement unit with 18 decimal places — an ETH-style wei — survives
    /// the price type without loss. This is the representable-range failure the
    /// issue calls disqualifying for `f64`, which carries ~15-16 significant
    /// digits and cannot hold one wei beside a whole ether.
    #[test]
    fn an_eighteen_decimal_unit_round_trips_without_loss() {
        let one_wei = d("0.000000000000000001");
        let price = ModelPriceConfig {
            model: "crypto-priced".to_string(),
            input_per_mtok: one_wei,
            output_per_mtok: d("0"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "ETH".to_string(),
        };
        assert_eq!(price.input_per_mtok, one_wei);
        assert_eq!(price.input_per_mtok.scale(), 18);

        // a whole unit and one wei are distinguishable side by side, which is
        // exactly what f64 cannot do
        let one_eth = d("1");
        assert_ne!(one_eth + one_wei, one_eth);
        assert_eq!((one_eth + one_wei) - one_eth, one_wei);

        // 1e6 tokens at one wei per Mtok is one wei, not zero
        assert_eq!(price.cost(1_000_000, 0, 0, 0, 0), one_wei);
    }

    /// The snapshot is exchanged between a control plane and a gateway that may
    /// be different builds, and it carries a `config_version` change counter
    /// rather than a schema version — so there is no negotiation and no way for
    /// an older reader to say it wants numbers. Prices and limits therefore stay
    /// JSON *numbers*, which is what `rust_decimal`'s default string form would
    /// have quietly changed (#967).
    #[test]
    fn prices_and_limits_stay_json_numbers_on_the_wire() {
        let price = ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: d("2.5"),
            output_per_mtok: d("10"),
            cached_input_per_mtok: Some(d("1.25")),
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        };
        let json = serde_json::to_value(&price).expect("serializes");
        assert!(json["input_per_mtok"].is_number(), "{json}");
        assert!(json["output_per_mtok"].is_number(), "{json}");
        assert!(json["cached_input_per_mtok"].is_number(), "{json}");

        // and a number is still what comes back in
        let parsed: ModelPriceConfig = serde_json::from_value(json).expect("round trips");
        assert_eq!(parsed.input_per_mtok, d("2.5"));
        assert_eq!(parsed.cached_input_per_mtok, Some(d("1.25")));

        let budget = BudgetConfig {
            scope: BudgetScope::Org,
            id: "org-1".to_string(),
            limit_usd: d("100.5"),
            period: BudgetPeriod::default(),
            unpriced_policy: None,
            within: None,
        };
        let json = serde_json::to_value(&budget).expect("serializes");
        assert!(json["limit_usd"].is_number(), "{json}");

        // the currency rate table too, which the dashboard reads
        let currency = crate::CurrencyConfig {
            base: "USD".to_string(),
            rates: HashMap::from([("EUR".to_string(), d("1.10"))]),
        };
        let json = serde_json::to_value(&currency).expect("serializes");
        assert!(json["rates"]["EUR"].is_number(), "{json}");
        let parsed: crate::CurrencyConfig = serde_json::from_value(json).expect("round trips");
        assert_eq!(parsed.rates.get("EUR"), Some(&d("1.10")));
    }

    /// #1902: a stored period is read strictly. `7d` used to fall through to
    /// monthly without a word, so it has to come back as unrecognised rather
    /// than as any window at all.
    #[test]
    fn a_budget_period_is_read_strictly() {
        for (spelling, period) in [
            ("daily", BudgetPeriod::Daily),
            ("1d", BudgetPeriod::Daily),
            (" 24H ", BudgetPeriod::Daily),
            ("Monthly", BudgetPeriod::Monthly),
            ("30d", BudgetPeriod::Monthly),
            ("total", BudgetPeriod::Total),
            ("LIFETIME", BudgetPeriod::Total),
            ("all", BudgetPeriod::Total),
        ] {
            assert_eq!(BudgetPeriod::parse(spelling), Some(period), "{spelling}");
        }
        for unknown in ["7d", "weekly", "dialy", "", "  ", "30 d", "month"] {
            assert_eq!(BudgetPeriod::parse(unknown), None, "{unknown:?}");
        }
        // every named form is the one serde writes for that window, so a
        // period read from the database and one read from rolter.toml agree
        for period in [
            BudgetPeriod::Daily,
            BudgetPeriod::Monthly,
            BudgetPeriod::Total,
        ] {
            let named = serde_json::to_value(period).expect("serializes");
            let named = named.as_str().expect("a string");
            assert_eq!(BudgetPeriod::parse(named), Some(period), "{named}");
        }
    }

    /// A budget limit is compared, not reported, so the boundary has to be a
    /// state a test can name. `spend == limit` is over the cap, and the value
    /// just below it is not — neither of which is expressible when both sides
    /// are binary floats carrying rounding from wherever they were summed.
    #[test]
    fn a_budget_limit_has_an_exact_boundary() {
        let limit = d("10.0000");
        assert!(limit >= limit, "spend exactly at the limit is over it");
        assert!(d("9.9999") < limit);
        // the value one ten-thousandth under the cap — the smallest step the
        // numeric(12,4) column can express — is strictly under it
        assert_ne!(d("9.9999"), limit);
    }

    #[test]
    fn cost_applies_cached_rate() {
        let price = ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: d("2.0"),
            output_per_mtok: d("0.0"),
            cached_input_per_mtok: Some(d("0.5")),
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        };
        // 600 fresh * 2 + 400 cached * 0.5 = 1200 + 200 = 1400 / 1e6
        assert_eq!(price.cost(1000, 0, 400, 0, 0), d("0.0014"));
    }

    fn write_priced() -> ModelPriceConfig {
        ModelPriceConfig {
            model: "claude".to_string(),
            input_per_mtok: d("3.0"),
            output_per_mtok: d("15.0"),
            cached_input_per_mtok: Some(d("0.3")),
            cache_write_per_mtok: Some(d("3.75")),
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        }
    }

    #[test]
    fn cost_applies_the_cache_write_rate() {
        // 600 fresh * 3 + 400 written * 3.75 = 1800 + 1500 = 3300 / 1e6
        assert_eq!(write_priced().cost(1000, 0, 0, 400, 0), d("0.0033"));
    }

    /// A row without a write rate prices a write as the input it is, which is
    /// what every row did before the column existed, so no stored price changes
    /// what it charges.
    #[test]
    fn a_missing_write_rate_falls_back_to_the_input_rate() {
        let mut price = write_priced();
        price.cache_write_per_mtok = None;
        assert_eq!(price.cost(1000, 0, 0, 400, 0), price.cost(1000, 0, 0, 0, 0));
        assert_eq!(price.cost(1000, 0, 0, 400, 0), d("0.003"));
    }

    /// The whole prompt is `fresh + cached + written` (#2863), so each token is
    /// priced once, at its own rate.
    #[test]
    fn reads_writes_and_fresh_input_are_each_priced_once() {
        // 10 fresh * 3 + 80 cached * 0.3 + 30 written * 3.75 + 50 out * 15
        // = 30 + 24 + 112.5 + 750 = 916.5 / 1e6
        assert_eq!(write_priced().cost(120, 50, 80, 30, 0), d("0.0009165"));
    }

    /// A provider that reports more cache tokens than the prompt holds must not
    /// be charged for tokens that are not there: reads are taken first, then
    /// writes out of the remainder.
    #[test]
    fn cache_counts_beyond_the_prompt_are_not_charged_twice() {
        // 100-token prompt: 80 cached, then only the 20 left can be writes
        let capped = write_priced().cost(100, 0, 80, 80, 0);
        // 80 * 0.3 + 20 * 3.75 = 24 + 75 = 99 / 1e6
        assert_eq!(capped, d("0.000099"));
        // and nothing at all beyond the prompt when the writes alone overshoot
        assert_eq!(
            write_priced().cost(100, 0, 0, 1_000, 0),
            write_priced().cost(100, 0, 0, 100, 0)
        );
    }

    #[test]
    #[allow(deprecated)]
    fn the_former_name_prices_no_cache_write() {
        let price = write_priced();
        assert_eq!(price.cost_usd(1000, 0, 400), price.cost(1000, 0, 400, 0, 0));
    }

    /// The field crosses the snapshot as a JSON number like its neighbours, and
    /// a snapshot from a control plane that predates it still parses (the field
    /// is absent, so it reads as no write rate).
    #[test]
    fn the_write_rate_is_a_json_number_and_optional_on_the_wire() {
        let json = serde_json::to_value(write_priced()).expect("serializes");
        assert!(json["cache_write_per_mtok"].is_number(), "{json}");
        let parsed: ModelPriceConfig = serde_json::from_value(json).expect("round trips");
        assert_eq!(parsed.cache_write_per_mtok, Some(d("3.75")));

        let older: ModelPriceConfig = serde_json::from_value(serde_json::json!({
            "model": "claude",
            "input_per_mtok": 3.0,
            "output_per_mtok": 15.0,
            "cached_input_per_mtok": 0.3,
            "currency": "USD",
        }))
        .expect("an older snapshot parses");
        assert_eq!(older.cache_write_per_mtok, None);
        assert_eq!(older.cache_write_1h_per_mtok, None);
    }

    fn one_hour_priced() -> ModelPriceConfig {
        ModelPriceConfig {
            cache_write_1h_per_mtok: Some(d("6.0")),
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            ..write_priced()
        }
    }

    /// #2891: the two caches are priced at their own rates, out of the writes
    /// and not on top of them.
    #[test]
    fn each_cache_write_share_is_priced_at_its_own_rate() {
        // 600 fresh * 3 + 100 5 minute * 3.75 + 300 1 hour * 6
        // = 1800 + 375 + 1800 = 3975 / 1e6
        assert_eq!(one_hour_priced().cost(1000, 0, 0, 400, 300), d("0.003975"));
        // the whole write in the 1 hour cache
        assert_eq!(
            one_hour_priced().cost(1000, 0, 0, 400, 400),
            d("0.004200"),
            "600 * 3 + 400 * 6"
        );
        // and no 1 hour share is the plain write rate, as before the field
        assert_eq!(
            one_hour_priced().cost(1000, 0, 0, 400, 0),
            write_priced().cost(1000, 0, 0, 400, 0)
        );
    }

    /// A row that sets only the one write rate prices both caches at it, and a
    /// row that sets neither prices them as input: the 1 hour rate falls back
    /// to the write rate and then to the input rate.
    #[test]
    fn the_one_hour_rate_falls_back_to_the_write_rate_then_the_input_rate() {
        let price = write_priced();
        assert_eq!(price.cache_write_1h_per_mtok, None);
        assert_eq!(
            price.cost(1000, 0, 0, 400, 300),
            price.cost(1000, 0, 0, 400, 0)
        );

        let mut price = write_priced();
        price.cache_write_per_mtok = None;
        assert_eq!(price.cost(1000, 0, 0, 400, 300), d("0.003"));

        // a 1 hour rate with no 5 minute one: the 5 minute share is input
        price.cache_write_1h_per_mtok = Some(d("6.0"));
        // 600 fresh * 3 + 100 5 minute * 3 + 300 1 hour * 6 = 1800 + 300 + 1800
        assert_eq!(price.cost(1000, 0, 0, 400, 300), d("0.0039"));
    }

    /// The 1 hour count is a share of the writes, so a provider that reports
    /// more of it than it wrote, or more writes than the prompt holds, is not
    /// charged for tokens that are not there.
    #[test]
    fn a_one_hour_share_is_never_charged_beyond_the_writes() {
        let price = one_hour_priced();
        assert_eq!(
            price.cost(1000, 0, 0, 400, 9_999),
            price.cost(1000, 0, 0, 400, 400)
        );
        // 100-token prompt, 80 cached: only 20 tokens are left to be writes,
        // so a 1 hour share of 50 is capped at those 20
        assert_eq!(
            price.cost(100, 0, 80, 80, 50),
            price.cost(100, 0, 80, 20, 20)
        );
        // 80 * 0.3 + 20 * 6 = 24 + 120 = 144 / 1e6
        assert_eq!(price.cost(100, 0, 80, 80, 50), d("0.000144"));
    }

    /// Like its neighbours the rate is a JSON number on the wire, and a
    /// snapshot from a control plane that predates it still parses.
    #[test]
    fn the_one_hour_rate_is_a_json_number_and_optional_on_the_wire() {
        let json = serde_json::to_value(one_hour_priced()).expect("serializes");
        assert!(json["cache_write_1h_per_mtok"].is_number(), "{json}");
        let parsed: ModelPriceConfig = serde_json::from_value(json).expect("round trips");
        assert_eq!(parsed.cache_write_1h_per_mtok, Some(d("6.0")));
    }

    #[test]
    fn a_price_currency_without_a_rate_is_a_config_error() {
        // #650: pricing at face value bills the wrong number and pricing at
        // zero bills nothing, so the config is rejected instead
        let mut config = GatewayConfig::default();
        config.model_prices.push(ModelPriceConfig {
            model: "mistral-large".to_string(),
            input_per_mtok: d("2.0"),
            output_per_mtok: d("6.0"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "EUR".to_string(),
        });
        let problems = config.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("mistral-large") && p.contains("EUR")),
            "{problems:?}"
        );

        // adding a rate is the entire fix — no code change, no enum edit
        config.currency.rates.insert("EUR".to_string(), d("1.10"));
        let problems = config.validate().err().unwrap_or_default();
        assert!(
            !problems.iter().any(|p| p.contains("mistral-large")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_base_currency_price_needs_no_rate_row() {
        let mut config = GatewayConfig::default();
        config.model_prices.push(ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: d("2.5"),
            output_per_mtok: d("10.0"),
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "usd".to_string(),
        });
        let problems = config.validate().err().unwrap_or_default();
        assert!(
            !problems.iter().any(|p| p.contains("gpt-4o")),
            "{problems:?}"
        );
    }

    fn price_with(model: &str, input: &str, cached: Option<&str>) -> ModelPriceConfig {
        ModelPriceConfig {
            model: model.to_string(),
            input_per_mtok: d(input),
            output_per_mtok: d("10.0"),
            cached_input_per_mtok: cached.map(d),
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
            image_per_unit: None,
            audio_input_per_minute: None,
            speech_per_mchar: None,
            audio_input_per_mtok: None,
            audio_output_per_mtok: None,
            cached_audio_input_per_mtok: None,
            currency: "USD".to_string(),
        }
    }

    #[test]
    fn a_negative_rate_is_named_by_its_field() {
        let mut price = price_with("gpt-4o", "2.5", Some("-0.1"));
        price.output_per_mtok = d("-1");
        price.cache_write_per_mtok = Some(d("-3.75"));
        price.cache_write_1h_per_mtok = Some(d("-6"));
        let problems = price.rate_problems();
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(
            problems[0].starts_with("output_per_mtok is -1"),
            "{problems:?}"
        );
        assert!(problems[1].starts_with("cached_input_per_mtok is -0.1"));
        assert!(problems[2].starts_with("cache_write_per_mtok is -3.75"));
        assert!(problems[3].starts_with("cache_write_1h_per_mtok is -6"));

        // zero is a legitimate rate: a self-hosted model is free to call
        let mut free = price_with("local", "0", Some("0"));
        free.output_per_mtok = d("0");
        free.cache_write_per_mtok = Some(d("0"));
        free.cache_write_1h_per_mtok = Some(d("0"));
        assert!(free.rate_problems().is_empty());
    }

    #[test]
    fn a_negative_unit_rate_is_named_by_its_field() {
        let mut price = price_with("dall-e-3", "0", None);
        price.image_per_unit = Some(d("-0.04"));
        price.audio_input_per_minute = Some(d("-0.006"));
        price.speech_per_mchar = Some(d("-15"));
        let problems = price.rate_problems();
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].starts_with("image_per_unit is -0.04"));
        assert!(problems[1].starts_with("audio_input_per_minute is -0.006"));
        assert!(problems[2].starts_with("speech_per_mchar is -15"));
    }

    /// #2937: a unit rate prices its own unit and nothing else, and a price
    /// that sets no rate for the request's unit says nothing about it.
    fn audio_price() -> ModelPriceConfig {
        ModelPriceConfig {
            model: "gpt-realtime".into(),
            input_per_mtok: d("4"),
            output_per_mtok: d("16"),
            cached_input_per_mtok: Some(d("0.4")),
            audio_input_per_mtok: Some(d("32")),
            audio_output_per_mtok: Some(d("64")),
            cached_audio_input_per_mtok: Some(d("0.5")),
            ..Default::default()
        }
    }

    const M: u32 = 1_000_000;

    /// #1882: each modality is charged at its own rate, with the cache hits
    /// taken out of each side.
    #[test]
    fn audio_cost_prices_each_modality_at_its_own_rate() {
        let price = audio_price();
        // 1M text + 1M audio in, 1M text + 1M audio out, no cache
        let audio = AudioTokens {
            input: M,
            output: M,
            cached_input: 0,
        };
        assert_eq!(
            price.cost_with_audio(2 * M, 2 * M, 0, 0, 0, audio),
            d("4") + d("32") + d("16") + d("64")
        );
        // half of each side served from the cache: 0.5M text hits at 0.4,
        // 0.5M audio hits at 0.5, the rest fresh
        let cached = AudioTokens {
            input: M,
            output: 0,
            cached_input: M / 2,
        };
        assert_eq!(
            price.cost_with_audio(2 * M, 0, M, 0, 0, cached),
            d("2") + d("0.2") + d("16") + d("0.25")
        );
    }

    /// #1882: with no audio rate set a split changes nothing, and with no split
    /// the audio rates have nothing to apply to.
    #[test]
    fn audio_cost_falls_back_to_the_text_rates() {
        let text_only = ModelPriceConfig {
            model: "m".into(),
            input_per_mtok: d("4"),
            output_per_mtok: d("16"),
            cached_input_per_mtok: Some(d("0.4")),
            ..Default::default()
        };
        let split = AudioTokens {
            input: 600,
            output: 300,
            cached_input: 100,
        };
        assert_eq!(
            text_only.cost_with_audio(1000, 500, 200, 0, 0, split),
            text_only.cost(1000, 500, 200, 0, 0)
        );
        let priced = audio_price();
        assert_eq!(
            priced.cost_with_audio(1000, 500, 200, 0, 0, AudioTokens::default()),
            priced.cost(1000, 500, 200, 0, 0)
        );
    }

    /// #1882: a cached audio token falls back to the cached text rate, and
    /// without one to the audio input rate.
    #[test]
    fn cached_audio_falls_back_to_the_cached_rate_then_the_audio_rate() {
        let audio = AudioTokens {
            input: M,
            output: 0,
            cached_input: M,
        };
        let mut price = audio_price();
        price.cached_audio_input_per_mtok = None;
        assert_eq!(price.cost_with_audio(M, 0, M, 0, 0, audio), d("0.4"));
        price.cached_input_per_mtok = None;
        assert_eq!(price.cost_with_audio(M, 0, M, 0, 0, audio), d("32"));
    }

    /// #1882: counts that disagree are clamped, never priced twice or below
    /// zero: audio beyond the prompt, a cached audio count beyond the audio
    /// there is, and cache hits the provider did not split.
    #[test]
    fn audio_cost_clamps_inconsistent_counts() {
        let price = audio_price();
        // more audio than prompt: all of the prompt is audio
        let over = AudioTokens {
            input: 5 * M,
            output: 5 * M,
            cached_input: 0,
        };
        assert_eq!(
            price.cost_with_audio(M, M, 0, 0, 0, over),
            d("32") + d("64")
        );
        // an unsplit cache hit larger than the text part spills into audio:
        // 0.5M text prompt fully cached (0.2), the other 0.5M hits are cached
        // audio read as text-cached first, and what does not fit is priced as
        // fresh audio (16)
        let unsplit = AudioTokens {
            input: M / 2,
            output: 0,
            cached_input: 0,
        };
        assert_eq!(
            price.cost_with_audio(M, 0, M, 0, 0, unsplit),
            d("0.2") + d("16")
        );
    }

    /// The audio rates are rates like the others: negative ones are a problem
    /// on the row, and a snapshot from before them parses without.
    #[test]
    fn audio_rates_are_checked_and_optional_on_the_wire() {
        let mut price = audio_price();
        assert!(price.rate_problems().is_empty());
        price.audio_output_per_mtok = Some(d("-1"));
        price.cached_audio_input_per_mtok = Some(d("-2"));
        let problems = price.rate_problems();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].starts_with("audio_output_per_mtok is -1"));
        let older: ModelPriceConfig =
            serde_json::from_str(r#"{"model":"m","input_per_mtok":1,"output_per_mtok":2}"#)
                .unwrap();
        assert_eq!(older.audio_input_per_mtok, None);
        let wire: ModelPriceConfig = serde_json::from_str(
            r#"{"model":"m","audio_input_per_mtok":32,"cached_audio_input_per_mtok":0.5}"#,
        )
        .unwrap();
        assert_eq!(wire.audio_input_per_mtok, Some(d("32")));
        assert_eq!(wire.cached_audio_input_per_mtok, Some(d("0.5")));
    }

    #[test]
    fn unit_cost_prices_each_unit_at_its_own_rate() {
        let price = ModelPriceConfig {
            image_per_unit: Some(d("0.04")),
            audio_input_per_minute: Some(d("0.006")),
            speech_per_mchar: Some(d("15")),
            ..Default::default()
        };
        assert_eq!(price.unit_cost(&BilledUnits::Images(3)), Some(d("0.12")));
        // 90 seconds is a minute and a half
        assert_eq!(
            price.unit_cost(&BilledUnits::AudioSeconds(d("90"))),
            Some(d("0.009"))
        );
        assert_eq!(
            price.unit_cost(&BilledUnits::SpeechChars(20)),
            Some(d("0.0003"))
        );
        // zero units at a set rate is a priced request that cost nothing
        assert_eq!(price.unit_cost(&BilledUnits::Images(0)), Some(d("0")));
        assert_eq!(price.unit_cost(&BilledUnits::TokensOnly), None);

        let tokens_only = price_with("gpt-4o", "2.5", None);
        assert_eq!(tokens_only.unit_cost(&BilledUnits::Images(3)), None);
        assert_eq!(
            tokens_only.unit_cost(&BilledUnits::AudioSeconds(d("90"))),
            None
        );
        assert_eq!(tokens_only.unit_cost(&BilledUnits::SpeechChars(20)), None);
    }

    /// A snapshot from a control plane that predates the unit rates still
    /// parses, and the rates cross as JSON numbers like their neighbours.
    #[test]
    fn unit_rates_default_to_unset_and_round_trip_as_numbers() {
        let older: ModelPriceConfig =
            serde_json::from_str(r#"{"model":"m","input_per_mtok":1.0}"#).unwrap();
        assert_eq!(older.image_per_unit, None);
        assert_eq!(older.audio_input_per_minute, None);
        assert_eq!(older.speech_per_mchar, None);
        let price = ModelPriceConfig {
            image_per_unit: Some(d("0.04")),
            ..older
        };
        let json = serde_json::to_value(&price).unwrap();
        assert_eq!(json["image_per_unit"], 0.04);
        let back: ModelPriceConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.image_per_unit, Some(d("0.04")));
    }

    // #2889: the control plane refuses a negative rate on write, but a row
    // stored before that check still reaches the snapshot, and one of them
    // must neither 500 it for every tenant nor credit a budget
    #[test]
    fn sanitize_omits_only_the_price_with_a_negative_rate() {
        let mut config = GatewayConfig::default();
        config
            .model_prices
            .push(price_with("gpt-4o", "2.5", Some("1.25")));
        config
            .model_prices
            .push(price_with("negative-input", "-1", None));
        config
            .model_prices
            .push(price_with("negative-cached", "2.5", Some("-0.5")));

        let warnings = config.sanitize_for_snapshot();

        let kept: Vec<&str> = config
            .model_prices
            .iter()
            .map(|p| p.model.as_str())
            .collect();
        assert_eq!(kept, ["gpt-4o"]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("'negative-input'") && warnings[0].contains("input_per_mtok"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("'negative-cached'")
                && warnings[1].contains("cached_input_per_mtok"),
            "{warnings:?}"
        );
        assert!(config.validate_snapshot().is_ok());
    }

    // #2891: the 1 hour rate is one of the rates a snapshot row is held to, so
    // a negative one drops that price alone and does not 500 the snapshot
    #[test]
    fn sanitize_omits_a_price_with_a_negative_one_hour_rate() {
        let mut config = GatewayConfig::default();
        config
            .model_prices
            .push(price_with("gpt-4o", "2.5", Some("1.25")));
        let mut long = price_with("negative-long", "3", None);
        long.cache_write_1h_per_mtok = Some(d("-6"));
        config.model_prices.push(long);

        let warnings = config.sanitize_for_snapshot();

        assert_eq!(config.model_prices.len(), 1);
        assert_eq!(config.model_prices[0].model, "gpt-4o");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("'negative-long'")
                && warnings[0].contains("cache_write_1h_per_mtok"),
            "{warnings:?}"
        );
        assert!(config.validate_snapshot().is_ok());
    }

    #[test]
    fn a_file_config_with_a_negative_rate_still_fails_validation() {
        let mut config = GatewayConfig::default();
        config
            .model_prices
            .push(price_with("negative-input", "-1", None));
        let problems = config.validate().expect_err("a negative rate is refused");
        assert!(
            problems
                .iter()
                .any(|p| p.contains("model_prices['negative-input']")
                    && p.contains("input_per_mtok")),
            "{problems:?}"
        );
    }

    #[test]
    fn retry_defaults_and_backoff() {
        let r = RetryConfig::default();
        assert_eq!(r.max_retries, 2);
        // attempt 1: base 100, full jitter -> [0, 100]
        assert_eq!(r.backoff_ms(1, 1.0), 100);
        assert_eq!(r.backoff_ms(1, 0.0), 0);
        // attempt 2: 200 exp; half jitter -> 100
        assert_eq!(r.backoff_ms(2, 0.5), 100);
        // exponential growth is capped at max_backoff_ms
        assert_eq!(r.backoff_ms(10, 1.0), 2_000);
    }

    #[test]
    fn cooldown_duration_and_enable() {
        let c = CooldownConfig::default();
        assert!(c.enabled());
        assert_eq!(c.base_secs, 5);
        // no hint -> base
        assert_eq!(c.duration_secs(None), 5);
        // retry-after honoured but never below base
        assert_eq!(c.duration_secs(Some(2)), 5);
        assert_eq!(c.duration_secs(Some(30)), 30);
        // capped at max_secs
        assert_eq!(c.duration_secs(Some(10_000)), 300);
        // base 0 disables
        let off = CooldownConfig {
            base_secs: 0,
            max_secs: 300,
        };
        assert!(!off.enabled());
    }

    #[test]
    fn parses_global_and_provider_ca_bundles() {
        let config = GatewayConfig::from_toml_str(
            r#"
            [tls]
            ca_bundles = ["/etc/rolter/global.pem"]

            [[providers]]
            name = "private"
            kind = "openai_compatible"
            api_base = "https://llm.internal"
            ca_bundles = ["/etc/rolter/provider.pem"]
            "#,
        )
        .unwrap();
        assert_eq!(
            config.tls.ca_bundles,
            vec![PathBuf::from("/etc/rolter/global.pem")]
        );
        assert_eq!(
            config.ca_bundles_for(&config.providers[0]),
            vec![PathBuf::from("/etc/rolter/provider.pem")]
        );
    }

    #[test]
    fn load_rejects_missing_ca_bundle_with_path() {
        let config_path = std::env::temp_dir().join(format!(
            "rolter-missing-ca-config-{}.toml",
            std::process::id()
        ));
        let missing =
            std::env::temp_dir().join(format!("rolter-missing-ca-{}.pem", std::process::id()));
        let _ = std::fs::remove_file(&missing);
        std::fs::write(
            &config_path,
            format!(
                "[[providers]]\nname = \"p\"\nkind = \"openai_compatible\"\napi_base = \"https://llm.internal\"\nca_bundles = [\"{}\"]\n",
                missing.display()
            ),
        )
        .unwrap();
        let error = GatewayConfig::load(&config_path).unwrap_err().to_string();
        let _ = std::fs::remove_file(config_path);
        assert!(error.contains("cannot be read"));
        assert!(error.contains(&missing.display().to_string()));
    }

    #[test]
    fn load_rejects_malformed_ca_bundle() {
        let suffix = std::process::id();
        let config_path = std::env::temp_dir().join(format!("rolter-bad-ca-config-{suffix}.toml"));
        let bundle_path = std::env::temp_dir().join(format!("rolter-bad-ca-{suffix}.pem"));
        std::fs::write(
            &bundle_path,
            "-----BEGIN CERTIFICATE-----\nnot-base64!\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        std::fs::write(
            &config_path,
            format!(
                "[[providers]]\nname = \"p\"\nkind = \"openai_compatible\"\napi_base = \"https://llm.internal\"\nca_bundles = [\"{}\"]\n",
                bundle_path.display()
            ),
        )
        .unwrap();
        let error = GatewayConfig::load(&config_path).unwrap_err().to_string();
        let _ = std::fs::remove_file(config_path);
        let _ = std::fs::remove_file(bundle_path);
        assert!(error.contains("contains malformed PEM"));
    }

    #[test]
    fn expands_readonly_models_and_keeps_editable_defaults_separate() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[models.readonly]]
            model = "locked"

            [[models.default]]
            model = "editable"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.routes.len(), 1);
        assert_eq!(cfg.routes[0].model, "locked");
        assert_eq!(cfg.models.defaults.len(), 1);
        assert_eq!(cfg.models.defaults[0].model, "editable");
    }

    #[test]
    fn flat_providers_array_is_all_readonly() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.providers[0].name, "openai");
        assert!(cfg.provider_defaults.is_empty());
    }

    #[test]
    fn tiered_providers_table_splits_readonly_and_default() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers.readonly]]
            name = "locked"
            kind = "openai"
            api_base = "https://api.openai.com"

            [[providers.default]]
            name = "seeded"
            kind = "openai"
            api_base = "https://api.openai.com"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.providers[0].name, "locked");
        assert_eq!(cfg.provider_defaults.len(), 1);
        assert_eq!(cfg.provider_defaults[0].name, "seeded");
    }

    #[test]
    fn tiered_provider_groups_table_splits_tiers() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[provider_groups.readonly]]
            name = "cluster"
            slug = "cluster"
            [[provider_groups.readonly.members]]
            provider = "a"

            [[provider_groups.default]]
            name = "editable-cluster"
            slug = "editable-cluster"
            [[provider_groups.default.members]]
            provider = "b"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.provider_groups.len(), 1);
        assert_eq!(cfg.provider_groups[0].slug.as_deref(), Some("cluster"));
        assert_eq!(cfg.provider_group_defaults.len(), 1);
        assert_eq!(
            cfg.provider_group_defaults[0].slug.as_deref(),
            Some("editable-cluster")
        );
    }

    #[test]
    fn default_provider_colliding_with_readonly_slug_is_rejected() {
        let err = GatewayConfig::from_toml_str(
            r#"
            [[providers.readonly]]
            name = "vLLM"
            slug = "vllm"
            kind = "openai"
            api_base = "http://a"

            [[providers.default]]
            name = "other"
            slug = "vllm"
            kind = "openai"
            api_base = "http://b"
            "#,
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("collides with a readonly provider"));
    }

    #[test]
    fn default_group_colliding_with_readonly_slug_is_rejected() {
        let err = GatewayConfig::from_toml_str(
            r#"
            [[provider_groups.readonly]]
            name = "cluster"
            slug = "vllm-cluster"
            [[provider_groups.readonly.members]]
            provider = "a"

            [[provider_groups.default]]
            name = "dup"
            slug = "vllm-cluster"
            [[provider_groups.default.members]]
            provider = "b"
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("collides with a readonly group"));
    }

    // regression for #627: one half-built route used to fail validation and 500
    // the whole snapshot, freezing config propagation for every tenant
    fn config_with_a_good_and_a_targetless_route() -> GatewayConfig {
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "good"
            [[routes.targets]]
            provider = "openai"

            [[routes]]
            model = "half-built"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        // targets is a required toml field; clear it post-parse to reach the
        // "created the route, haven't added targets yet" state
        cfg.routes[1].targets.clear();
        cfg
    }

    fn config_with_guardrail_override(disable: &[&str], enable: &[&str]) -> GatewayConfig {
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "good"
            [[routes.targets]]
            provider = "openai"

            [[guardrails.rules]]
            name = "live"
            pattern = "secret"
            action = "block"
            "#,
        )
        .unwrap();
        let overrides = &mut cfg.routes[0].advanced.guardrails;
        overrides.disable = disable.iter().map(|s| s.to_string()).collect();
        overrides.enable = enable.iter().map(|s| s.to_string()).collect();
        cfg
    }

    #[test]
    fn sanitize_prunes_a_guardrail_override_naming_an_absent_rule() {
        let mut cfg = config_with_guardrail_override(&["paused", "live"], &["gone"]);
        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("'paused'")));
        assert!(warnings.iter().any(|w| w.contains("'gone'")));
        let overrides = &cfg.routes[0].advanced.guardrails;
        assert_eq!(overrides.disable, vec!["live".to_string()]);
        assert!(overrides.enable.is_empty());
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn a_padded_guardrail_override_matches_the_trimmed_rule_name() {
        let mut cfg = config_with_guardrail_override(&[" live "], &[]);
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert!(cfg.sanitize_for_snapshot().is_empty());
        assert_eq!(cfg.routes[0].advanced.guardrails.disable.len(), 1);
    }

    #[test]
    fn a_file_config_with_an_unknown_guardrail_override_still_fails_validation() {
        let cfg = config_with_guardrail_override(&["typo"], &[]);
        let problems = cfg.validate().expect_err("typo must be refused");
        assert!(problems
            .iter()
            .any(|p| p.contains("overrides guardrail rule 'typo'")));
    }

    #[test]
    fn sanitize_prunes_only_the_invalid_prompt_template() {
        use crate::prompt_templates::{Decorator, PromptTemplate, TemplateVariable};
        let template = |id: &str, content: &str, vars: Vec<TemplateVariable>| PromptTemplate {
            id: id.to_string(),
            version: 1,
            routes: Vec::new(),
            scopes: Vec::new(),
            variables: vars,
            decorators: vec![Decorator {
                role: Default::default(),
                position: Default::default(),
                content: content.to_string(),
            }],
        };
        let mut cfg = config_with_a_good_and_a_targetless_route();
        cfg.routes.pop();
        cfg.prompt_templates.enabled = true;
        cfg.prompt_templates.templates = vec![
            template("good", "plain", Vec::new()),
            template("undeclared", "hi {{ who }}", Vec::new()),
            template(
                "both",
                "x",
                vec![TemplateVariable {
                    name: "v".into(),
                    required: true,
                    default: Some("d".into()),
                }],
            ),
        ];
        assert!(cfg.validate().is_err(), "precondition: config invalid");

        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings
            .iter()
            .any(|w| w.contains("'undeclared'") && w.contains("undeclared variable 'who'")));
        assert!(warnings.iter().any(|w| w.contains("'both'")));
        let ids: Vec<_> = cfg
            .prompt_templates
            .templates
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["good"]);
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn sanitize_drops_an_unservable_route_and_keeps_the_valid_one() {
        let mut cfg = config_with_a_good_and_a_targetless_route();
        assert!(
            cfg.validate().is_err(),
            "precondition: whole config invalid"
        );

        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("half-built"));

        let models: Vec<_> = cfg.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, vec!["good"]);
        // the surviving config now serves, instead of 500ing the snapshot
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn sanitize_drops_a_route_whose_targets_are_all_unknown_providers() {
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "ghost"
            [[routes.targets]]
            provider = "deleted-provider"
            "#,
        )
        .unwrap();
        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(warnings.len(), 1);
        assert!(cfg.routes.is_empty());
    }

    /// two providers; `pool` and `variant` are TOML target tables for the
    /// route `m`, so a case states only what differs
    fn restricted_route_config(pool: &str, variant: &str) -> GatewayConfig {
        toml::from_str(&format!(
            r#"
            [[providers]]
            name = "a"
            kind = "openai"
            api_base = "https://api.openai.com/v1"
            [[providers]]
            name = "b"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "m"
            {pool}
            {variant}

            [[routes]]
            model = "ok"
            [[routes.targets]]
            provider = "a"
            "#
        ))
        .unwrap()
    }

    const ORDINARY_POOL: &str = r#"[[routes.targets]]
            provider = "a""#;
    const RESTRICTED_VARIANT: &str = r#"[[routes.variants]]
            name = "v"
            [[routes.variants.targets]]
            provider = "b"
            takes_over = ["throttled"]"#;
    const ORDINARY_VARIANT: &str = r#"[[routes.variants]]
            name = "v"
            [[routes.variants.targets]]
            provider = "b""#;

    #[test]
    fn a_route_with_variants_dials_only_them_when_checking_for_restricted_targets() {
        // the pool has an ordinary target but the data plane never consults it
        let stranded = restricted_route_config(ORDINARY_POOL, RESTRICTED_VARIANT);
        let problems = stranded.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains(crate::failover::ALL_TARGETS_RESTRICTED)),
            "{problems:?}"
        );
        // an ordinary variant target serves, whatever the pool holds
        let served = restricted_route_config(ORDINARY_POOL, ORDINARY_VARIANT);
        assert!(served.validate().is_ok(), "{:?}", served.validate());
        // with no variants the pool is what is dialled
        let pool_only = restricted_route_config(
            r#"[[routes.targets]]
            provider = "b"
            takes_over = ["throttled"]"#,
            "",
        );
        assert!(pool_only.validate().is_err());
    }

    #[test]
    fn the_flag_form_of_the_dialled_rule_counts_variants_over_the_pool() {
        let none: [[bool; 0]; 0] = [];
        // without variants the pool is dialled
        assert!(only_restricted_dialled([true, true], none));
        assert!(!only_restricted_dialled([true, false], none));
        // nothing to dial is not "only restricted"
        assert!(!only_restricted_dialled([], none));
        // with variants the pool does not count, whatever it holds
        assert!(only_restricted_dialled([false], [vec![true], vec![true]]));
        assert!(!only_restricted_dialled([true], [vec![true], vec![false]]));
    }

    #[test]
    fn sanitize_drops_a_route_left_with_only_restricted_targets() {
        let mut cfg = restricted_route_config(ORDINARY_POOL, RESTRICTED_VARIANT);
        let report = cfg.sanitize_for_snapshot_report();
        let models: Vec<_> = cfg.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["ok"]);
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.contains("route 'm'") && p.contains("all_targets_restricted")),
            "{:?}",
            report.problems
        );
        // the rest of the snapshot still validates and applies
        assert_eq!(cfg.validate_snapshot(), Ok(()));
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn sanitize_keeps_a_servable_duplicate_of_a_dropped_model() {
        // pruning is by index, so an unservable row must not take its servable
        // twin with it — validate still reports the duplication itself
        let mut cfg = config_with_a_good_and_a_targetless_route();
        cfg.routes[1].model = "good".to_string();

        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(warnings.len(), 1);
        assert_eq!(cfg.routes.len(), 1);
        assert_eq!(cfg.routes[0].targets.len(), 1, "kept the servable twin");
    }

    /// The #926 report exactly: one openrouter provider with a wrong
    /// `api_base` alongside a healthy fleet. Fourteen valid providers and
    /// eleven valid routes did not propagate because of it.
    #[test]
    fn one_invalid_provider_does_not_withhold_the_whole_fleet() {
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[providers]]
            name = "openrouter-edge"
            kind = "openrouter"
            api_base = "https://openrouter.example.com/v1"
            api_key_env = "OPENROUTER_API_KEY"

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        assert!(
            cfg.validate().is_err(),
            "precondition: the whole config is invalid today"
        );

        let warnings = cfg.sanitize_for_snapshot();

        // the bad row is gone, and says why
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("openrouter-edge"), "{warnings:?}");
        assert!(
            warnings[0].contains("https://openrouter.ai/api/v1"),
            "the reason has to ride along, not just the fact: {warnings:?}"
        );

        // and everything that was fine still propagates
        let providers: Vec<_> = cfg.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(providers, vec!["openai"]);
        let models: Vec<_> = cfg.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, vec!["gpt-4o"]);
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn dropping_a_provider_drops_the_routes_that_needed_it() {
        // the gateway already refuses a route it cannot resolve, so shipping a
        // route pointing at a dropped provider would only move the error later
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[providers]]
            name = "broken"
            kind = "openai"
            api_base = "not-a-url"

            [[routes]]
            model = "served"
            [[routes.targets]]
            provider = "openai"

            [[routes]]
            model = "orphaned"
            [[routes.targets]]
            provider = "broken"
            "#,
        )
        .unwrap();

        let warnings = cfg.sanitize_for_snapshot();
        // the provider, the target that named it, and the route left empty
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("'broken'")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'orphaned' omitted")),
            "{warnings:?}"
        );

        let models: Vec<_> = cfg.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, vec!["served"]);
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    /// A healthy provider beside one `provider_problems` omits (an openrouter
    /// `api_base` that is not openrouter's), which the tests below hang
    /// references on. `rest` is the routes, keys and groups that name them.
    fn fleet_with_an_omitted_provider(rest: &str) -> GatewayConfig {
        let head = r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[providers]]
            name = "bad"
            kind = "openrouter"
            api_base = "https://openrouter.example.com/v1"
            api_key_env = "X"
        "#;
        toml::from_str(&format!("{head}\n{rest}")).unwrap()
    }

    /// #2945: a route that names the omitted provider and a healthy one kept
    /// its dangling target, so `validate_snapshot` refused the snapshot for
    /// every tenant over one provider's defect.
    #[test]
    fn sanitize_prunes_the_target_of_an_omitted_provider_beside_a_healthy_one() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "openai"
            [[routes.targets]]
            provider = "bad"
            "#,
        );
        assert!(
            cfg.validate_snapshot().is_err(),
            "precondition: the dangling target is refused"
        );

        let warnings = cfg.sanitize_for_snapshot();

        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("provider 'bad' omitted"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("route 'm' target 'bad' omitted"),
            "{warnings:?}"
        );
        let targets: Vec<_> = cfg.routes[0]
            .targets
            .iter()
            .map(|t| t.provider.as_str())
            .collect();
        assert_eq!(targets, ["openai"], "the route keeps its healthy target");
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    /// #3431: pruning the omitted provider's ordinary target can leave a pool of
    /// only fallbacks, which `validate_snapshot` would refuse for every tenant
    #[test]
    fn sanitize_drops_a_route_whose_ordinary_target_was_pruned_with_its_provider() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "bad"
            [[routes.targets]]
            provider = "openai"
            takes_over = ["throttled"]

            [[routes]]
            model = "ok"
            [[routes.targets]]
            provider = "openai"
            "#,
        );
        assert!(cfg.validate_snapshot().is_err(), "precondition");

        let warnings = cfg.sanitize_for_snapshot();

        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'm'") && w.contains("all_targets_restricted")),
            "{warnings:?}"
        );
        let models: Vec<_> = cfg.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["ok"]);
        assert_eq!(cfg.validate_snapshot(), Ok(()));
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    /// #2950: the loader left the provider out before the sanitizer ever saw
    /// it, so the file's reference to it was an unknown name and refused.
    #[test]
    fn sanitize_prunes_references_to_a_provider_the_store_loader_left_out() {
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "openai"
            [[routes.targets]]
            provider = "stored"

            [[virtual_keys]]
            key = "k"
            providers = ["openai", "stored"]
            "#,
        )
        .unwrap();
        assert!(
            cfg.validate_snapshot().is_err(),
            "precondition: an unknown name is refused"
        );
        cfg.left_out_providers
            .insert((String::new(), "stored".to_string()));

        let warnings = cfg.sanitize_for_snapshot();

        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'm' target 'stored' omitted")),
            "{warnings:?}"
        );
        assert_eq!(cfg.routes[0].targets.len(), 1);
        assert_eq!(cfg.virtual_keys[0].providers, ["openai"]);
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    /// #1876: what the left-out prune takes from the file is recorded like any
    /// other omission, so a client sees the route and key that stopped being
    /// served. The left-out provider itself is a store row the config read does
    /// not list, so it has no position to point at and stays a line only.
    #[test]
    fn rows_dropped_for_a_left_out_provider_are_recorded() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "kept"
            [[routes.targets]]
            provider = "openai"

            [[routes]]
            model = "stranded"
            [[routes.targets]]
            provider = "stored"

            [[virtual_keys]]
            key = "k-served"
            name = "served"

            [[virtual_keys]]
            key = "k-stranded"
            name = "stranded-key"
            providers = ["stored"]
            "#,
        );
        cfg.left_out_providers
            .insert((String::new(), "stored".to_string()));

        let report = cfg.sanitize_for_snapshot_report();

        let at: Vec<_> = report
            .omitted
            .iter()
            .filter(|o| o.kind != OmittedKind::Provider)
            .map(|o| (o.kind, o.name.as_str(), o.index))
            .collect();
        assert_eq!(
            at,
            [
                (OmittedKind::Route, "stranded", 1),
                (OmittedKind::VirtualKey, "stranded-key", 1),
            ],
            "{:?}",
            report.problems
        );
    }

    /// A left-out name that a file provider also carries is that provider.
    #[test]
    fn a_left_out_name_a_file_provider_carries_is_not_pruned() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "openai"
            "#,
        );
        cfg.left_out_providers
            .insert((String::new(), "openai".to_string()));
        let warnings = cfg.sanitize_for_snapshot();
        assert_eq!(cfg.routes[0].targets.len(), 1, "{warnings:?}");
    }

    #[test]
    fn sanitize_prunes_omitted_provider_variant_targets_and_the_variant_it_empties() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "split"
            [[routes.variants]]
            name = "only-bad"
            [[routes.variants.targets]]
            provider = "bad"
            [[routes.variants]]
            name = "mixed"
            [[routes.variants.targets]]
            provider = "bad"
            [[routes.variants.targets]]
            provider = "openai"

            [[routes]]
            model = "all-bad"
            [[routes.variants]]
            name = "a"
            [[routes.variants.targets]]
            provider = "bad"
            "#,
        );

        let warnings = cfg.sanitize_for_snapshot();

        let split = cfg.routes.iter().find(|r| r.model == "split").unwrap();
        let variants: Vec<_> = split.variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            variants,
            ["mixed"],
            "a variant with no target left is removed, since validate reads a targetless one as a defect of its own"
        );
        let targets: Vec<_> = split.variants[0]
            .targets
            .iter()
            .map(|t| t.provider.as_str())
            .collect();
        assert_eq!(targets, ["openai"]);
        assert!(
            cfg.routes.iter().all(|r| r.model != "all-bad"),
            "a route whose every variant went has nothing to serve: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'split' variant 'only-bad' omitted")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'split' variant 'mixed' target 'bad' omitted")),
            "{warnings:?}"
        );
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    #[test]
    fn sanitize_prunes_omitted_provider_group_members() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[provider_groups]]
            name = "pool"
            [[provider_groups.members]]
            provider = "openai"
            [[provider_groups.members]]
            provider = "bad"

            [[provider_groups]]
            name = "doomed"
            [[provider_groups.members]]
            provider = "bad"
            "#,
        );

        let warnings = cfg.sanitize_for_snapshot();

        let members = |slug: &str| -> Vec<String> {
            cfg.provider_groups
                .iter()
                .find(|g| g.name == slug)
                .unwrap()
                .members
                .iter()
                .map(|m| m.provider.clone())
                .collect()
        };
        assert_eq!(members("pool"), ["openai"]);
        // a group with no member never resolves, which is how it already reads
        assert!(members("doomed").is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("provider group 'pool' member 'bad' omitted")),
            "{warnings:?}"
        );
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    /// An empty `providers` allow-list means every provider (the gateway's
    /// `KeyMeta::provider_allowed`), so emptying one by pruning would widen the
    /// key. A key left with an entry is only narrowed to its other providers; a
    /// key left with none is denied by leaving it out.
    #[test]
    fn sanitize_never_widens_a_key_by_emptying_its_provider_allow_list() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "openai"

            [[virtual_keys]]
            key = "sk-mixed-secret"
            name = "mixed"
            providers = ["bad", "openai"]

            [[virtual_keys]]
            key = "sk-only-bad-secret"
            name = "only-bad"
            providers = ["bad"]

            [[virtual_keys]]
            key = "sk-open-secret"
            name = "open"

            [[virtual_keys]]
            key = "sk-healthy-secret"
            name = "healthy"
            providers = ["openai"]
            "#,
        );
        let db_key = |id: &str, providers: &[&str]| VirtualKeyRecord {
            key_hash: format!("hash-{id}"),
            id: id.to_string(),
            providers: providers.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        };
        cfg.db_virtual_keys = vec![
            db_key("db-mixed", &["bad", "openai"]),
            db_key("db-only-bad", &["bad", "bad"]),
            db_key("db-open", &[]),
        ];
        assert!(
            cfg.validate_snapshot().is_err(),
            "precondition: a file key naming the omitted provider is refused"
        );

        let warnings = cfg.sanitize_for_snapshot();

        let file_keys: Vec<(&str, &[String])> = cfg
            .virtual_keys
            .iter()
            .map(|k| (k.name.as_deref().unwrap(), k.providers.as_slice()))
            .collect();
        assert_eq!(
            file_keys,
            [
                ("mixed", &["openai".to_string()][..]),
                ("open", &[][..]),
                ("healthy", &["openai".to_string()][..]),
            ],
            "the key that allowed only an omitted provider is gone, not left allowing everything"
        );
        let db_keys: Vec<(&str, &[String])> = cfg
            .db_virtual_keys
            .iter()
            .map(|k| (k.id.as_str(), k.providers.as_slice()))
            .collect();
        assert_eq!(
            db_keys,
            [
                ("db-mixed", &["openai".to_string()][..]),
                ("db-open", &[][..]),
            ]
        );
        for needle in [
            "virtual key 'only-bad' omitted",
            "virtual key 'mixed' allow-list entries 'bad' omitted",
            "virtual key 'db-only-bad' omitted",
            "virtual key 'db-mixed' allow-list entries 'bad' omitted",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "{needle}: {warnings:?}"
            );
        }
        // the problem lines name a key, never its secret
        assert!(
            warnings.iter().all(|w| !w.contains("-secret")),
            "{warnings:?}"
        );
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    #[test]
    fn sanitize_leaves_a_key_alone_when_it_names_no_omitted_provider() {
        // pruning is by what was omitted, not by what is unknown: a name no
        // provider ever carried is a typo `validate_snapshot` still refuses in
        // a file config
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "openai"

            [[virtual_keys]]
            key = "sk-typo"
            name = "typo"
            providers = ["opennai"]
            "#,
        );

        cfg.sanitize_for_snapshot();

        assert_eq!(cfg.virtual_keys.len(), 1);
        assert_eq!(cfg.virtual_keys[0].providers, ["opennai"]);
        assert!(cfg.validate_snapshot().is_err());
    }

    /// #1876: a client points at a row by its loaded position, so two routes
    /// sharing a model, or two keys sharing a name, are told apart.
    #[test]
    fn the_report_points_at_the_exact_row_dropped() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "twin"
            [[routes.targets]]
            provider = "openai"

            [[routes]]
            model = "twin"
            [[routes.targets]]
            provider = "bad"

            [[virtual_keys]]
            key = "sk-first-secret"
            name = "twin"
            providers = ["openai"]

            [[virtual_keys]]
            key = "sk-second-secret"
            name = "twin"
            providers = ["bad"]
            "#,
        );

        let report = cfg.sanitize_for_snapshot_report();

        let at = |kind: OmittedKind| -> Vec<(&str, usize)> {
            report
                .omitted
                .iter()
                .filter(|o| o.kind == kind)
                .map(|o| (o.name.as_str(), o.index))
                .collect()
        };
        assert_eq!(at(OmittedKind::Provider), [("bad", 1)]);
        assert_eq!(
            at(OmittedKind::Route),
            [("twin", 1)],
            "the second twin only"
        );
        assert_eq!(
            at(OmittedKind::VirtualKey),
            [("twin", 1)],
            "the second twin only"
        );
        assert!(report.omitted.iter().all(|o| !o.reason.is_empty()));
        // a part dropped from an entry that is still served is a line, not a record
        assert!(report.problems.len() >= report.omitted.len());
    }

    /// #1876: a key dropped by the example-key prune keeps the index it had in
    /// the loaded list even when an earlier key was already dropped.
    #[test]
    fn the_example_key_record_keeps_its_loaded_index() {
        let mut cfg = fleet_with_an_omitted_provider(&format!(
            r#"
            [[virtual_keys]]
            key = "sk-only-bad-secret"
            name = "only-bad"
            providers = ["bad"]

            [[virtual_keys]]
            key = "sk-served-secret"
            name = "served"

            [[virtual_keys]]
            key = "{PUBLIC_EXAMPLE_KEY}"
            "#
        ));

        let mut report = cfg.sanitize_for_snapshot_report();
        cfg.prune_public_example_key(&mut report);

        let keys: Vec<_> = report
            .omitted
            .iter()
            .filter(|o| o.kind != OmittedKind::Provider)
            .map(|o| (o.kind, o.name.as_str(), o.index))
            .collect();
        assert_eq!(
            keys,
            [
                (OmittedKind::VirtualKey, "only-bad", 0),
                (OmittedKind::PublicExampleKey, "<unnamed>", 2),
            ]
        );
        assert_eq!(cfg.virtual_keys.len(), 1);
        assert!(report
            .problems
            .iter()
            .any(|p| p.contains("it is the public example key")));
    }

    #[test]
    fn sanitize_switches_off_a_semantic_cache_whose_embeddings_provider_was_omitted() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "cached"
            [[routes.targets]]
            provider = "openai"
            [routes.cache]
            ttl_secs = 60
            [routes.cache.semantic]
            provider = "bad"
            model = "text-embedding-3-small"

            [[routes]]
            model = "kept"
            [[routes.targets]]
            provider = "openai"
            [routes.cache.semantic]
            provider = "openai"
            model = "text-embedding-3-small"
            "#,
        );
        assert!(
            cfg.validate_snapshot().is_err(),
            "precondition: the dangling embeddings provider is refused"
        );

        let warnings = cfg.sanitize_for_snapshot();

        let cached = cfg.routes.iter().find(|r| r.model == "cached").unwrap();
        let cache = cached.cache.as_ref().expect("the route keeps its cache");
        assert!(cache.semantic.is_none(), "semantic lookup is off");
        assert_eq!(
            cache.ttl_secs,
            Some(60),
            "the rest of the cache is untouched"
        );
        assert!(
            cfg.routes.iter().any(|r| r.model == "cached"),
            "the route keeps serving"
        );
        let kept = cfg.routes.iter().find(|r| r.model == "kept").unwrap();
        assert!(
            kept.cache.as_ref().unwrap().semantic.is_some(),
            "a semantic cache on a healthy provider is not touched"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'cached' semantic cache omitted")
                    && w.contains("'bad'")),
            "{warnings:?}"
        );
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    #[test]
    fn a_dropped_route_does_not_also_report_its_semantic_cache() {
        let mut cfg = fleet_with_an_omitted_provider(
            r#"
            [[routes]]
            model = "gone"
            [[routes.targets]]
            provider = "bad"
            [routes.cache.semantic]
            provider = "bad"
            model = "text-embedding-3-small"
            "#,
        );

        let warnings = cfg.sanitize_for_snapshot();

        assert!(cfg.routes.is_empty());
        assert!(
            warnings.iter().all(|w| !w.contains("semantic cache")),
            "{warnings:?}"
        );
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    #[test]
    fn a_name_another_provider_still_carries_is_not_treated_as_omitted() {
        // two rows under one name is a defect `validate` reports; once the
        // invalid one is gone the name resolves, and pruning every reference
        // would take the surviving provider's traffic with it
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "dup"
            kind = "openai"
            api_base = "not-a-url"

            [[providers]]
            name = "dup"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "dup"
            "#,
        )
        .unwrap();

        cfg.sanitize_for_snapshot();

        assert_eq!(cfg.routes[0].targets.len(), 1);
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    #[test]
    fn sanitize_prunes_a_variant_a_cross_project_prune_leaves_empty() {
        let mut cfg = GatewayConfig {
            providers: vec![
                scoped_provider("p1-private", "org-1", Some("proj-1")),
                scoped_provider("shared", "org-1", None),
            ],
            routes: vec![{
                let mut route = route_in("split", "proj-2", &[]);
                route.variants = vec![
                    Variant {
                        name: "foreign".to_string(),
                        weight: 1,
                        targets: route_in("x", "proj-2", &["p1-private"]).targets,
                        params: Default::default(),
                    },
                    Variant {
                        name: "shared".to_string(),
                        weight: 1,
                        targets: route_in("x", "proj-2", &["shared"]).targets,
                        params: Default::default(),
                    },
                ];
                route
            }],
            ..Default::default()
        };

        cfg.sanitize_for_snapshot();

        let variants: Vec<_> = cfg.routes[0]
            .variants
            .iter()
            .map(|v| v.name.as_str())
            .collect();
        assert_eq!(variants, ["shared"]);
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );
    }

    #[test]
    fn a_valid_fleet_is_never_reshaped_by_sanitizing() {
        // the common path: sanitize must be a no-op, or it is a silent config
        // rewriter rather than a safety net
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        let before = cfg.clone();
        assert!(cfg.sanitize_for_snapshot().is_empty());
        assert_eq!(cfg.providers.len(), before.providers.len());
        assert_eq!(cfg.routes.len(), before.routes.len());
    }

    /// Two orgs that each have an `openai` provider and a `gpt-4o` route on it,
    /// as the store composes them (#1857).
    fn two_orgs_one_name() -> GatewayConfig {
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "openai"

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        for (i, org) in ["org-a", "org-b"].into_iter().enumerate() {
            let tenancy = Some(Tenancy {
                org_id: org.to_string(),
                project_id: None,
            });
            cfg.providers[i].tenancy = tenancy.clone();
            cfg.routes[i].tenancy = tenancy;
        }
        cfg
    }

    #[test]
    fn two_orgs_may_each_name_a_route_and_a_provider_the_same() {
        let mut cfg = two_orgs_one_name();
        assert!(cfg.sanitize_for_snapshot().is_empty());
        assert_eq!((cfg.providers.len(), cfg.routes.len()), (2, 2));
        cfg.validate_snapshot().unwrap();
    }

    #[test]
    fn one_org_still_may_not_name_two_providers_or_routes_the_same() {
        let mut cfg = two_orgs_one_name();
        cfg.providers[1].tenancy = cfg.providers[0].tenancy.clone();
        cfg.routes[1].tenancy = cfg.routes[0].tenancy.clone();
        let problems = cfg.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p == "duplicate provider name 'openai' in org 'org-a'"),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p == "duplicate route model 'gpt-4o' in org 'org-a'"),
            "{problems:?}"
        );
    }

    #[test]
    fn a_route_resolves_its_own_orgs_provider_then_a_global_one_never_another_orgs() {
        let mut cfg = two_orgs_one_name();
        // org-b's provider is renamed away: org-b's route now names nothing of
        // its own, and org-a's provider is not a fallback for it
        cfg.providers[1].name = "elsewhere".to_string();
        let problems = cfg.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("route 'gpt-4o' targets unknown provider 'openai'")),
            "{problems:?}"
        );
        // a global provider of that name is one
        let mut global = cfg.providers[0].clone();
        global.tenancy = None;
        cfg.providers.push(global);
        cfg.validate().unwrap();
    }

    #[test]
    fn a_deployment_wide_route_never_reaches_one_orgs_provider() {
        let mut cfg = two_orgs_one_name();
        // org-b's provider goes, so org-a is the only org with an `openai`; a
        // global route naming it with a global fallback beside it
        cfg.providers.truncate(1);
        cfg.routes.truncate(1);
        let mut global: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "local"
            kind = "openai"
            api_base = "http://127.0.0.1:8000/v1"

            [[routes]]
            model = "shared"
            [[routes.targets]]
            provider = "openai"
            [[routes.targets]]
            provider = "local"
            "#,
        )
        .unwrap();
        cfg.providers.append(&mut global.providers);
        cfg.routes.append(&mut global.routes);
        let problems = cfg.sanitize_for_snapshot();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("deployment-wide providers only")),
            "{problems:?}"
        );
        let shared = cfg.routes.iter().find(|r| r.model == "shared").unwrap();
        let targets: Vec<&str> = shared.targets.iter().map(|t| t.provider.as_str()).collect();
        assert_eq!(targets, ["local"]);
        cfg.validate_snapshot().unwrap();
    }

    #[test]
    fn sanitizing_one_orgs_broken_provider_leaves_the_other_orgs_namesake_alone() {
        let mut cfg = two_orgs_one_name();
        cfg.providers[0].api_base = "not a url".to_string();
        let problems = cfg.sanitize_for_snapshot();
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("provider 'openai' omitted")),
            "{problems:?}"
        );
        // org-a's route lost its only target and went; org-b's kept its own
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.routes.len(), 1);
        assert_eq!(
            cfg.routes[0].tenancy.as_ref().map(|t| t.org_id.as_str()),
            Some("org-b")
        );
        assert_eq!(cfg.routes[0].targets.len(), 1);
        cfg.validate_snapshot().unwrap();
    }

    /// #3464: the store left out one org's `openai` because its row could not
    /// be read. Only that org's references go: the other org's route and group
    /// keep their own `openai`, and the left-out org's route does not fall
    /// through to the global namesake its own row shadowed.
    #[test]
    fn a_left_out_provider_prunes_only_its_own_orgs_references() {
        let mut cfg = two_orgs_one_name();
        let mut extra: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[provider_groups]]
            name = "pool-a"
            slug = "pool-a"
            [[provider_groups.members]]
            provider = "openai"

            [[provider_groups]]
            name = "pool-b"
            slug = "pool-b"
            [[provider_groups.members]]
            provider = "openai"

            [[virtual_keys]]
            key = "k"
            name = "operator"
            providers = ["openai"]
            "#,
        )
        .unwrap();
        for (i, org) in ["org-a", "org-b"].into_iter().enumerate() {
            let tenancy = Some(Tenancy {
                org_id: org.to_string(),
                project_id: None,
            });
            extra.provider_groups[i].tenancy = tenancy;
        }
        // org-a's row never reached the config; the global `openai` did
        cfg.providers.remove(0);
        cfg.providers.append(&mut extra.providers);
        cfg.provider_groups.append(&mut extra.provider_groups);
        cfg.virtual_keys.append(&mut extra.virtual_keys);
        cfg.left_out_providers
            .insert(("org-a".to_string(), "openai".to_string()));

        let problems = cfg.sanitize_for_snapshot();

        let org = |t: Option<&Tenancy>| t.map(|t| t.org_id.clone());
        let routes: Vec<_> = cfg
            .routes
            .iter()
            .map(|r| (org(r.tenancy.as_ref()), r.targets.len()))
            .collect();
        assert_eq!(routes, [(Some("org-b".into()), 1)], "{problems:?}");
        let groups: Vec<_> = cfg
            .provider_groups
            .iter()
            .map(|g| (g.name.as_str(), g.members.len()))
            .collect();
        // org-a's group lost its member, and an empty group serves nothing
        assert_eq!(groups, [("pool-a", 0), ("pool-b", 1)], "{problems:?}");
        // the operator's key resolves the global `openai`, which is served
        assert_eq!(cfg.virtual_keys[0].providers, ["openai"], "{problems:?}");
        cfg.validate_snapshot().unwrap();
    }

    #[test]
    fn a_gateway_that_predates_namespaces_gets_every_colliding_org_row_left_out() {
        let mut cfg = two_orgs_one_name();
        let mut global: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "local"
            kind = "openai"
            api_base = "http://127.0.0.1:8000/v1"

            [[routes]]
            model = "gpt-4o"
            [[routes.targets]]
            provider = "local"
            "#,
        )
        .unwrap();
        cfg.providers.append(&mut global.providers);
        cfg.routes.append(&mut global.routes);
        let mut problems = crate::namespace::fold_for_legacy_gateway(&mut cfg);
        problems.extend(cfg.sanitize_for_snapshot());
        // both orgs' rows go, the global ones stay, and nothing is guessed
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(cfg.providers[0].name, "local");
        assert_eq!(cfg.routes.len(), 1);
        assert!(cfg.routes[0].tenancy.is_none());
        assert_eq!(
            problems
                .iter()
                .filter(|p| p.contains("predates per-org namespaces"))
                .count(),
            4,
            "{problems:?}"
        );
        cfg.validate_snapshot().unwrap();
    }

    #[test]
    fn folding_for_an_older_gateway_costs_nothing_without_a_repeated_name() {
        let mut cfg = two_orgs_one_name();
        cfg.providers[1].name = "openai-b".to_string();
        cfg.providers[1].slug = Some("openai-b".to_string());
        cfg.routes[1].model = "gpt-4o-b".to_string();
        cfg.routes[1].targets[0].provider = "openai-b".to_string();
        assert!(crate::namespace::fold_for_legacy_gateway(&mut cfg).is_empty());
        assert_eq!((cfg.providers.len(), cfg.routes.len()), (2, 2));
    }

    #[test]
    fn a_duplicate_provider_name_still_refuses_rather_than_guessing() {
        // not row-local: both rows are individually fine, so dropping one would
        // be a coin flip over which provider the fleet talks to
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://proxy.example.com/v1"
            "#,
        )
        .unwrap();
        assert!(cfg.sanitize_for_snapshot().is_empty());
        assert_eq!(cfg.providers.len(), 2);
        let problems = cfg.validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("duplicate provider name")),
            "{problems:?}"
        );
    }

    #[test]
    fn sanitize_leaves_structural_problems_for_validate() {
        // a duplicate model where *both* rows are servable is not row-local:
        // dropping one silently would be worse than refusing the snapshot
        let mut cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "openai"
            kind = "openai"
            api_base = "https://api.openai.com/v1"

            [[routes]]
            model = "dup"
            [[routes.targets]]
            provider = "openai"

            [[routes]]
            model = "dup"
            [[routes.targets]]
            provider = "openai"
            "#,
        )
        .unwrap();
        assert!(cfg.sanitize_for_snapshot().is_empty());
        assert_eq!(cfg.routes.len(), 2);
        let problems = cfg.validate().unwrap_err();
        assert!(problems.iter().any(|p| p.contains("duplicate route model")));
    }

    fn scoped_provider(name: &str, org: &str, project: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            kind: ProviderKind::OpenaiCompatible,
            api_base: "https://example.com".to_string(),
            tenancy: Some(Tenancy {
                org_id: org.to_string(),
                project_id: project.map(str::to_string),
            }),
            project_scoped: project.is_some(),
            ..Default::default()
        }
    }

    fn route_in(model: &str, project: &str, providers: &[&str]) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            strategy: BalancingStrategy::default(),
            targets: providers
                .iter()
                .map(|provider| Target {
                    provider: provider.to_string(),
                    ..Default::default()
                })
                .collect(),
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
            tenancy: Some(Tenancy {
                org_id: "org-1".to_string(),
                project_id: Some(project.to_string()),
            }),
        }
    }

    // #1919: the control plane refuses a target on a provider scoped to another
    // project, so one that exists came from SQL or a seed. only that target is
    // pruned, not the snapshot, and the route survives on what it may still use
    #[test]
    fn sanitize_prunes_a_target_on_a_provider_scoped_to_another_project() {
        let mut cfg = GatewayConfig {
            providers: vec![
                scoped_provider("p1-private", "org-1", Some("proj-1")),
                scoped_provider("shared", "org-1", None),
            ],
            routes: vec![
                // its own project's provider plus an org-wide one: untouched
                route_in("ok", "proj-1", &["p1-private", "shared"]),
                // another project's provider: that target goes, the fallback stays
                route_in("crossed", "proj-2", &["p1-private", "shared"]),
                // nothing left to serve: dropped by the existing pass
                route_in("only-foreign", "proj-2", &["p1-private"]),
            ],
            ..Default::default()
        };
        let warnings = cfg.sanitize_for_snapshot();
        let targets = |model: &str| -> Option<Vec<String>> {
            cfg.routes
                .iter()
                .find(|r| r.model == model)
                .map(|r| r.targets.iter().map(|t| t.provider.clone()).collect())
        };
        assert_eq!(targets("ok").unwrap(), ["p1-private", "shared"]);
        assert_eq!(targets("crossed").unwrap(), ["shared"]);
        assert!(targets("only-foreign").is_none(), "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("route 'crossed'") && w.contains("'p1-private'")),
            "{warnings:?}"
        );
        // the snapshot itself is still valid
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn sanitize_prunes_a_group_member_scoped_to_a_project_the_group_is_not() {
        let member = |provider: &str| GroupMember {
            provider: provider.to_string(),
            model: None,
            weight: 1,
        };
        let group = |slug: &str, project: Option<&str>| ProviderGroupConfig {
            name: slug.to_string(),
            slug: Some(slug.to_string()),
            members: vec![member("p1-private"), member("shared")],
            tenancy: Some(Tenancy {
                org_id: "org-1".to_string(),
                project_id: project.map(str::to_string),
            }),
            project_scoped: project.is_some(),
            ..Default::default()
        };
        let mut cfg = GatewayConfig {
            providers: vec![
                scoped_provider("p1-private", "org-1", Some("proj-1")),
                scoped_provider("shared", "org-1", None),
            ],
            provider_groups: vec![
                group("own", Some("proj-1")),
                group("other", Some("proj-2")),
                // an org-wide group would hand the private provider to every project
                group("wide", None),
            ],
            ..Default::default()
        };
        let warnings = cfg.sanitize_for_snapshot();
        let members = |slug: &str| -> Vec<String> {
            cfg.provider_groups
                .iter()
                .find(|g| g.slug.as_deref() == Some(slug))
                .unwrap()
                .members
                .iter()
                .map(|m| m.provider.clone())
                .collect()
        };
        assert_eq!(members("own"), ["p1-private", "shared"]);
        assert_eq!(members("other"), ["shared"]);
        assert_eq!(members("wide"), ["shared"]);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }

    #[test]
    fn a_scoped_provider_admits_only_keys_of_its_project() {
        let owned = |project: Option<&str>| {
            Some(Tenancy {
                org_id: "org-1".to_string(),
                project_id: project.map(str::to_string),
            })
        };
        let scoped = owned(Some("proj-1"));
        assert!(Tenancy::admits_scoped(scoped.as_ref(), "org-1", "proj-1"));
        assert!(!Tenancy::admits_scoped(scoped.as_ref(), "org-1", "proj-2"));
        // an org with no project is not the project
        assert!(!Tenancy::admits_scoped(scoped.as_ref(), "org-1", ""));
        assert!(!Tenancy::admits_scoped(scoped.as_ref(), "org-2", "proj-1"));
        // the operator's own key carries no org and is not narrowed
        assert!(Tenancy::admits_scoped(scoped.as_ref(), "", ""));
        // org-wide: every project of the org, nobody outside it
        let wide = owned(None);
        assert!(Tenancy::admits_scoped(wide.as_ref(), "org-1", "proj-2"));
        assert!(Tenancy::admits_scoped(wide.as_ref(), "org-1", ""));
        assert!(!Tenancy::admits_scoped(wide.as_ref(), "org-2", "proj-2"));
        assert!(Tenancy::admits_scoped(None, "org-2", "proj-2"));
    }

    // #634: a provider api_base pointed at instance metadata turns the gateway
    // into an SSRF primitive. link-local is denied by default; the private and
    // loopback ranges stay reachable because self-hosted upstreams live there
    #[test]
    fn egress_policy_denies_link_local_by_default() {
        let policy = EgressPolicy::default();
        let err = policy
            .check_url("https://169.254.169.254/latest/meta-data/", "api_base")
            .unwrap_err();
        assert!(err.contains("link-local"), "{err}");
        // ipv6 metadata address, and with a port
        assert!(policy
            .check_url("http://[fe80::1]:80/v1", "api_base")
            .is_err());
    }

    #[test]
    fn egress_policy_allows_self_hosted_upstreams_by_default() {
        let policy = EgressPolicy::default();
        // the air-gapped / on-prem cases rolter exists to serve
        for url in [
            "http://localhost:8000/v1",
            "http://127.0.0.1:8000/v1",
            "http://10.0.0.5:8000/v1",
            "http://192.168.1.10:8000/v1",
            "http://sim-a:8000/v1",
            "https://api.openai.com/v1",
        ] {
            assert!(
                policy.check_url(url, "api_base").is_ok(),
                "{url} was denied"
            );
        }
    }

    #[test]
    fn egress_policy_can_lock_down_private_and_loopback() {
        let policy = EgressPolicy {
            block_private: true,
            block_loopback: true,
            ..Default::default()
        };
        assert!(policy
            .check_url("http://10.0.0.5:8000/v1", "api_base")
            .is_err());
        assert!(policy
            .check_url("http://127.0.0.1:8000/v1", "api_base")
            .is_err());
        assert!(policy.check_url("http://[fc00::1]/v1", "api_base").is_err());
        // a public upstream is still fine
        assert!(policy
            .check_url("https://api.openai.com/v1", "api_base")
            .is_ok());
    }

    #[test]
    fn egress_allow_hosts_is_an_explicit_escape_hatch() {
        let policy = EgressPolicy {
            allow_hosts: vec!["169.254.169.254".to_string()],
            ..Default::default()
        };
        assert!(policy
            .check_url("https://169.254.169.254/v1", "api_base")
            .is_ok());
    }

    // #1953: the http client reads a url with the WHATWG parser, which turns
    // every one of these into 169.254.169.254, so the check has to classify
    // the same address or it guards a host the request never goes to
    #[test]
    fn egress_policy_denies_every_spelling_of_the_metadata_address() {
        let policy = EgressPolicy::default();
        for url in [
            // one decimal number, hex, a 16-bit last part, octal
            "http://2852039166/latest/meta-data/",
            "http://0xa9fea9fe/latest/meta-data/",
            "http://0XA9FEA9FE/",
            "http://169.254.43518/",
            "http://0251.0376.0251.0376/",
            // a trailing dot, and percent-encoded digits
            "https://169.254.169.254./",
            "http://%31%36%39.254.169.254/",
            // ipv4-mapped ipv6, dialled as the ipv4 address it carries
            "http://[::ffff:169.254.169.254]/",
            "http://[::ffff:a9fe:a9fe]:80/",
            // a backslash ends the authority, so the host is before the '@'
            "https://169.254.169.254\\@example.com/",
            "HTTP://169.254.169.254/",
            // an egress proxy url: a socks scheme keeps its host opaque
            "socks5://2852039166:1080",
        ] {
            let err = policy.check_url(url, "api_base").expect_err(url);
            assert!(err.contains("link-local"), "{url}: {err}");
        }
    }

    #[test]
    fn egress_policy_denies_every_spelling_of_loopback_when_asked() {
        let policy = EgressPolicy {
            block_loopback: true,
            ..Default::default()
        };
        for url in [
            "http://2130706433/",
            "http://0x7f000001:4001/",
            "http://127.1/",
            "http://[::ffff:127.0.0.1]/",
            // connecting to the unspecified address reaches the local host
            "http://0.0.0.0:4001/",
            "http://0/",
            "http://[::]:4001/",
        ] {
            let err = policy.check_url(url, "api_base").expect_err(url);
            assert!(err.contains("loopback"), "{url}: {err}");
        }
        // the default policy keeps loopback reachable in every spelling
        let default = EgressPolicy::default();
        assert!(default
            .check_url("http://0.0.0.0:4001/", "api_base")
            .is_ok());
        assert!(default.check_url("http://2130706433/", "api_base").is_ok());
    }

    #[test]
    fn egress_policy_denies_every_spelling_of_a_private_address_when_asked() {
        let policy = EgressPolicy {
            block_private: true,
            ..Default::default()
        };
        for url in [
            "http://167772165/",
            "http://10.5/",
            "http://[::ffff:10.0.0.5]/",
        ] {
            let err = policy.check_url(url, "api_base").expect_err(url);
            assert!(err.contains("private"), "{url}: {err}");
        }
    }

    #[test]
    fn deny_reason_classifies_mapped_and_unspecified_addresses() {
        let policy = EgressPolicy {
            block_loopback: true,
            ..Default::default()
        };
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        assert!(policy.deny_reason(ip("::ffff:169.254.169.254")).is_some());
        assert_eq!(policy.deny_reason(ip("0.0.0.0")), Some("loopback"));
        assert_eq!(policy.deny_reason(ip("::")), Some("loopback"));
        assert_eq!(policy.deny_reason(ip("::ffff:127.0.0.1")), Some("loopback"));
        assert_eq!(policy.deny_reason(ip("8.8.8.8")), None);
    }

    #[test]
    fn egress_allow_hosts_matches_the_address_the_url_reaches() {
        let policy = EgressPolicy {
            allow_hosts: vec!["169.254.169.254".to_string(), "example.com".to_string()],
            ..Default::default()
        };
        // another spelling of an allowed address is the same destination
        assert!(policy.check_url("http://0xa9fea9fe/v1", "api_base").is_ok());
        // but an allowed name written after a backslash is not the host
        assert!(policy
            .check_url("http://169.254.170.2\\@example.com/", "api_base")
            .is_err());
    }

    #[test]
    fn egress_policy_leaves_hostnames_to_the_connect_time_check() {
        let policy = EgressPolicy {
            block_loopback: true,
            block_private: true,
            ..Default::default()
        };
        for url in [
            "http://localhost:8000/v1",
            "http://sim-a:8000/v1",
            "https://api.openai.com/v1",
            "socks5h://proxy.internal:1080",
        ] {
            assert!(policy.check_url(url, "api_base").is_ok(), "{url}");
        }
    }

    #[test]
    fn url_host_handles_ports_userinfo_and_ipv6() {
        assert_eq!(url_host("http://example.com/v1"), Some("example.com"));
        assert_eq!(url_host("http://example.com:8080/v1"), Some("example.com"));
        assert_eq!(url_host("http://user:pw@10.0.0.1:80/v1"), Some("10.0.0.1"));
        assert_eq!(url_host("http://[::1]:4000/v1"), Some("::1"));
        assert_eq!(url_host("https://169.254.169.254"), Some("169.254.169.254"));
    }

    #[test]
    fn validate_rejects_a_metadata_api_base() {
        let cfg: GatewayConfig = toml::from_str(
            r#"
            [[providers]]
            name = "evil"
            kind = "openai"
            api_base = "https://169.254.169.254/latest"

            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "evil"
            "#,
        )
        .unwrap();
        let problems = cfg.validate().unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("link-local")),
            "{problems:?}"
        );
    }

    #[test]
    fn validate_accepts_scope_covered_mcp_session() {
        let mut config = GatewayConfig::default();
        config.mcp_servers.push(McpServerConfig {
            id: "server-1".to_string(),
            org_id: "org-1".to_string(),
            slug: "docs".to_string(),
            url: "https://mcp.example.com".to_string(),
            transport: "streamable_http".to_string(),
            required_scopes: vec!["tools:read".to_string()],
            auth_kind: McpAuthKind::Oauth,
            ..Default::default()
        });
        config.mcp_oauth_sessions.push(McpOAuthSessionConfig {
            id: "session-1".to_string(),
            server_id: "server-1".to_string(),
            user_id: "user-1".to_string(),
            scopes: vec!["tools:read".to_string()],
            expires_at: Utc::now() + chrono::Duration::hours(1),
            access_token: "secret".to_string(),
        });
        assert!(config.validate().is_ok(), "{:?}", config.validate());
    }

    /// The credential travels in the snapshot as plaintext, so the one thing
    /// that must never happen is it reaching a log line because something
    /// formatted the config. `Debug` is how that would happen (#952).
    #[test]
    fn an_mcp_credential_is_redacted_in_debug_output() {
        let server = McpServerConfig {
            id: "server-1".to_string(),
            slug: "docs".to_string(),
            auth_kind: McpAuthKind::Bearer,
            credential: Some("super-secret-token".to_string()),
            ..Default::default()
        };
        let rendered = format!("{server:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "the credential leaked into Debug: {rendered}"
        );
        assert!(rendered.contains("REDACTED"), "{rendered}");
        // the absent case must be distinguishable from the redacted one, or a
        // server with no credential reads as one whose secret was withheld
        let open = McpServerConfig {
            slug: "open".to_string(),
            ..Default::default()
        };
        assert!(format!("{open:?}").contains("credential: None"));
    }

    /// A kind from a newer control plane must not deserialize to something
    /// this build would act on. It lands on `Unknown`, which the proxy refuses
    /// rather than treating as an open server (#952).
    #[test]
    fn an_unrecognised_mcp_auth_kind_deserializes_to_unknown() {
        let parsed: McpServerConfig = serde_json::from_str(
            r#"{"id":"s","org_id":"o","slug":"x","url":"https://e.example.com",
                "transport":"streamable_http","auth_kind":"mtls"}"#,
        )
        .expect("an unknown kind must not fail the whole snapshot");
        assert_eq!(parsed.auth_kind, McpAuthKind::Unknown);

        // and the four it does know still round-trip
        for (raw, expected) in [
            ("none", McpAuthKind::None),
            ("bearer", McpAuthKind::Bearer),
            ("header", McpAuthKind::Header),
            ("oauth", McpAuthKind::Oauth),
        ] {
            let json = format!(
                r#"{{"id":"s","org_id":"o","slug":"x","url":"https://e.example.com",
                     "transport":"streamable_http","auth_kind":"{raw}"}}"#
            );
            let parsed: McpServerConfig = serde_json::from_str(&json).expect(raw);
            assert_eq!(parsed.auth_kind, expected, "{raw}");
        }
    }

    /// A snapshot from a control plane that predates #952 carries no auth
    /// fields at all. It must still load — that is the forward/backward
    /// compatibility the config model promises.
    #[test]
    fn an_mcp_server_from_an_older_control_plane_still_deserializes() {
        let parsed: McpServerConfig = serde_json::from_str(
            r#"{"id":"s","org_id":"o","slug":"x","url":"https://e.example.com",
                "transport":"streamable_http","required_scopes":["tools:read"]}"#,
        )
        .expect("the pre-#952 shape must still load");
        assert_eq!(parsed.auth_kind, McpAuthKind::None);
        assert!(parsed.credential.is_none());
        assert!(parsed.request_timeout_ms.is_none());
    }

    #[test]
    fn validate_rejects_mcp_session_missing_required_scope() {
        let mut config = GatewayConfig::default();
        config.mcp_servers.push(McpServerConfig {
            id: "server-1".to_string(),
            org_id: "org-1".to_string(),
            slug: "docs".to_string(),
            url: "https://mcp.example.com".to_string(),
            transport: "streamable_http".to_string(),
            required_scopes: vec!["tools:execute".to_string()],
            auth_kind: McpAuthKind::Oauth,
            ..Default::default()
        });
        config.mcp_oauth_sessions.push(McpOAuthSessionConfig {
            id: "session-1".to_string(),
            server_id: "server-1".to_string(),
            user_id: "user-1".to_string(),
            scopes: vec!["tools:read".to_string()],
            expires_at: Utc::now() + chrono::Duration::hours(1),
            access_token: "secret".to_string(),
        });
        let problems = config.validate().unwrap_err();
        assert!(problems
            .iter()
            .any(|problem| problem.contains("required scopes")));
    }

    const METADATA: &str = "https://169.254.169.254/latest/meta-data/";

    fn egress_config() -> GatewayConfig {
        GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "vllm"
            kind = "openai"
            api_base = "https://api.openai.com/v1"
            api_key_env = "KEY"

            [[routes]]
            model = "m"
            [[routes.targets]]
            provider = "vllm"
            model = "m"
        "#,
        )
        .expect("a config")
    }

    fn plugin(slug: &str, endpoint: &str) -> crate::plugin_dispatch::PluginInstanceConfig {
        crate::plugin_dispatch::PluginInstanceConfig {
            slug: slug.to_string(),
            org_id: "org".to_string(),
            project_id: None,
            stage: crate::plugin_dispatch::PluginStage::PreUpstream,
            position: 0,
            failure_mode: crate::guardrail_webhook::FailureMode::FailOpen,
            endpoint: endpoint.to_string(),
            auth: None,
        }
    }

    /// #2383: every operator-written URL goes through the egress policy in a
    /// file config, and `validate` names the field.
    #[test]
    fn validate_refuses_a_denied_url_in_every_operator_written_field() {
        type Mutation = Box<dyn Fn(&mut GatewayConfig)>;
        let cases: Vec<(&str, Mutation)> = vec![
            (
                "guardrail_webhook.url",
                Box::new(|c| {
                    c.guardrail_webhook.enabled = true;
                    c.guardrail_webhook.url = METADATA.to_string();
                }),
            ),
            (
                "pii_sanitizer.url",
                Box::new(|c| {
                    c.pii_sanitizer.enabled = true;
                    c.pii_sanitizer.url = METADATA.to_string();
                }),
            ),
            (
                "pii_sanitizer.restore_url",
                Box::new(|c| {
                    c.pii_sanitizer.enabled = true;
                    c.pii_sanitizer.url = "https://presidio.example.com/s".to_string();
                    c.pii_sanitizer.restore_url = METADATA.to_string();
                }),
            ),
            (
                "pii_sanitizer.restore_stream_url",
                Box::new(|c| {
                    c.pii_sanitizer.enabled = true;
                    c.pii_sanitizer.url = "https://presidio.example.com/s".to_string();
                    c.pii_sanitizer.restore_stream_url = METADATA.to_string();
                }),
            ),
            (
                "plugin 'audit' endpoint",
                Box::new(|c| c.plugins.instances.push(plugin("audit", METADATA))),
            ),
            (
                "status_page_url",
                Box::new(|c| c.providers[0].status_page_url = Some(METADATA.to_string())),
            ),
            (
                "lmcache.endpoint",
                Box::new(|c| {
                    c.providers[0].lmcache = Some(LmCacheConfig {
                        endpoint: METADATA.to_string(),
                        refresh_secs: 2,
                        stale_secs: 10,
                    })
                }),
            ),
            (
                "kv_events.endpoint",
                Box::new(|c| {
                    c.providers[0].kv_events = Some(KvEventsConfig {
                        endpoint: "tcp://169.254.169.254:5557".to_string(),
                        topic: "kv-events".to_string(),
                        max_blocks: 10,
                        stale_secs: 30,
                    })
                }),
            ),
        ];
        for (field, mutate) in cases {
            let mut config = egress_config();
            assert!(
                config.validate().is_ok(),
                "{field}: the base config is valid"
            );
            mutate(&mut config);
            let problems = config.validate().expect_err(field);
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains(field) && p.contains("egress policy")),
                "{field}: no problem names it"
            );
        }
    }

    /// #2997: a denied or malformed telemetry setting costs the provider its
    /// cache telemetry, not its traffic.
    #[test]
    fn the_snapshot_clears_a_denied_telemetry_endpoint_and_keeps_the_provider() {
        let mut config = egress_config();
        config.providers[0].kv_events = Some(KvEventsConfig {
            endpoint: "tcp://169.254.169.254:5557".to_string(),
            topic: "kv-events".to_string(),
            max_blocks: 10,
            stale_secs: 30,
        });
        config.providers[0].lmcache = Some(LmCacheConfig {
            endpoint: METADATA.to_string(),
            refresh_secs: 2,
            stale_secs: 10,
        });
        // file configs stay strict
        assert!(config.validate().is_err());

        let warnings = config.sanitize_for_snapshot();

        assert_eq!(config.providers.len(), 1, "the provider keeps serving");
        assert!(config.providers[0].kv_events.is_none());
        assert!(config.providers[0].lmcache.is_none());
        assert!(config.validate_snapshot().is_ok());
        assert!(config.validate().is_ok(), "nothing denied is left");
        for needle in ["kv_events", "lmcache"] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning names {needle}: {warnings:?}"
            );
        }
    }

    #[test]
    fn the_snapshot_keeps_an_allowed_telemetry_setting() {
        let mut config = egress_config();
        config.providers[0].kv_events = Some(KvEventsConfig {
            endpoint: "tcp://vllm-0.example.com:5557".to_string(),
            topic: "kv-events".to_string(),
            max_blocks: 10,
            stale_secs: 30,
        });
        let warnings = config.sanitize_for_snapshot();
        assert!(config.providers[0].kv_events.is_some());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn a_disabled_webhook_or_sanitizer_is_not_checked_for_egress() {
        let mut config = egress_config();
        config.guardrail_webhook.url = METADATA.to_string();
        config.pii_sanitizer.url = METADATA.to_string();
        assert!(config.validate().is_ok());
    }

    /// One denied endpoint must not withhold the snapshot from every tenant:
    /// the row is pruned with a problem line instead.
    #[test]
    fn the_snapshot_keeps_a_guard_with_a_denied_url_and_reports_it() {
        let mut config = egress_config();
        config.guardrail_webhook.enabled = true;
        config.guardrail_webhook.url = METADATA.to_string();
        config.pii_sanitizer.enabled = true;
        config.pii_sanitizer.url = "https://presidio.example.com/s".to_string();
        config.pii_sanitizer.restore_url = METADATA.to_string();
        config.plugins.instances.push(plugin("bad", METADATA));
        config
            .plugins
            .instances
            .push(plugin("good", "https://plugins.example.com/hook"));
        config.providers[0].status_page_url = Some(METADATA.to_string());

        let warnings = config.sanitize_for_snapshot();

        assert!(
            config.validate_snapshot().is_ok(),
            "a kept guard must not withhold the snapshot"
        );
        // file configs stay strict
        assert!(config.validate().is_err());
        // kept, not switched off: a fail-closed guard must not turn fail-open
        assert!(config.guardrail_webhook.enabled);
        assert!(config.pii_sanitizer.enabled);
        assert_eq!(config.plugins.instances.len(), 2);
        // the optional status page is cleared, the provider keeps serving (#3005)
        assert_eq!(config.providers.len(), 1);
        assert!(config.providers[0].status_page_url.is_none());
        for needle in [
            "guardrail_webhook.url",
            "pii_sanitizer.restore_url",
            "plugin 'bad'",
            "status_page_url",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(needle)),
                "no warning names {needle}"
            );
        }
    }

    #[test]
    fn project_scoped_warnings_name_each_flagged_entry() {
        let cfg = GatewayConfig::from_toml_str(
            r#"
            [[providers]]
            name = "scoped"
            kind = "openai"
            api_base = "https://api.example.com/v1"
            api_key_env = "K"
            project_scoped = true

            [[providers]]
            name = "shared"
            kind = "openai"
            api_base = "https://api.example.com/v1"
            api_key_env = "K"

            [[provider_groups]]
            name = "scoped-group"
            project_scoped = true
            "#,
        )
        .expect("parses");
        let warnings = cfg.project_scoped_warnings();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("provider 'scoped'"));
        assert!(warnings[1].contains("provider group 'scoped-group'"));
        assert!(warnings.iter().all(|w| w.contains("rolter-seed --import")));
    }

    const MIXED_ROUTE: &str = r#"
        [[providers]]
        name = "decider"
        kind = "system_one"
        api_base = "http://decider.internal:8080"

        [[providers]]
        name = "chat"
        kind = "openai"
        api_base = "https://api.example.com/v1"
        api_key_env = "K"

        [[providers]]
        name = "other-decider"
        kind = "system_one"
        api_base = "http://decider-2.internal:8080"

        [[routes]]
        model = "mixed"
        targets = [{ provider = "decider" }, { provider = "chat" }]

        [[routes]]
        model = "decisions"
        targets = [{ provider = "decider" }, { provider = "other-decider" }]

        [[routes]]
        model = "chatty"
        targets = [{ provider = "chat" }]
    "#;

    #[test]
    fn a_route_mixing_decision_and_generative_targets_is_named() {
        let cfg = GatewayConfig::from_toml_str(MIXED_ROUTE).expect("parses");
        let warnings = cfg.mixed_decision_route_warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("route 'mixed' mixes"),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("decision-only targets (decider)"));
        assert!(warnings[0].contains("not (chat)"));
    }

    #[test]
    fn a_mixed_route_is_reported_by_the_snapshot_but_still_served() {
        let mut cfg = GatewayConfig::from_toml_str(MIXED_ROUTE).expect("parses");
        let report = cfg.sanitize_for_snapshot_report();
        assert!(
            report
                .problems
                .iter()
                .any(|p| p.starts_with("route 'mixed' mixes")),
            "{:?}",
            report.problems
        );
        assert!(cfg.routes.iter().any(|r| r.model == "mixed"));
        assert!(report.omitted.is_empty(), "{:?}", report.omitted);
    }

    #[test]
    fn project_scoped_warnings_are_empty_when_unset() {
        let cfg = GatewayConfig::from_toml_str(
            "[[providers]]\nname = \"shared\"\nkind = \"openai\"\napi_base = \"https://api.example.com/v1\"\napi_key_env = \"K\"\n",
        )
        .expect("parses");
        assert!(cfg.project_scoped_warnings().is_empty());
    }

    // --- enum values from a newer release (#3347) ---

    /// A snapshot carrying a value of every fallback enum that this build does
    /// not know: as JSON from a newer control plane would.
    fn snapshot_from_the_future() -> serde_json::Value {
        serde_json::json!({
            "unpriced_policy": "quarterly_review",
            "queue": {"backpressure": "spill_to_disk"},
            "providers": [
                {"name": "known", "kind": "openai", "api_base": "https://api.openai.com",
                 "role_profile": "telepathic", "model_role_profiles": {"m": "telepathic"}},
                {"name": "future", "kind": "quantum", "api_base": "https://q.example"}
            ],
            "provider_groups": [
                {"name": "pool", "strategy": "mind_reading",
                 "members": [{"provider": "known"}]}
            ],
            "routes": [
                {"model": "kept", "targets": [
                    {"provider": "known"},
                    {"provider": "known", "takes_over": ["quantum_flux"]}
                 ],
                 "param_policy": {"mode": "maybe"}},
                {"model": "orphaned", "targets": [{"provider": "future"}]},
                {"model": "guessing", "strategy": "mind_reading",
                 "targets": [{"provider": "known"}]}
            ],
            "db_virtual_keys": [{"key_hash": "h", "id": "vk-1", "org_id": "org-1"}],
            "budgets": [
                {"scope": "galaxy", "id": "g", "limit_usd": 5},
                {"scope": "org", "id": "org-1", "limit_usd": 5, "period": "fortnightly",
                 "unpriced_policy": "quarterly_review"}
            ],
            "rate_limits": [{"scope": "galaxy", "id": "g", "rpm": 10}],
            "guardrails": {
                "streaming_post_call": "sometimes",
                "rules": [{"name": "r", "builtin": "email", "action": "scold"}]
            },
            "guardrail_webhook": {"failure_mode": "fail_sideways"},
            "semantic_guards": {
                "providers": [{"name": "p", "url": "https://g.example",
                               "failure_mode": "fail_sideways"}],
                "policies": [{"name": "pol", "on_violation": "scold", "streaming": "trickle"}]
            },
            "pii_sanitizer": {
                "direction": "sideways", "restoration": "always",
                "streaming": "trickle", "failure_mode": "fail_sideways"
            },
            "mcp_servers": [{"id": "m1", "org_id": "org-1", "slug": "tools",
                             "url": "https://mcp.example", "transport": "streamable_http",
                             "auth_kind": "telepathy"}],
            "plugins": {"instances": [
                {"slug": "pl", "org_id": "org-1", "stage": "pre_route", "position": 0,
                 "failure_mode": "fail_sideways", "endpoint": "https://pl.example"}
            ]}
        })
    }

    /// Each value read fail-closed is counted under its kind, a fixed name
    /// that never carries the value or the entity (#3424).
    #[test]
    fn unknown_values_are_counted_by_kind_and_never_by_value() {
        let mut cfg: GatewayConfig = serde_json::from_value(snapshot_from_the_future())
            .expect("a snapshot from a newer release decodes");
        let report = cfg.fail_closed_unknown_report();
        let count = |kind: &str| report.kinds.get(kind).copied().unwrap_or(0);
        assert_eq!(count("provider_kind"), 1, "{:?}", report.kinds);
        assert_eq!(count("route_strategy"), 1, "{:?}", report.kinds);
        assert_eq!(count("provider_group_strategy"), 1, "{:?}", report.kinds);
        assert_eq!(count("budget_scope"), 1, "{:?}", report.kinds);
        assert_eq!(count("budget_period"), 1, "{:?}", report.kinds);
        assert_eq!(count("rate_limit_scope"), 1, "{:?}", report.kinds);
        assert_eq!(count("failure_class"), 1, "{:?}", report.kinds);
        assert_eq!(count("unpriced_policy"), 2, "{:?}", report.kinds);
        assert_eq!(count("role_profile"), 2, "{:?}", report.kinds);
        assert_eq!(count("failure_mode"), 4, "{:?}", report.kinds);
        assert_eq!(count("mcp_auth_kind"), 1, "{:?}", report.kinds);
        for kind in report.kinds.keys() {
            assert!(
                kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{kind}"
            );
            for value in ["quantum", "galaxy", "fortnightly", "future", "pool"] {
                assert!(!kind.contains(value), "{kind}");
            }
        }
        // every value is a line too, and the sanitizer's cascade adds more
        let values: u32 = report.kinds.values().sum();
        assert!(report.lines.len() >= values as usize, "{report:?}");
        // a snapshot whose values are all known counts nothing
        let mut known = GatewayConfig::default();
        assert_eq!(known.fail_closed_unknown_report(), UnknownValues::default());
    }

    #[test]
    fn unknown_enum_values_fail_closed_for_their_entity_and_the_rest_applies() {
        use crate::guardrail_webhook::FailureMode;
        use crate::guardrails::{GuardAction, StreamingPostCall};
        use crate::pii_sanitizer::{RestorationPolicy, SanitizeDirection, StreamingResponse};
        use crate::semantic_guards::{StreamingStrategy, ViolationAction};

        let mut cfg: GatewayConfig = serde_json::from_value(snapshot_from_the_future())
            .expect("a value this build does not know must not fail the decode");
        let problems = cfg.fail_closed_unknown_values();

        // the provider of an unknown kind is never dialled, and the route that
        // only reached it goes with it; the route of an unknown strategy refuses
        let providers: Vec<&str> = cfg.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(providers, ["known"]);
        let routes: Vec<&str> = cfg.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(routes, ["kept"]);
        assert!(cfg.provider_groups[0].members.is_empty());
        // the rest of the snapshot, the keys above all, still applies
        assert_eq!(cfg.db_virtual_keys.len(), 1);
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );

        // settings take their most restrictive known reading
        assert_eq!(cfg.unpriced_policy, UnpricedPolicy::Block);
        assert_eq!(cfg.queue.backpressure, BackpressurePolicy::Error);
        assert_eq!(cfg.providers[0].role_profile, Some(RoleProfile::SystemOnly));
        assert_eq!(
            cfg.providers[0].model_role_profiles["m"],
            RoleProfile::SystemOnly
        );
        assert_eq!(cfg.routes[0].param_policy.mode, OverrideMode::Deny);
        // an unknown class stays in the set, matching nothing (#3390)
        assert_eq!(
            cfg.routes[0].targets[1].takes_over[0].kind,
            crate::failover::FailureKind::Unknown
        );
        assert_eq!(cfg.budgets[1].unpriced_policy, Some(UnpricedPolicy::Block));
        assert_eq!(cfg.guardrails.rules[0].action, GuardAction::Block);
        assert_eq!(
            cfg.guardrails.streaming_post_call,
            StreamingPostCall::Reject
        );
        assert_eq!(cfg.guardrail_webhook.failure_mode, FailureMode::FailClosed);
        assert_eq!(
            cfg.semantic_guards.providers[0].failure_mode,
            FailureMode::FailClosed
        );
        let policy = &cfg.semantic_guards.policies[0];
        assert_eq!(policy.on_violation, ViolationAction::Block);
        assert_eq!(policy.streaming, StreamingStrategy::Reject);
        assert_eq!(cfg.pii_sanitizer.direction, SanitizeDirection::Both);
        assert_eq!(cfg.pii_sanitizer.restoration, RestorationPolicy::Never);
        assert_eq!(cfg.pii_sanitizer.streaming, StreamingResponse::Reject);
        assert_eq!(cfg.pii_sanitizer.failure_mode, FailureMode::FailClosed);
        assert_eq!(
            cfg.plugins.instances[0].failure_mode,
            FailureMode::FailClosed
        );

        // budgets and limits stay unknown: the gateway's enforcement binds them
        assert_eq!(cfg.budgets[0].scope, BudgetScope::Unknown);
        assert_eq!(cfg.budgets[1].period, BudgetPeriod::Unknown);
        assert_eq!(cfg.rate_limits[0].scope, BudgetScope::Unknown);

        for named in [
            "future",
            "guessing",
            "pool",
            "unpriced_policy",
            "plugin 'pl'",
            "quantum_flux",
        ] {
            assert!(
                problems.iter().any(|p| p.contains(named)),
                "no problem names {named}: {problems:?}"
            );
        }
    }

    #[test]
    fn a_snapshot_with_only_known_values_is_left_alone() {
        let mut cfg: GatewayConfig = serde_json::from_value(serde_json::json!({
            "providers": [{"name": "p", "kind": "openai", "api_base": "https://a.example"}],
            "routes": [{"model": "m", "targets": [{"provider": "p"}]}]
        }))
        .unwrap();
        assert!(cfg.fail_closed_unknown_values().is_empty());
        assert_eq!(cfg.routes.len(), 1);
    }

    #[test]
    fn a_config_file_refuses_a_value_this_build_does_not_know() {
        for (toml, named) in [
            (
                "[[providers]]\nname = \"p\"\nkind = \"opneai\"\napi_base = \"https://a.example\"\n",
                "provider 'p' kind",
            ),
            (
                "[[routes]]\nmodel = \"m\"\nstrategy = \"round_robbin\"\ntargets = []\n",
                "route 'm' strategy",
            ),
            ("unpriced_policy = \"blok\"\n", "unpriced_policy"),
            (
                "[[budgets]]\nscope = \"org\"\nid = \"o\"\nlimit_usd = 1\nperiod = \"weekly\"\n",
                "budget 'o' period",
            ),
            (
                "[guardrail_webhook]\nfailure_mode = \"fail_closd\"\n",
                "guardrail_webhook.failure_mode",
            ),
            (
                "[[routes]]\nmodel = \"m\"\n[[routes.targets]]\nprovider = \"p\"\n\
                 takes_over = [\"throtled\"]\n",
                "takes_over 'throtled'",
            ),
        ] {
            let err = GatewayConfig::from_toml_str(toml)
                .err()
                .unwrap_or_else(|| panic!("{named}: a typo must not load"));
            assert!(err.to_string().contains(named), "{named}: {err}");
            // a typo is answered with what the build accepts
            assert!(err.to_string().contains("expected one of"), "{named}: {err}");
        }
        let err = GatewayConfig::from_toml_str("unpriced_policy = \"blok\"\n")
            .err()
            .unwrap();
        assert!(err.to_string().contains("ignore, warn, block"), "{err}");
    }

    #[test]
    fn an_unknown_unpriced_policy_outranks_block() {
        assert!(UnpricedPolicy::Unknown > UnpricedPolicy::Block);
    }

    #[test]
    fn an_unknown_guard_provider_kind_is_named_and_left_to_fail_closed() {
        use crate::guardrail_webhook::GuardProviderKind;
        let mut cfg: GatewayConfig = serde_json::from_value(serde_json::json!({
            "semantic_guards": {"providers": [
                {"name": "future", "enabled": true, "kind": "quantum", "failure_mode": "fail_open"}
            ]}
        }))
        .expect("an unknown kind must not fail the decode");
        let problems = cfg.fail_closed_unknown_values().join("\n");
        assert!(
            problems.contains("semantic guard provider 'future' kind"),
            "{problems}"
        );
        // kept as it is: the gateway blocks its stage per request
        assert_eq!(
            cfg.semantic_guards.providers[0].kind,
            GuardProviderKind::Unknown
        );
        assert!(
            cfg.validate_snapshot().is_ok(),
            "{:?}",
            cfg.validate_snapshot()
        );

        // a file is written by hand: the typo is refused with the list
        let err = GatewayConfig::from_toml_str(
            "[[semantic_guards.providers]]\nname = \"x\"\nkind = \"locl\"\n",
        )
        .err()
        .unwrap_or_else(|| panic!("a typo must not load"));
        assert!(err.to_string().contains("webhook, local"), "{err}");
    }
}
