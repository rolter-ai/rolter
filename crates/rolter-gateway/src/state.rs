use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use rolter_balancer::{build_with_stats, LoadBalancer, TargetStats};
use rolter_core::{
    BudgetConfig, CacheConfig, CooldownConfig, GatewayConfig, HealthConfig, ModelPriceConfig,
    ModelRoute, ProviderConfig, RateLimitConfig, RealtimeConfig, RetryConfig, Target,
};
use rolter_proxy::Forwarder;

use crate::budgets::BudgetEnforcer;
use crate::cache::ResponseCache;
use crate::health_events::HealthEventSink;
use crate::logging::LogSink;
use crate::metrics::Metrics;
use crate::queue::ProviderQueues;
use crate::rate_limits::RateLimiter;

/// A resolved route plus its constructed balancer.
pub struct RouteEntry {
    pub route: ModelRoute,
    pub balancer: Box<dyn LoadBalancer>,
    /// one balancer per variant (index-aligned with `route.variants`), built
    /// from the route's strategy and the variant's target weights so selection
    /// inside a variant honours the same strategy as the classic pool
    pub variant_balancers: Vec<Box<dyn LoadBalancer>>,
}

/// A virtual key as the request path sees it: identity/scope for attribution
/// plus the allow-list and validity window. Indexed by peppered digest in the
/// snapshot; the plaintext key is never retained.
#[derive(Debug, Clone, Default)]
pub struct KeyMeta {
    /// peppered virtual-key digest used only for tenant isolation
    pub tenant_key: String,
    pub id: String,
    pub org_id: String,
    pub team_id: String,
    pub project_id: String,
    pub models: Vec<String>,
    pub disabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// per-key response-cache override; `None` inherits the route decision,
    /// `Some(false)` bypasses the cache for this key, `Some(true)` caches even
    /// on a non-opted-in route (the global switch is still required)
    pub cache_override: Option<bool>,
}

impl KeyMeta {
    /// Whether the key may authenticate at `now`: not disabled and not expired.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.disabled && self.expires_at.is_none_or(|exp| now < exp)
    }
}

/// Immutable routing state. Hot-reload swaps a whole new snapshot in atomically
/// so request handlers never block on a lock or observe a half-applied config.
pub struct Snapshot {
    pub providers: HashMap<String, ProviderConfig>,
    pub routes: HashMap<String, RouteEntry>,
    /// virtual keys indexed by their peppered digest ([`rolter_auth::hash_key`]),
    /// never by plaintext — merges config-defined and database-defined keys
    pub keys: HashMap<String, KeyMeta>,
    /// deployment secret used to derive the key digests above
    pub pepper: String,
    /// per-model token pricing, keyed by public model name
    pub prices: HashMap<String, ModelPriceConfig>,
    /// spend caps to enforce, shared cheaply with per-request spend recorders
    pub budgets: Arc<Vec<BudgetConfig>>,
    /// throughput caps to enforce, shared cheaply with per-request recorders
    pub rate_limits: Arc<Vec<RateLimitConfig>>,
    /// upstream retry policy applied on transient failures
    pub retry: RetryConfig,
    /// bounded per-provider queue configuration, swapped atomically with routes
    pub queue: rolter_core::QueueConfig,
    /// per-target cooldown policy applied on transient failures
    pub cooldown: CooldownConfig,
    /// active health-probing tuning, read live by the background prober so a
    /// hot-reload can enable/disable probing and re-tune interval/timeout/path
    pub health: HealthConfig,
    /// global response-cache policy (master switch + default TTL + namespace);
    /// per-route opt-in lives on each route's `cache` field
    pub cache: CacheConfig,
    /// guardrails for long-lived WebSocket Realtime sessions
    pub realtime: RealtimeConfig,
}

/// Live per-target latency handle for the `fastest` strategy, backed by the
/// shared in-flight tracker (which survives config reloads). `namespace` is
/// the load-tracker key the route's guards record under: the public model for
/// the classic pool, the variant key for a variant pool.
struct RouteLatency {
    loads: crate::load::LoadTracker,
    namespace: String,
}

impl rolter_balancer::scorer::LatencySource for RouteLatency {
    fn latencies(&self, n: usize) -> Vec<f64> {
        self.loads.latency_snapshot(&self.namespace, n)
    }
}

impl Snapshot {
    /// Build a snapshot from a configuration. `loads` is the shared in-flight/
    /// latency tracker the `fastest` strategy reads live at pick time.
    pub fn build(config: &GatewayConfig, loads: &crate::load::LoadTracker) -> Self {
        let providers = config
            .providers
            .iter()
            .cloned()
            .map(|p| (p.name.clone(), p))
            .collect();
        let prices: HashMap<String, ModelPriceConfig> = config
            .model_prices
            .iter()
            .cloned()
            .map(|p| (p.model.clone(), p))
            .collect();
        let mut routes = HashMap::new();
        for route in &config.routes {
            let weights: Vec<u32> = route.targets.iter().map(|t| t.weight).collect();
            let stats = TargetStats {
                cost_per_mtok: target_costs(&route.targets, &route.model, &prices),
                latency: Some(Arc::new(RouteLatency {
                    loads: loads.clone(),
                    namespace: route.model.clone(),
                })),
            };
            let balancer = build_with_stats(route.strategy, &weights, &stats);
            let variant_balancers = route
                .variants
                .iter()
                .map(|v| {
                    let w: Vec<u32> = v.targets.iter().map(|t| t.weight).collect();
                    let s = TargetStats {
                        cost_per_mtok: target_costs(&v.targets, &route.model, &prices),
                        latency: Some(Arc::new(RouteLatency {
                            loads: loads.clone(),
                            namespace: crate::handlers::variant_key(&route.model, &v.name),
                        })),
                    };
                    build_with_stats(route.strategy, &w, &s)
                })
                .collect();
            routes.insert(
                route.model.clone(),
                RouteEntry {
                    route: route.clone(),
                    balancer,
                    variant_balancers,
                },
            );
        }
        let pepper = config.server.resolve_key_pepper();
        let mut keys: HashMap<String, KeyMeta> = HashMap::new();
        // config-defined keys: digest derived from the plaintext, no scope ids
        for k in &config.virtual_keys {
            let digest = rolter_auth::hash_key(&pepper, &k.key);
            keys.insert(
                digest.clone(),
                KeyMeta {
                    tenant_key: digest,
                    models: k.models.clone(),
                    disabled: k.disabled,
                    expires_at: k.expires_at,
                    cache_override: k.cache,
                    ..Default::default()
                },
            );
        }
        // database-defined keys: digest already stored, carry scope identity
        for k in &config.db_virtual_keys {
            keys.insert(
                k.key_hash.clone(),
                KeyMeta {
                    tenant_key: k.key_hash.clone(),
                    id: k.id.clone(),
                    org_id: k.org_id.clone(),
                    team_id: k.team_id.clone(),
                    project_id: k.project_id.clone(),
                    models: k.models.clone(),
                    disabled: k.disabled,
                    expires_at: k.expires_at,
                    cache_override: k.cache,
                },
            );
        }
        Self {
            providers,
            routes,
            keys,
            pepper,
            prices,
            budgets: Arc::new(config.budgets.clone()),
            rate_limits: Arc::new(config.rate_limits.clone()),
            retry: config.retry.clone(),
            queue: config.queue.clone(),
            cooldown: config.cooldown.clone(),
            health: config.health.clone(),
            cache: config.cache.clone(),
            realtime: config.realtime.clone(),
        }
    }
}

/// Per-target catalog cost for the `cheapest` strategy: the price of the
/// target's upstream model, falling back to the route's public model when the
/// target does not rename it. The rate is `input + output $/Mtok` — only the
/// relative order between targets matters to the scorer, and summing both
/// sides ranks sensibly without assuming a token mix. Unknown = `0.0`
/// (scored neutrally).
fn target_costs(
    targets: &[Target],
    public_model: &str,
    prices: &HashMap<String, ModelPriceConfig>,
) -> Vec<f64> {
    targets
        .iter()
        .map(|t| {
            let model = t.model.as_deref().unwrap_or(public_model);
            prices
                .get(model)
                .or_else(|| prices.get(public_model))
                .map(|p| p.input_per_mtok + p.output_per_mtok)
                .unwrap_or(0.0)
        })
        .collect()
}

/// Shared state handed to every request handler. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub forwarder: Arc<Forwarder>,
    /// bounded worker queues keyed by provider; queue settings come from the
    /// live snapshot so a hot reload takes effect for subsequent requests
    pub provider_queues: ProviderQueues,
    pub metrics: Arc<Metrics>,
    pub log: LogSink,
    /// batched writer for provider health events; disabled when no clickhouse url
    pub health_events: HealthEventSink,
    /// enforces spend caps against Redis; disabled when no redis url is set
    pub budgets: BudgetEnforcer,
    /// enforces throughput caps against Redis; disabled when no redis url is set
    pub rate_limiter: RateLimiter,
    /// exact-match response cache against Redis; disabled when no redis url is
    /// set. The global master switch lives on the live snapshot's `cache` field
    pub response_cache: ResponseCache,
    /// tenant-scoped routing records for model-less Responses lifecycle calls
    pub response_registry: crate::response_registry::ResponseRegistry,
    /// per-target cooldown registry, shared across requests and config reloads
    pub cooldowns: crate::cooldowns::Cooldowns,
    /// per-target in-flight load counters feeding the balancer
    pub loads: crate::load::LoadTracker,
    /// provider health registry populated by the background prober
    pub health: crate::health::Health,
    /// per-target circuit breaker registry, shared across requests and reloads
    pub breaker: crate::breaker::Breaker,
    /// upstream engine metrics snapshot populated by the background scraper
    pub upstream_metrics: crate::upstream_metrics::UpstreamMetrics,
    /// process-local concurrency registry for persistent Realtime sessions
    pub(crate) realtime_sessions: crate::realtime::Sessions,
}

impl AppState {
    /// Build state with logging and budget enforcement disabled. Used by tests
    /// and any caller that does not need the ClickHouse writer or Redis.
    #[cfg(test)]
    pub fn new(config: &GatewayConfig) -> Self {
        let metrics = Arc::new(Metrics::default());
        let log = LogSink::disabled(metrics.clone());
        let health_events = HealthEventSink::disabled(metrics.clone());
        Self::assemble(
            config,
            metrics,
            log,
            health_events,
            BudgetEnforcer::disabled(),
            RateLimiter::disabled(),
            ResponseCache::disabled(),
        )
    }

    /// Build state and, when a `clickhouse_url` is configured, spawn the async
    /// batched log writer. When `redis_url` is set, budget enforcement is backed
    /// by that Redis. Must be called from within a Tokio runtime.
    pub fn with_logging(config: &GatewayConfig, redis_url: Option<&str>) -> Self {
        let metrics = Arc::new(Metrics::default());
        let log = match &config.logging.clickhouse_url {
            Some(url) => LogSink::spawn(
                url.clone(),
                config.logging.batch_max,
                Duration::from_millis(config.logging.flush_ms),
                config.logging.queue_capacity,
                metrics.clone(),
            ),
            None => LogSink::disabled(metrics.clone()),
        };
        // reuse the same clickhouse endpoint and batching knobs as request logs
        let health_events = match &config.logging.clickhouse_url {
            Some(url) => HealthEventSink::spawn(
                url.clone(),
                config.logging.batch_max,
                Duration::from_millis(config.logging.flush_ms),
                config.logging.queue_capacity,
                metrics.clone(),
            ),
            None => HealthEventSink::disabled(metrics.clone()),
        };
        // the request funnel doubles as the passive health-event source
        let log = log.with_health_events(health_events.clone());
        let (budgets, rate_limiter) = match redis_url {
            Some(url) => (BudgetEnforcer::new(url), RateLimiter::new(url)),
            None => (BudgetEnforcer::disabled(), RateLimiter::disabled()),
        };
        // the cache shares the same Redis; keep the client even when the global
        // switch is currently off so a hot-reload can flip `[cache] enabled`
        // without rebuilding state (the snapshot's `cache.enabled` gates use)
        let response_cache = match redis_url {
            Some(url) => ResponseCache::new(url),
            None => ResponseCache::disabled(),
        };
        Self::assemble(
            config,
            metrics,
            log,
            health_events,
            budgets,
            rate_limiter,
            response_cache,
        )
    }

    fn assemble(
        config: &GatewayConfig,
        metrics: Arc<Metrics>,
        log: LogSink,
        health_events: HealthEventSink,
        budgets: BudgetEnforcer,
        rate_limiter: RateLimiter,
        response_cache: ResponseCache,
    ) -> Self {
        // created before the snapshot so the fastest strategy's latency
        // sources can hold a handle to the same tracker the guards record into
        let loads = crate::load::LoadTracker::new();
        let forwarder = Arc::new(Forwarder::with_timeouts(&config.timeouts));
        let provider_queues = ProviderQueues::new(forwarder.clone(), metrics.clone());
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::build(config, &loads))),
            forwarder,
            provider_queues,
            metrics,
            log,
            health_events,
            budgets,
            rate_limiter,
            response_cache,
            response_registry: crate::response_registry::ResponseRegistry::new(&config.responses),
            cooldowns: crate::cooldowns::Cooldowns::new(),
            loads,
            // an enabled registry only when probing is on, else an inert one that
            // always a live registry so a hot-reload that enables probing has a
            // store to populate; while probing is disabled the prober leaves the
            // map empty and every provider reads healthy (fail open)
            health: crate::health::Health::new(),
            // always a reconfigurable breaker (even when currently disabled) so a
            // config hot-reload can enable/disable and re-tune it in place without
            // discarding accumulated per-target state; see reload()
            breaker: crate::breaker::Breaker::new(
                config.breaker.enabled(),
                config.breaker.failure_threshold,
                config.breaker.open_secs,
            ),
            // an enabled snapshot only when scraping is on, else an inert one
            // that reports zero depth so it never perturbs the load view
            upstream_metrics: if config.metrics_scrape.enabled {
                crate::upstream_metrics::UpstreamMetrics::new()
            } else {
                crate::upstream_metrics::UpstreamMetrics::default()
            },
            realtime_sessions: crate::realtime::Sessions::default(),
        }
    }

    /// Atomically replace the routing snapshot (used by the config watcher).
    /// Records `version` in metrics and bumps the reload counter.
    pub fn reload(&self, config: &GatewayConfig, version: u64) {
        self.response_registry.reconfigure(&config.responses);
        self.snapshot
            .store(Arc::new(Snapshot::build(config, &self.loads)));
        // re-tune the circuit breaker in place (enable/disable + thresholds)
        // without discarding accumulated per-target state; the health prober picks
        // up its tuning from the new snapshot on its next sweep
        self.breaker.reconfigure(
            config.breaker.enabled(),
            config.breaker.failure_threshold,
            config.breaker.open_secs,
        );
        self.metrics
            .config_version
            .store(version, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .config_reloads_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(model: &str, input: f64, output: f64) -> ModelPriceConfig {
        ModelPriceConfig {
            model: model.to_string(),
            input_per_mtok: input,
            output_per_mtok: output,
            cached_input_per_mtok: None,
        }
    }

    fn target(provider: &str, model: Option<&str>) -> Target {
        Target {
            provider: provider.to_string(),
            model: model.map(str::to_string),
            weight: 1,
        }
    }

    #[test]
    fn target_costs_prefer_upstream_model_price() {
        let prices: HashMap<String, ModelPriceConfig> = [
            ("gpt".to_string(), price("gpt", 2.0, 8.0)),
            ("gpt-mini".to_string(), price("gpt-mini", 0.1, 0.4)),
        ]
        .into();
        let targets = vec![
            target("a", Some("gpt-mini")), // upstream price wins
            target("b", None),             // falls back to the public model
            target("c", Some("unpriced")), // unknown upstream -> public fallback
        ];
        let costs = target_costs(&targets, "gpt", &prices);
        assert_eq!(costs, vec![0.5, 10.0, 10.0]);
    }

    #[test]
    fn target_costs_unknown_everywhere_is_zero() {
        let prices = HashMap::new();
        let targets = vec![target("a", None)];
        assert_eq!(target_costs(&targets, "gpt", &prices), vec![0.0]);
    }

    #[test]
    fn reload_toggles_and_retunes_the_breaker() {
        let mut config = GatewayConfig::default();
        // breaker off by default: failures never trip, targets always admitted
        let state = AppState::with_logging(&config, None);
        assert!(!state.breaker.on_failure("m", 0));
        assert!(state.breaker.allows("m", 0));

        // a hot-reload enables the breaker with a threshold of 1
        config.breaker.enabled = true;
        config.breaker.failure_threshold = 1;
        config.breaker.open_secs = 30;
        state.reload(&config, 1);

        // now a single failure trips the target open in place, no restart needed
        assert!(state.breaker.on_failure("m", 0));
        assert!(!state.breaker.allows("m", 0));

        // a further reload that disables the breaker makes it admit again
        config.breaker.enabled = false;
        state.reload(&config, 2);
        assert!(state.breaker.allows("m", 0));
    }

    #[test]
    fn snapshot_carries_live_health_tuning() {
        let mut config = GatewayConfig::default();
        config.health.enabled = true;
        config.health.interval_secs = 7;
        config.health.path = "/ready".to_string();
        let state = AppState::with_logging(&config, None);
        let snap = state.snapshot.load();
        // the prober reads these off the snapshot each sweep
        assert!(snap.health.enabled);
        assert_eq!(snap.health.interval_secs, 7);
        assert_eq!(snap.health.path, "/ready");
    }
}
