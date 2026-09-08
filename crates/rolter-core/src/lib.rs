//! Shared building blocks for the rolter gateway and control plane.
//!
//! This crate holds the configuration model, domain error type and telemetry
//! bootstrap that every other rolter crate depends on.

pub mod access_policy;
pub mod config;
pub mod currency;
pub mod error;
pub mod guardrail_webhook;
pub mod guardrails;
pub mod pii_sanitizer;
pub mod plugin_dispatch;
pub mod probe;
pub mod prompt_templates;
pub mod slug;
pub mod stability;
pub mod telemetry;

pub use access_policy::ModelPolicy;
pub use config::{
    AdaptiveRoutingConfig, AdvancedModelConfig, ApiKeyConfig, BackpressurePolicy,
    BalancingStrategy, BreakerConfig, BudgetConfig, BudgetPeriod, BudgetScope, CacheConfig,
    ClientConfig, CompatibilityConfig, CooldownConfig, EgressPolicy, FeatureFlagsConfig,
    GatewayConfig, GroupMember, HealthConfig, KvEventsConfig, LmCacheConfig, LoggingConfig,
    McpOAuthSessionConfig, McpServerConfig, MetricsScrapeConfig, ModelDefaultsConfig, ModelLimits,
    ModelPriceConfig, ModelRoute, ModelUsagePricing, ModelVisibility, OverrideMode, ParamPolicy,
    PayloadCaptureConfig, ProviderConfig, ProviderGroupConfig, ProviderKind, QueueConfig,
    RateLimitConfig, RealtimeConfig, ResponsesConfig, RetryConfig, RoleProfile, RouteCache,
    SecurityPolicyConfig, SemanticCacheConfig, ServerConfig, Target, TimeoutConfig, TlsConfig,
    UnpricedPolicy, UsageRecordingConfig, Variant, VirtualKeyConfig, VirtualKeyRecord,
    MAX_EXPLORATION_RATIO, MCP_TRANSPORTS, RESERVED_PATHS,
};
pub use currency::{CurrencyConfig, CurrencyConverter, StaticRates, DEFAULT_BASE_CURRENCY};
pub use error::{Error, Result};
pub use guardrail_webhook::{
    FailureMode, GuardrailWebhookConfig, WebhookAuth, WebhookDecision, WebhookRequest,
    WebhookStage, WebhookTenant,
};
pub use guardrails::{
    BuiltinRule, CompiledGuardrails, GuardAction, GuardStage, GuardrailReport, GuardrailRule,
    GuardrailsConfig, ScanOutcome, StreamingPostCall,
};
pub use pii_sanitizer::{
    Finding, PiiSanitizerConfig, RestorationPolicy, RestorationTicket, RestoreRequest,
    RestoreResponse, SanitizeDirection, SanitizeRequest, SanitizeResponse, StreamingResponse,
    TokenScope,
};
pub use plugin_dispatch::{PluginInstanceConfig, PluginRequest, PluginStage, PluginsConfig};
pub use probe::{
    count_catalogue, probe_expectation, probe_request, ProbeExpectation, ANTHROPIC_VERSION,
};
pub use prompt_templates::{
    CompiledTemplates, Decorator, DecoratorPosition, DecoratorRole, PromptTemplate,
    PromptTemplateActivationScope, PromptTemplateRequestScope, PromptTemplatesConfig, RenderError,
    RenderedMessage, TemplateReport, TemplateVariable, TEMPLATE_VARS_FIELD,
};
pub use stability::{Stability, SubsystemStability, SUBSYSTEMS};

/// Redis pub/sub channel the control plane publishes config-version bumps
/// on; gateways subscribe to it to trigger an immediate snapshot poll.
pub const CONFIG_CHANNEL: &str = "rolter.config";
