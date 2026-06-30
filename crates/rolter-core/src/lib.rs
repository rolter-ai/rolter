//! Shared building blocks for the rolter gateway and control plane.
//!
//! This crate holds the configuration model, domain error type and telemetry
//! bootstrap that every other rolter crate depends on.

pub mod config;
pub mod error;
pub mod telemetry;

pub use config::{
    BalancingStrategy, GatewayConfig, LoggingConfig, ModelRoute, ProviderConfig, ProviderKind,
    ServerConfig, Target, VirtualKeyConfig,
};
pub use error::{Error, Result};
