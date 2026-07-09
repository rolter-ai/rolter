//! Shared building blocks for the rolter gateway and control plane.
//!
//! This crate holds the configuration model, domain error type and telemetry
//! bootstrap that every other rolter crate depends on.

pub mod config;
pub mod error;
pub mod telemetry;

pub use config::{
    BalancingStrategy, BreakerConfig, BudgetConfig, BudgetPeriod, BudgetScope, CooldownConfig,
    GatewayConfig, HealthConfig, LoggingConfig, ModelPriceConfig, ModelRoute, ProviderConfig,
    ProviderKind, RateLimitConfig, RetryConfig, ServerConfig, Target, TimeoutConfig,
    VirtualKeyConfig, VirtualKeyRecord,
};
pub use error::{Error, Result};

/// Redis pub/sub channel the control plane publishes config-version bumps
/// on; gateways subscribe to it to trigger an immediate snapshot poll.
pub const CONFIG_CHANNEL: &str = "rolter.config";
