use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    /// exact-match response cache (Redis) shared across replicas; off by default
    #[serde(default)]
    pub cache: CacheConfig,
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
    "/v1/messages",
    "/v1/embeddings",
    "/v1/rerank",
    "/v1/images/generations",
    "/v1/audio/speech",
    "/v1/audio/transcriptions",
    "/v1/audio/translations",
];

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
    /// a self-hosted ollama daemon using its openai-compatible api
    Ollama,
    /// a self-hosted llama.cpp llama-server using its openai-compatible api
    LlamaCpp,
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
    /// relative selection weight; clamped to at least 1 when sampling
    #[serde(default = "default_weight")]
    pub weight: u32,
}

impl ApiKeyConfig {
    /// Resolve this entry's key, preferring the inline value then the env var.
    pub fn resolve(&self) -> Option<String> {
        if let Some(k) = &self.key {
            return Some(k.clone());
        }
        self.env.as_ref().and_then(|e| std::env::var(e).ok())
    }
}

impl ProviderConfig {
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
            .and_then(|e| std::env::var(e).ok())
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
            .filter_map(|k| k.resolve().map(|s| (s, k.weight.max(1))))
            .collect()
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
            return Some((0, keys.remove(0).0));
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

/// A weighted routing variant: a named, weighted bundle of ordered targets plus
/// optional param defaults. A logical model maps to a set of these for A/B,
/// canary, and per-key traffic splitting under one schema (TensorZero pattern).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Variant {
    /// stable identifier for the variant (used in logs/metrics/attribution)
    pub name: String,
    /// relative traffic share; clamped to at least 1 when sampling
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// ordered upstream targets tried within this variant (provider routing order)
    pub targets: Vec<Target>,
    /// variant-scoped default inference params (same semantics as route params)
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// Pick an index from a sequence of weights proportional to weight, given a
/// random draw `r` in `[0.0, 1.0)`. Each weight is clamped to at least 1 so a
/// zero-weight entry can still be selected as a fallback. Returns `None` for an
/// empty sequence.
fn weighted_index(weights: impl Iterator<Item = u32>, r: f64) -> Option<usize> {
    let clamped: Vec<u64> = weights.map(|w| w.max(1) as u64).collect();
    if clamped.is_empty() {
        return None;
    }
    let total: u64 = clamped.iter().sum();
    // map r into [0, total); guard against r outside [0,1) landing past the end
    let mut point = (r.clamp(0.0, 1.0) * total as f64) as u64;
    if point >= total {
        point = total - 1;
    }
    let mut acc = 0u64;
    for (i, w) in clamped.iter().enumerate() {
        acc += w;
        if point < acc {
            return Some(i);
        }
    }
    Some(clamped.len() - 1)
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
            OverrideMode::Deny => self.allow.iter().any(|p| p == param),
        }
    }
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
    /// per-key response-cache override, independent of a route's own opt-in.
    /// `None` inherits the route decision; `Some(false)` forces this key's
    /// responses to bypass the cache; `Some(true)` caches them even on a route
    /// that didn't opt in. the global `[cache]` switch is still required either way
    #[serde(default)]
    pub cache: Option<bool>,
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
    /// per-key response-cache override; see [`VirtualKeyConfig::cache`]
    #[serde(default)]
    pub cache: Option<bool>,
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
}

fn default_true() -> bool {
    true
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
        }
    }
}

fn default_status_page_interval_secs() -> u64 {
    60
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
            if provider.kind == ProviderKind::Ollama
                && provider.api_base.trim_end_matches('/').ends_with("/v1")
            {
                problems.push(format!(
                    "ollama provider '{}' api_base must be the daemon origin without /v1",
                    provider.name
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
            if route.targets.is_empty() && route.variants.is_empty() {
                problems.push(format!(
                    "route '{}' has neither targets nor variants",
                    route.model
                ));
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
                    if !provider_names.contains(target.provider.as_str()) {
                        problems.push(format!(
                            "route '{}' variant '{}' targets unknown provider '{}'",
                            route.model, variant.name, target.provider
                        ));
                    }
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
            cache: None,
            variants: vec![],
        }
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
            api_key_env: None,
            egress_proxy: None,
            api_keys,
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
        }
    }

    #[test]
    fn single_key_provider_resolves_one_element_list() {
        // legacy configs behave exactly as before: one key, weight 1
        let p = provider_with_keys(Vec::new());
        assert_eq!(p.resolve_api_key().as_deref(), Some("legacy"));
        assert_eq!(p.resolve_api_keys(), vec![("legacy".to_string(), 1)]);
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
                weight: 0, // clamped to 1
            },
        ]);
        // the legacy pair is ignored once api_keys is non-empty
        assert_eq!(p.resolve_api_key().as_deref(), Some("k1"));
        assert_eq!(
            p.resolve_api_keys(),
            vec![("k1".to_string(), 3), ("k2".to_string(), 1)]
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
        // zero-weight entries are clamped to 1 so they remain selectable
        assert_eq!(weighted_index([0u32, 0].into_iter(), 0.0), Some(0));
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
        });
        assert!(!route.cache_enabled());
        // ttl override applies regardless of the enabled flag
        assert_eq!(route.cache_ttl_secs(300), 10);

        // enabled with per-key isolation and no ttl override
        route.cache = Some(RouteCache {
            enabled: true,
            ttl_secs: None,
            per_key: true,
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
