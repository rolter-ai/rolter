use chrono::{DateTime, Utc};
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
    /// database-defined virtual keys, carried as peppered digests plus scope
    /// identity (never plaintext). composed from the store, not the toml
    #[serde(default)]
    pub db_virtual_keys: Vec<VirtualKeyRecord>,
    #[serde(default)]
    pub model_prices: Vec<ModelPriceConfig>,
    /// spend caps enforced by the gateway against Redis-tracked cumulative cost
    #[serde(default)]
    pub budgets: Vec<BudgetConfig>,
    /// request/token throughput caps enforced against a Redis sliding window
    #[serde(default)]
    pub rate_limits: Vec<RateLimitConfig>,
    /// upstream retry policy for transient failures (408/429/5xx, connect errors)
    #[serde(default)]
    pub retry: RetryConfig,
    /// per-target cooldown applied after a transient upstream failure
    #[serde(default)]
    pub cooldown: CooldownConfig,
    /// upstream connect/response timeouts
    #[serde(default)]
    pub timeouts: TimeoutConfig,
    /// active upstream health probing
    #[serde(default)]
    pub health: HealthConfig,
    /// per-target circuit breaker for sustained upstream failures
    #[serde(default)]
    pub breaker: BreakerConfig,
    /// background scrape of upstream engine `/metrics` for load-aware routing
    #[serde(default)]
    pub metrics_scrape: MetricsScrapeConfig,
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
    /// deployment-wide secret mixed into virtual-key digests. keeps plaintext
    /// keys out of gateway memory and makes a leaked digest useless without it.
    /// falls back to the `ROLTER_KEY_PEPPER` env var when unset (see
    /// [`ServerConfig::resolve_key_pepper`]).
    #[serde(default)]
    pub key_pepper: Option<String>,
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
    /// smooth weighted round-robin honouring each target's `weight`
    Weighted,
    /// composable filter → weighted-score → argmax pipeline (static weight +
    /// in-flight load + prefix-cache affinity scorers)
    Pipeline,
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
    /// administratively revoke the key without deleting it
    #[serde(default)]
    pub disabled: bool,
    /// optional expiry; the key stops authenticating at/after this instant
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

impl VirtualKeyConfig {
    /// Whether the key may authenticate at `now`: not disabled and not expired.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.disabled && self.expires_at.is_none_or(|exp| now < exp)
    }
}

/// A database-defined virtual key as seen by the gateway: the peppered digest
/// (never the plaintext) plus the scope identity used for log attribution,
/// budgets and rate limits.
#[derive(Debug, Clone, Deserialize, Serialize)]
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
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

impl VirtualKeyRecord {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.disabled && self.expires_at.is_none_or(|exp| now < exp)
    }
}

/// Per-model token pricing used to compute `cost_usd` for each request. Rates
/// are USD per million tokens (matching the `model_prices` catalog).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelPriceConfig {
    /// public model name this price applies to
    pub model: String,
    #[serde(default)]
    pub input_per_mtok: f64,
    #[serde(default)]
    pub output_per_mtok: f64,
    /// price for cache-hit input tokens; falls back to `input_per_mtok`
    #[serde(default)]
    pub cached_input_per_mtok: Option<f64>,
}

impl ModelPriceConfig {
    /// Compute request cost in USD from token counts. `cached_input` is the
    /// portion of `prompt` tokens served from cache (priced at the cached rate
    /// when set); pass 0 when unknown.
    pub fn cost_usd(&self, prompt: u32, completion: u32, cached_input: u32) -> f64 {
        let cached = cached_input.min(prompt);
        let fresh = prompt - cached;
        let cached_rate = self.cached_input_per_mtok.unwrap_or(self.input_per_mtok);
        (fresh as f64 * self.input_per_mtok
            + cached as f64 * cached_rate
            + completion as f64 * self.output_per_mtok)
            / 1_000_000.0
    }
}

/// The scope level a [`BudgetConfig`] applies to. Matched against the request's
/// virtual-key scope chain (org → team → project → key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BudgetScope {
    Org,
    Team,
    Project,
    Key,
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
}

impl BudgetPeriod {
    /// Identifier of the current window at `now`; part of the Redis spend key so
    /// a new window starts with a zero counter.
    pub fn bucket(&self, now: DateTime<Utc>) -> String {
        match self {
            BudgetPeriod::Daily => now.format("%Y%m%d").to_string(),
            BudgetPeriod::Monthly => now.format("%Y%m").to_string(),
            BudgetPeriod::Total => "all".to_string(),
        }
    }

    /// TTL for the Redis spend counter (generous, just for cleanup — the bucket
    /// key already partitions windows). `None` for [`BudgetPeriod::Total`].
    pub fn ttl_secs(&self) -> Option<u64> {
        match self {
            BudgetPeriod::Daily => Some(2 * 24 * 3600),
            BudgetPeriod::Monthly => Some(40 * 24 * 3600),
            BudgetPeriod::Total => None,
        }
    }
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
    /// spend cap in USD for the window
    pub limit_usd: f64,
    #[serde(default)]
    pub period: BudgetPeriod,
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
}

/// Upstream retry policy. On a transient upstream failure (HTTP 408/429/5xx or a
/// connection error) the gateway re-picks a target (excluding ones already tried,
/// so retries fail over to sibling targets when available) and forwards again,
/// up to `max_retries` extra attempts. Backoff is exponential with full jitter
/// between `base_backoff_ms` and `max_backoff_ms`; a 429 `Retry-After` header
/// overrides the computed delay. Retries only happen before any body bytes have
/// been streamed to the client, so no partial response is ever duplicated.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// Upstream timeout policy. `connect_secs` bounds establishing the TCP/TLS
/// connection; `request_secs` bounds time-to-response-headers (not the body), so
/// a hung upstream is abandoned without killing legitimately long SSE streams. A
/// timeout surfaces as a transient upstream error, so it feeds the retry and
/// cooldown machinery. Set a field to 0 to disable that timeout.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

fn default_connect_secs() -> u64 {
    10
}

fn default_request_secs() -> u64 {
    60
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
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_health_interval_secs(),
            timeout_secs: default_health_timeout_secs(),
            path: default_health_path(),
        }
    }
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            clickhouse_url: None,
            batch_max: default_log_batch_max(),
            flush_ms: default_log_flush_ms(),
            queue_capacity: default_log_queue_capacity(),
        }
    }
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

    /// Validate internal consistency and surface every problem at once so an
    /// operator can fix a whole config in one pass rather than one error per
    /// restart. Checks: unique/non-empty provider names, well-formed provider
    /// `api_base` and `egress_proxy` URLs, unique/non-empty route models, each
    /// route having at least one target that references a known provider with a
    /// positive weight, unique/non-empty virtual keys, positive budget limits and
    /// rate limits that actually cap something. Returns every problem found, so
    /// callers can log/report them all.
    pub fn validate(&self) -> std::result::Result<(), Vec<String>> {
        let mut problems = Vec::new();

        let mut provider_names = std::collections::HashSet::new();
        for provider in &self.providers {
            if provider.name.trim().is_empty() {
                problems.push("provider has an empty name".to_string());
            } else if !provider_names.insert(provider.name.as_str()) {
                problems.push(format!("duplicate provider name '{}'", provider.name));
            }
            if !is_http_url(&provider.api_base) {
                problems.push(format!(
                    "provider '{}' has an invalid api_base '{}' (expected http:// or https:// url)",
                    provider.name, provider.api_base
                ));
            }
            if let Some(proxy) = &provider.egress_proxy {
                if !is_proxy_url(proxy) {
                    problems.push(format!(
                        "provider '{}' has an invalid egress_proxy '{}' (expected http(s)/socks5(h) url)",
                        provider.name, proxy
                    ));
                }
            }
        }

        let mut route_models = std::collections::HashSet::new();
        for route in &self.routes {
            if route.model.trim().is_empty() {
                problems.push("route has an empty model name".to_string());
            } else if !route_models.insert(route.model.as_str()) {
                problems.push(format!("duplicate route model '{}'", route.model));
            }
            if route.targets.is_empty() {
                problems.push(format!("route '{}' has no targets", route.model));
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

        let mut key_values = std::collections::HashSet::new();
        for vk in &self.virtual_keys {
            if vk.key.trim().is_empty() {
                problems.push("virtual key has an empty key value".to_string());
            } else if !key_values.insert(vk.key.as_str()) {
                problems.push("duplicate virtual key value".to_string());
            }
        }

        for budget in &self.budgets {
            if budget.limit_usd <= 0.0 || budget.limit_usd.is_nan() {
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

        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

/// Whether `s` is a plausible `http`/`https` URL with a non-empty host.
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
        assert!(problems.iter().any(|p| p.contains("has no targets")));
        assert!(problems.iter().any(|p| p.contains("duplicate virtual key")));
        assert!(problems
            .iter()
            .any(|p| p.contains("non-positive limit_usd")));
        assert!(problems.iter().any(|p| p.contains("neither rpm nor tpm")));
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
    fn cost_usd_from_token_counts() {
        let price = ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: 2.5,
            output_per_mtok: 10.0,
            cached_input_per_mtok: None,
        };
        // 1000 * 2.5/1e6 + 500 * 10/1e6 = 0.0025 + 0.005 = 0.0075
        assert!((price.cost_usd(1000, 500, 0) - 0.0075).abs() < 1e-12);
    }

    #[test]
    fn cost_usd_applies_cached_rate() {
        let price = ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: 2.0,
            output_per_mtok: 0.0,
            cached_input_per_mtok: Some(0.5),
        };
        // 600 fresh * 2 + 400 cached * 0.5 = 1200 + 200 = 1400 / 1e6
        assert!((price.cost_usd(1000, 0, 400) - 0.0014).abs() < 1e-12);
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
}
