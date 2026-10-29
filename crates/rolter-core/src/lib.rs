//! Shared building blocks for the rolter gateway and control plane.
//!
//! This crate holds the configuration model, domain error type and telemetry
//! bootstrap that every other rolter crate depends on.
//!
//! **Internal crate.** It is published only so `cargo install rolter` can
//! resolve, and it offers no stable Rust API: any public item here may change
//! or disappear in any release, including a patch release. Build against
//! rolter's HTTP surfaces instead — see
//! [ADR-0032](https://github.com/rolter-ai/rolter/blob/master/docs/dev-docs/adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

pub mod access_policy;
pub mod admin_token;
#[cfg(feature = "cli-guard")]
pub mod cli_guard;
pub mod config;
pub mod config_lint;
pub mod config_migrate;
pub mod currency;
pub mod error;
pub mod failover;
pub mod guardrail_webhook;
pub mod guardrails;
pub mod mcp_challenge;
pub mod mcp_log;
pub mod namespace;
pub mod node_identity;
pub mod payload_redaction;
pub mod pii_sanitizer;
pub mod plugin_dispatch;
pub mod probe;
pub mod prompt_templates;
pub mod redact;
pub mod request_outcome;
pub mod secret_env;
pub mod semantic_guards;
#[cfg(feature = "shutdown")]
pub mod shutdown;
pub mod slug;
pub mod snapshot_format;
pub mod spec_registry;
pub mod stability;
pub mod telemetry;
pub mod upstream;

pub use access_policy::ModelPolicy;
pub use config::{
    is_reserved_route_header, request_id_header_problem, request_id_refused_headers,
    strip_retired_advanced_keys, DEFAULT_REQUEST_ID_HEADER,
};
pub use config::{
    AdaptiveRoutingConfig, AdvancedModelConfig, ApiKeyConfig, AudioTokens, BackpressurePolicy,
    BalancingStrategy, BilledUnits, BreakerConfig, BudgetConfig, BudgetPeriod, BudgetScope,
    CacheConfig, ClientConfig, CompatibilityConfig, CooldownConfig, EgressPolicy,
    FeatureFlagsConfig, GatewayConfig, GroupMember, HealthConfig, KvEventsConfig, LmCacheConfig,
    LoggingConfig, McpAuthKind, McpGatewayKeyRefConfig, McpGatewaySettingsConfig,
    McpGatewayToolGroupConfig, McpGatewayToolRefConfig, McpManifestEnforcementConfig,
    McpOAuthSessionConfig, McpOrgSettingsConfig, McpServerConfig, McpToolGroupConfig,
    McpToolGroupSubjectConfig, McpToolRefConfig, MetricsScrapeConfig, ModelDefaultsConfig,
    ModelLimits, ModelPriceConfig, ModelRoute, ModelVisibility, OverrideMode, ParamPolicy,
    PayloadCaptureConfig, PreviousKeySecret, ProviderConfig, ProviderGroupConfig, ProviderKind,
    QueueConfig, RateLimitConfig, RealtimeConfig, ResponsesConfig, RetryConfig, RoleProfile,
    RouteCache, ScopeWithin, SecurityPolicyConfig, SemanticCacheConfig, ServerConfig, Target,
    Tenancy, TimeoutConfig, TlsConfig, UnpricedPolicy, UsageRecordingConfig, Variant,
    VirtualKeyConfig, VirtualKeyRecord, MAX_EXPLORATION_RATIO, MAX_ROUTE_RETRIES,
    MAX_ROUTE_THROUGHPUT, MCP_TRANSPORTS, PUBLIC_EXAMPLE_KEY, RESERVED_PATHS,
    RESERVED_ROUTE_HEADERS,
};
pub use config::{OmittedEntry, OmittedKind, SnapshotReport, UnknownValues};
pub use config_lint::{unknown_keys, UnknownKey};
pub use config_migrate::{
    migrate as migrate_config, plan as plan_config_migration, Change as ConfigChange,
    Migration as ConfigMigration, Report as ConfigMigrationReport, Step as ConfigMigrationStep,
    CURRENT_SCHEMA_VERSION,
};
pub use currency::{CurrencyConfig, CurrencyConverter, StaticRates, DEFAULT_BASE_CURRENCY};
pub use error::{Error, Result};
pub use guardrail_webhook::{
    FailureMode, GuardProviderKind, GuardrailWebhookConfig, WebhookAuth, WebhookDecision,
    WebhookRequest, WebhookStage, WebhookTenant,
};
pub use guardrails::{
    dry_run_policy, dry_run_rule, BuiltinRule, CompiledGuardrails, DryRun, DryRunOutcome,
    DryRunSkip, DryRunSpan, GuardAction, GuardStage, GuardrailReport, GuardrailRule,
    GuardrailsConfig, PolicyRun, PolicyStep, ScanOutcome, StreamingPostCall, MAX_DRY_RUN_SPANS,
};
pub use node_identity::{node_id, node_identity, NodeIdSource, NodeIdentity};
pub use pii_sanitizer::{
    Finding, PiiSanitizerConfig, RestorationPolicy, RestorationTicket, RestoreRequest,
    RestoreResponse, SanitizeDirection, SanitizeRequest, SanitizeResponse, StreamingResponse,
    TokenScope,
};
pub use plugin_dispatch::{
    PluginInstanceConfig, PluginRequest, PluginStage, PluginsConfig, PLUGIN_CONTRACT_VERSION,
};
pub use probe::{
    count_catalogue, probe_expectation, probe_request, ProbeExpectation, ANTHROPIC_VERSION,
};
pub use prompt_templates::{
    CompiledTemplates, Decorator, DecoratorPosition, DecoratorRole, PromptTemplate,
    PromptTemplateActivationScope, PromptTemplateRequestScope, PromptTemplatesConfig, RenderError,
    RenderedMessage, TemplateReport, TemplateVariable, TEMPLATE_VARS_FIELD,
};
pub use secret_env::{credential_from_env, is_control_plane_secret, is_secret_unless_allowed};
pub use semantic_guards::{
    CompiledPolicy, CompiledSemanticGuards, GuardrailPolicy, PolicyRequest, PolicyScope,
    ScoredDecision, SemanticGuardsConfig, StreamingStrategy, ViolationAction,
};
pub use stability::{
    stability_of, stability_of_nav_key, subsystem, Stability, SubsystemStability, SUBSYSTEMS,
};

/// Redis pub/sub channel the control plane publishes config-version bumps
/// on; gateways subscribe to it to trigger an immediate snapshot poll.
pub const CONFIG_CHANNEL: &str = "rolter.config";

/// The model id of the gateway's built-in deterministic responder.
///
/// The gateway answers it locally unless a route of the same name shadows it
/// for the caller. The control plane refuses a database route by this name, so
/// one tenant can never take the builtin away from the others; a route in a
/// config file may still shadow it, which is how an operator replaces it.
pub const FAKE_LLM_MODEL: &str = "fake-llm";
