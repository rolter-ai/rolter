use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::Result;

/// Root bootstrap configuration loaded from a TOML file or the database.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GatewayConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub routes: Vec<ModelRoute>,
    #[serde(default)]
    pub virtual_keys: Vec<VirtualKeyConfig>,
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// Listener configuration for a rolter process.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    4000
}

/// The wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// native openai chat/completions api
    Openai,
    /// native anthropic messages api
    Anthropic,
    /// any openai-compatible endpoint such as vllm, tgi or ollama
    OpenaiCompatible,
}

/// An upstream provider rolter can forward to.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub name: String,
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
}

impl ProviderConfig {
    /// Resolve the effective api key, preferring the inline value then the env var.
    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(k) = &self.api_key {
            return Some(k.clone());
        }
        self.api_key_env
            .as_ref()
            .and_then(|e| std::env::var(e).ok())
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
    pub targets: Vec<Target>,
}

/// A virtual api key that clients present to the gateway.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VirtualKeyConfig {
    pub key: String,
    #[serde(default)]
    pub name: Option<String>,
    /// allowed public model names; empty means all models are allowed
    #[serde(default)]
    pub models: Vec<String>,
}

/// Where request and cost logs are written.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub clickhouse_url: Option<String>,
}

impl GatewayConfig {
    /// Parse a configuration from a TOML string.
    pub fn from_toml_str(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }

    /// Load a configuration from a TOML file on disk.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Self::from_toml_str(&raw)
    }

    /// Find a provider by name.
    pub fn resolve_provider(&self, name: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Validate internal consistency: every route target must reference a
    /// known provider, names must be unique and target weights positive.
    /// Returns every problem found, so callers can log/report them all.
    pub fn validate(&self) -> std::result::Result<(), Vec<String>> {
        let mut problems = Vec::new();

        let mut provider_names = std::collections::HashSet::new();
        for provider in &self.providers {
            if !provider_names.insert(provider.name.as_str()) {
                problems.push(format!("duplicate provider name '{}'", provider.name));
            }
        }

        let mut route_models = std::collections::HashSet::new();
        for route in &self.routes {
            if !route_models.insert(route.model.as_str()) {
                problems.push(format!("duplicate route model '{}'", route.model));
            }
            for target in &route.targets {
                if !provider_names.contains(target.provider.as_str()) {
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
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
