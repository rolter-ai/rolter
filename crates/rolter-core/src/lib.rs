//! Shared building blocks for the rolter gateway and control plane.
//!
//! This crate holds the configuration model, domain error type and telemetry
//! bootstrap that every other rolter crate depends on.

pub mod config;
pub mod error;
pub mod telemetry;

pub use config::{
    ApiKeyConfig, BalancingStrategy, BreakerConfig, BudgetConfig, BudgetPeriod, BudgetScope,
    CacheConfig, CooldownConfig, GatewayConfig, HealthConfig, LoggingConfig, MetricsScrapeConfig,
    ModelPriceConfig, ModelRoute, OverrideMode, ParamPolicy, ProviderConfig, ProviderKind,
    RateLimitConfig, RetryConfig, RouteCache, ServerConfig, Target, TimeoutConfig, Variant,
    VirtualKeyConfig, VirtualKeyRecord, RESERVED_PATHS,
};
pub use error::{Error, Result};

/// Redis pub/sub channel the control plane publishes config-version bumps
/// on; gateways subscribe to it to trigger an immediate snapshot poll.
pub const CONFIG_CHANNEL: &str = "rolter.config";
