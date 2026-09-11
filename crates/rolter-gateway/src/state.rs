use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use rolter_balancer::{build_with_stats, LoadBalancer, TargetStats};
use rolter_core::{
    BudgetConfig, CacheConfig, CooldownConfig, GatewayConfig, HealthConfig, LoggingConfig,
    ModelPriceConfig, ModelRoute, ProviderConfig, RateLimitConfig, RealtimeConfig, RetryConfig,
    Target,
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
    /// which guardrail rules apply on this route, resolved once here so the
    /// request path never re-derives it from names (#590)
    pub guardrails: rolter_core::guardrails::RuleSelection,
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
    /// user who minted this database-backed key; required for MCP OBO access
    pub user_id: String,
    pub models: Vec<String>,
    /// provider allow-list; an empty list permits every provider on the route
    pub providers: Vec<String>,
    pub disabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// per-key response-cache override; `None` inherits the route decision,
    /// `Some(false)` bypasses the cache for this key, `Some(true)` caches even
    /// on a non-opted-in route (the global switch is still required)
    pub cache_override: Option<bool>,
    /// governance dimensions this key's spend rolls up to; empty when the key
    /// is attributed by tenancy alone (#539)
    pub business_unit_id: String,
    pub customer_id: String,
    /// merged model/route policy of the access profiles this key's owner holds,
    /// resolved by the control plane when the snapshot was built. `None` is
    /// unrestricted, which is every key on a deployment with no access profiles
    /// and every key with no owner (#791)
    pub access_policy: Option<rolter_core::ModelPolicy>,
}

impl KeyMeta {
    /// Whether the key may authenticate at `now`: not disabled and not expired.
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.disabled && self.expires_at.is_none_or(|exp| now < exp)
    }

    pub fn provider_allowed(&self, provider: &str) -> bool {
        self.providers.is_empty() || self.providers.iter().any(|name| name == provider)
    }

    /// Whether this key may address `model`, by both gates it has to clear: the
    /// key's own allow-list and the access-profile policy its owner holds.
    ///
    /// The two are separate grants and both must permit. The key list is
    /// what the key's creator chose to scope this credential to; the policy is
    /// what an operator decided the *person* may reach at all. Neither can widen
    /// the other, so a key naming a model its owner is denied stays denied.
    pub fn model_permitted(&self, model: &str) -> bool {
        rolter_auth::model_allowed(&self.models, model)
            && self
                .access_policy
                .as_ref()
                .is_none_or(|policy| policy.permits_model(model))
    }

    /// Whether this key may address the configured route named `route`.
    ///
    /// Routes are a separate axis from models: an operator restricts a route to
    /// gate a whole named entry point, independently of the upstream model ids
    /// that entry point happens to fan out to.
    pub fn route_permitted(&self, route: &str) -> bool {
        self.access_policy
            .as_ref()
            .is_none_or(|policy| policy.permits_route(route))
    }
}

/// Immutable routing state. Hot-reload swaps a whole new snapshot in atomically
/// so request handlers never block on a lock or observe a half-applied config.
pub struct Snapshot {
    pub providers: HashMap<String, ProviderConfig>,
    /// provider slug -> provider name, for `provider-slug/model` addressing
    /// (ADR-0017). A provider without an explicit slug is indexed under one
    /// derived from its name so TOML-configured deployments get addressing too
    pub providers_by_slug: HashMap<String, String>,
    /// group slug -> provider group, for `group-slug/model` addressing (ADR-0017
    /// addendum). Shares the slug namespace with `providers_by_slug`; a group
    /// whose slug collides with a provider slug is dropped so the provider wins
    /// deterministically
    pub groups_by_slug: HashMap<String, rolter_core::ProviderGroupConfig>,
    pub routes: HashMap<String, RouteEntry>,
    /// virtual keys indexed by their peppered digest ([`rolter_auth::hash_key`]),
    /// never by plaintext — merges config-defined and database-defined keys
    pub keys: HashMap<String, KeyMeta>,
    /// MCP servers keyed by `(org_id, slug)` for tenant-safe path resolution
    pub mcp_servers: HashMap<(String, String), rolter_core::McpServerConfig>,
    /// newest live session keyed by `(server_id, user_id)`
    pub mcp_oauth_sessions: HashMap<(String, String), rolter_core::McpOAuthSessionConfig>,
    /// deployment secret used to derive the key digests above
    pub pepper: String,
    /// config override for whether an empty `keys` set still enforces auth.
    /// `None` defers to the deployment (see [`AppState::managed_auth`])
    pub require_auth: Option<bool>,
    /// deployment-wide ingress policy set on the control plane's Security
    /// screen and carried here by the snapshot (#1162)
    pub security: rolter_core::SecurityPolicyConfig,
    /// per-model token pricing, keyed by public model name, with every rate
    /// already expressed in [`Self::base_currency`] — a model priced in EUR
    /// arrives here converted, so the hot path never does FX (#650)
    pub prices: HashMap<String, ModelPriceConfig>,
    /// currency budgets, recorded spend and `cost_usd` are denominated in
    pub base_currency: String,
    /// spend caps to enforce, shared cheaply with per-request spend recorders
    pub budgets: Arc<Vec<BudgetConfig>>,
    /// deployment-wide default for traffic the gateway cannot price (#974);
    /// an applicable budget may tighten it per request
    pub unpriced_policy: rolter_core::UnpricedPolicy,
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
    /// live raw-payload capture settings; the writer still remains asynchronous
    pub logging: LoggingConfig,
    /// guardrails for long-lived WebSocket Realtime sessions
    pub realtime: RealtimeConfig,
    /// built-in regex guardrails / PII redaction, compiled once per snapshot and
    /// shared across requests; inert unless enabled (ROL-261)
    pub guardrails: Arc<rolter_core::CompiledGuardrails>,
    /// custom guardrail webhook config, swapped atomically with the snapshot;
    /// inert unless enabled (ROL-257)
    pub guardrail_webhook: rolter_core::GuardrailWebhookConfig,
    /// external PII sanitizer config, swapped atomically with the snapshot;
    /// inert unless enabled (#848)
    pub pii_sanitizer: rolter_core::PiiSanitizerConfig,
    /// versioned prompt templates / route decorators, compiled once per snapshot
    /// and shared across requests; inert unless enabled (ROL-256)
    pub prompt_templates: Arc<rolter_core::CompiledTemplates>,
    /// deployment-wide adaptive-routing policy, read when a route's balancer is
    /// built; inert unless enabled (#544)
    pub adaptive_routing: rolter_core::AdaptiveRoutingConfig,
    /// enabled webhook plugin instances, swapped atomically with the snapshot;
    /// empty by default and inert on the request path (#509)
    pub plugins: Arc<rolter_core::PluginsConfig>,
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
        Self::build_with_telemetry(config, loads, None)
    }

    fn build_with_telemetry(
        config: &GatewayConfig,
        loads: &crate::load::LoadTracker,
        telemetry: Option<&crate::cache_telemetry::CacheTelemetry>,
    ) -> Self {
        let providers: HashMap<String, ProviderConfig> = config
            .providers
            .iter()
            .cloned()
            .map(|mut p| {
                p.ca_bundles = Some(config.ca_bundles_for(&p));
                (p.name.clone(), p)
            })
            .collect();
        // index providers by their URL-safe slug for `provider-slug/model`
        // addressing; an explicit slug wins, otherwise derive one from the
        // name. skip invalid/empty derived slugs, and let the first provider
        // win a collision so the index stays deterministic
        let mut providers_by_slug: HashMap<String, String> = HashMap::new();
        for p in providers.values() {
            let slug = p
                .slug
                .clone()
                .unwrap_or_else(|| rolter_core::slug::slugify(&p.name));
            if rolter_core::slug::is_valid_slug(&slug) {
                providers_by_slug
                    .entry(slug)
                    .or_insert_with(|| p.name.clone());
            }
        }
        // index provider groups by slug for `group-slug/model` addressing
        // (ADR-0017 addendum). the slug shares the provider namespace: a group
        // whose slug collides with a provider slug is skipped so the provider
        // wins deterministically. empty groups are dropped — they never route.
        let mut groups_by_slug: HashMap<String, rolter_core::ProviderGroupConfig> = HashMap::new();
        for g in &config.provider_groups {
            let slug = g
                .slug
                .clone()
                .unwrap_or_else(|| rolter_core::slug::slugify(&g.name));
            if !rolter_core::slug::is_valid_slug(&slug)
                || g.members.is_empty()
                || providers_by_slug.contains_key(&slug)
            {
                continue;
            }
            groups_by_slug.entry(slug).or_insert_with(|| g.clone());
        }
        // price rates are converted into the base currency once, here, rather
        // than per request: cost is linear in the rates, so converting the
        // rates converts every cost derived from them, exactly.
        //
        // a price whose currency has no rate cannot be converted and is dropped
        // with a loud error. `GatewayConfig::validate` rejects that config
        // before it can become a snapshot, so this is unreachable in a served
        // config — but if it ever is reached, an unpriced model is the only
        // honest outcome: face value and zero are both the wrong number.
        let fx = rolter_core::StaticRates::new(config.currency.clone());
        let base_currency = config.currency.base_code();
        let prices: HashMap<String, ModelPriceConfig> = config
            .model_prices
            .iter()
            .filter_map(|p| match to_base_currency(&fx, p, &base_currency) {
                Some(converted) => Some((p.model.clone(), converted)),
                None => {
                    tracing::error!(
                        model = %p.model,
                        currency = %p.currency,
                        base = %base_currency,
                        "no conversion rate for this model's price currency;                          the model is priced as unpriced and accrues no spend"
                    );
                    None
                }
            })
            .collect();
        // compiled before the route loop so each route can resolve its
        // guardrail override into an index mask once (#590)
        let compiled_guardrails = Arc::new(rolter_core::CompiledGuardrails::from_config(
            &config.guardrails,
        ));
        let mut routes = HashMap::new();
        for route in &config.routes {
            let weights: Vec<u32> = route.targets.iter().map(|t| t.weight).collect();
            let stats = TargetStats {
                cost_per_mtok: target_costs(&route.targets, &route.model, &prices),
                latency: Some(Arc::new(RouteLatency {
                    loads: loads.clone(),
                    namespace: route.model.clone(),
                })),
                kv_cache: telemetry.map(|source| {
                    source.kv_source(
                        route
                            .targets
                            .iter()
                            .map(|target| target.provider.clone())
                            .collect(),
                    )
                }),
                lmcache: telemetry.map(|source| {
                    source.lmcache_source(
                        route
                            .targets
                            .iter()
                            .map(|target| target.provider.clone())
                            .collect(),
                    )
                }),
                adaptive: config.adaptive_routing.clone(),
                // created (and re-found) under the route's own load-tracker
                // namespace, so it survives the reload that rebuilt this
                // snapshot along with everything else the tracker holds
                predictor: loads
                    .predictor(&route.model, weights.len())
                    .map(|p| p as Arc<dyn rolter_balancer::scorer::LatencyPredictionSource>),
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
                        kv_cache: telemetry.map(|source| {
                            source.kv_source(
                                v.targets
                                    .iter()
                                    .map(|target| target.provider.clone())
                                    .collect(),
                            )
                        }),
                        lmcache: telemetry.map(|source| {
                            source.lmcache_source(
                                v.targets
                                    .iter()
                                    .map(|target| target.provider.clone())
                                    .collect(),
                            )
                        }),
                        adaptive: config.adaptive_routing.clone(),
                        predictor: loads
                            .predictor(
                                &crate::handlers::variant_key(&route.model, &v.name),
                                w.len(),
                            )
                            .map(|p| {
                                p as Arc<dyn rolter_balancer::scorer::LatencyPredictionSource>
                            }),
                    };
                    build_with_stats(route.strategy, &w, &s)
                })
                .collect();
            routes.insert(
                route.model.clone(),
                RouteEntry {
                    guardrails: compiled_guardrails.resolve_selection(&route.advanced.guardrails),
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
                    providers: k.providers.clone(),
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
                    user_id: k.user_id.clone(),
                    models: k.models.clone(),
                    providers: k.providers.clone(),
                    disabled: k.disabled,
                    expires_at: k.expires_at,
                    cache_override: k.cache,
                    business_unit_id: k.business_unit_id.clone(),
                    customer_id: k.customer_id.clone(),
                    access_policy: k.access_policy.clone(),
                },
            );
        }
        let mcp_servers = config
            .mcp_servers
            .iter()
            .cloned()
            .map(|server| ((server.org_id.clone(), server.slug.clone()), server))
            .collect();
        let mcp_oauth_sessions = config
            .mcp_oauth_sessions
            .iter()
            .cloned()
            .map(|session| {
                (
                    (session.server_id.clone(), session.user_id.clone()),
                    session,
                )
            })
            .collect();
        Self {
            providers,
            providers_by_slug,
            groups_by_slug,
            routes,
            keys,
            mcp_servers,
            mcp_oauth_sessions,
            pepper,
            require_auth: config.server.require_auth,
            security: config.security.clone(),
            prices,
            base_currency,
            budgets: Arc::new(config.budgets.clone()),
            unpriced_policy: config.unpriced_policy,
            rate_limits: Arc::new(config.rate_limits.clone()),
            retry: config.retry.clone(),
            queue: config.queue.clone(),
            cooldown: config.cooldown.clone(),
            health: config.health.clone(),
            cache: config.cache.clone(),
            logging: config.logging.clone(),
            realtime: config.realtime.clone(),
            guardrails: compiled_guardrails,
            guardrail_webhook: config.guardrail_webhook.clone(),
            pii_sanitizer: config.pii_sanitizer.clone(),
            prompt_templates: Arc::new(rolter_core::CompiledTemplates::from_config(
                &config.prompt_templates,
            )),
            adaptive_routing: config.adaptive_routing.clone(),
            plugins: Arc::new(config.plugins.clone()),
        }
    }

    /// Resolve a `provider-slug/model` address to a synthetic single-target
    /// [`RouteEntry`] pinned to that provider (ADR-0017). Callers try
    /// [`Snapshot::routes`] first so a route whose name literally contains `/`
    /// keeps winning; only on a miss do they split on the first `/` and land
    /// here. Returns `None` when the string has no `/`, the left segment is not
    /// a known provider slug, or the right (upstream model) segment is empty.
    ///
    /// The pinned entry bypasses cross-provider fan-out but still fans out
    /// within the provider: its key pool, cooldowns, and circuit breaker run
    /// through the same classic-pool machinery, and the right segment becomes
    /// the target's upstream model rewrite.
    pub fn resolve_pinned(&self, model: &str) -> Option<RouteEntry> {
        let (slug, upstream) = model.split_once('/')?;
        if upstream.is_empty() {
            return None;
        }
        // a provider slug pins a single provider; a group slug fans out across
        // the group's members. the two share a namespace, so at most one hits.
        if let Some(provider_name) = self.providers_by_slug.get(slug) {
            let target = Target {
                provider: provider_name.clone(),
                model: Some(upstream.to_string()),
                weight: 1,
            };
            let strategy = rolter_core::BalancingStrategy::default();
            return Some(self.synthetic_route(model, strategy, vec![target]));
        }
        if let Some(group) = self.groups_by_slug.get(slug) {
            // one target per member; each rewrites to its own upstream model
            // when set, otherwise forwards the requested model as-is (passthrough)
            let targets: Vec<Target> = group
                .members
                .iter()
                .map(|m| Target {
                    provider: m.provider.clone(),
                    model: Some(m.model.clone().unwrap_or_else(|| upstream.to_string())),
                    weight: m.weight,
                })
                .collect();
            if targets.is_empty() {
                return None;
            }
            return Some(self.synthetic_route(model, group.strategy, targets));
        }
        None
    }

    /// Build a synthetic single-pool [`RouteEntry`] for an addressed model
    /// (`provider-slug/model` or `group-slug/model`). The pinned entry runs
    /// through the same classic-pool machinery — key pools, cooldowns, circuit
    /// breaker — as a configured route, but is not registered in `routes`.
    fn synthetic_route(
        &self,
        model: &str,
        strategy: rolter_core::BalancingStrategy,
        targets: Vec<Target>,
    ) -> RouteEntry {
        let weights: Vec<u32> = targets.iter().map(|t| t.weight).collect();
        let stats = TargetStats {
            cost_per_mtok: target_costs(&targets, model, &self.prices),
            latency: None,
            adaptive: self.adaptive_routing.clone(),
            ..Default::default()
        };
        let balancer = build_with_stats(strategy, &weights, &stats);
        let route = ModelRoute {
            model: model.to_string(),
            strategy,
            targets,
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
        };
        RouteEntry {
            route,
            balancer,
            variant_balancers: Vec::new(),
            // a synthetic route has no config to override with, so it runs the
            // full global rule set
            guardrails: Default::default(),
        }
    }
}

/// Re-denominate a price into `base`, or `None` when the pair has no rate.
///
/// Every rate is scaled by the same factor and the currency label is rewritten,
/// so the returned price behaves identically to one the operator had entered in
/// the base currency directly.
fn to_base_currency(
    fx: &dyn rolter_core::CurrencyConverter,
    price: &ModelPriceConfig,
    base: &str,
) -> Option<ModelPriceConfig> {
    let factor = fx.convert(rust_decimal::Decimal::ONE, &price.currency, base)?;
    Some(ModelPriceConfig {
        model: price.model.clone(),
        input_per_mtok: price.input_per_mtok * factor,
        output_per_mtok: price.output_per_mtok * factor,
        cached_input_per_mtok: price.cached_input_per_mtok.map(|rate| rate * factor),
        currency: base.to_string(),
    })
}

/// Per-target catalog cost for the `cheapest` strategy: the price of the
/// target's upstream model, falling back to the route's public model when the
/// target does not rename it. The rate is `input + output $/Mtok` — only the
/// relative order between targets matters to the scorer, and summing both
/// sides ranks sensibly without assuming a token mix. Unknown = `0.0`
/// (scored neutrally).
///
/// `f64` on purpose, and one of the few money-adjacent values #967 leaves
/// alone: nothing is billed from this number. It is a ranking key the scorer
/// compares against its siblings, so the exactness a `Decimal` would buy has
/// nothing here to spend itself on.
fn target_costs(
    targets: &[Target],
    public_model: &str,
    prices: &HashMap<String, ModelPriceConfig>,
) -> Vec<f64> {
    use rust_decimal::prelude::ToPrimitive;
    targets
        .iter()
        .map(|t| {
            let model = t.model.as_deref().unwrap_or(public_model);
            prices
                .get(model)
                .or_else(|| prices.get(public_model))
                .and_then(|p| (p.input_per_mtok + p.output_per_mtok).to_f64())
                .unwrap_or(0.0)
        })
        .collect()
}

/// Shared state handed to every request handler. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    /// whether this gateway is managed by a control plane (it polls a snapshot
    /// url). managed deployments fail closed on an empty virtual-key set so
    /// revoking the last key locks the data plane down instead of opening it;
    /// `server.require_auth` overrides this either way. process-level rather
    /// than snapshot-level so a control-plane reload can never clear it
    pub managed_auth: bool,
    /// set by the config watcher when the control plane marks this node as
    /// draining; `/readyz` then reports not-ready so a load balancer stops
    /// sending new traffic while in-flight requests finish (#543)
    pub draining: Arc<std::sync::atomic::AtomicBool>,
    /// live egress policy every upstream client's resolver consults at connect
    /// time; swapped on reload so hostname/rebinding enforcement re-tunes
    /// without rebuilding pooled clients (#656)
    pub egress: rolter_proxy::egress_resolver::SharedEgressPolicy,
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
    /// dedup for the `warn` unpriced-traffic policy, so an unenforceable budget
    /// is named once per model per window rather than once per request (#974)
    pub unpriced_warns: Arc<crate::budgets::UnpricedWarnLog>,
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
    /// background vLLM/LMCache telemetry read by cache-aware scorers
    pub cache_telemetry: crate::cache_telemetry::CacheTelemetry,
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
        // budget spend and rate-limit tokens are recorded after the response,
        // through a bounded queue rather than a detached task per request, so a
        // slow counter store sheds records instead of accumulating them (#1051).
        // Only spawned with a counter store behind it: without redis there is
        // nothing to record, and an inert sink costs no workers
        let log = match redis_url {
            Some(_) => log.with_usage_recorders(crate::usage_recording::UsageRecorderSink::spawn(
                config.usage_recording.queue_capacity,
                config.usage_recording.workers,
                metrics.clone(),
            )),
            None => log,
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
        // one live egress policy shared by every upstream client's resolver, so
        // a hot reload re-tunes connect-time enforcement without discarding
        // pooled connections (#656)
        let egress = Arc::new(ArcSwap::from_pointee(config.egress.clone()));
        let forwarder = Arc::new(Forwarder::with_timeouts_and_egress(
            &config.timeouts,
            egress.clone(),
        ));
        forwarder.set_compatibility(&config.compatibility);
        forwarder.set_client_policy(&config.client);
        forwarder.set_model_defaults(&config.model_defaults);
        let provider_queues = ProviderQueues::new(forwarder.clone(), metrics.clone());
        let cache_telemetry = crate::cache_telemetry::CacheTelemetry::new(metrics.clone());
        cache_telemetry.configure(&config.providers);
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::build_with_telemetry(
                config,
                &loads,
                Some(&cache_telemetry),
            ))),
            // opted into by the binary when a snapshot url is configured
            managed_auth: false,
            draining: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            egress,
            forwarder,
            provider_queues,
            metrics,
            log,
            health_events,
            budgets,
            unpriced_warns: Arc::new(crate::budgets::UnpricedWarnLog::default()),
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
            cache_telemetry,
            realtime_sessions: crate::realtime::Sessions::default(),
        }
    }

    /// Atomically replace the routing snapshot (used by the config watcher).
    /// Records `version` in metrics and bumps the reload counter.
    pub fn reload(&self, config: &GatewayConfig, version: u64) {
        self.response_registry.reconfigure(&config.responses);
        // configured clients capture CA roots at construction time; clearing
        // them makes bundle rotation take effect on the next request
        self.forwarder.reload(&config.timeouts);
        // cross-dialect translation behavior is hot-swappable: the next request
        // reads the new policy, in-flight ones keep the one they started with
        self.forwarder.set_compatibility(&config.compatibility);
        // header policy and inference defaults are hot-swappable the same way
        self.forwarder.set_client_policy(&config.client);
        self.forwarder.set_model_defaults(&config.model_defaults);
        // the resolver reads this on every lookup, so a policy change takes
        // effect on the next connect without rebuilding a single client
        self.egress.store(Arc::new(config.egress.clone()));
        self.cache_telemetry.configure(&config.providers);
        self.snapshot.store(Arc::new(Snapshot::build_with_telemetry(
            config,
            &self.loads,
            Some(&self.cache_telemetry),
        )));
        // re-tune the circuit breaker in place (enable/disable + thresholds)
        // without discarding accumulated per-target state; the health prober picks
        // up its tuning from the new snapshot on its next sweep
        self.breaker.reconfigure(
            config.breaker.enabled(),
            config.breaker.failure_threshold,
            config.breaker.open_secs,
        );
        // a reload is where models and targets get retired, and the gateway
        // applies one without restarting — so it is also where the breaker
        // sheds entries nothing routes to any more. Open and half-open targets
        // are kept: they are under probe and forgetting one would re-admit
        // traffic to an upstream that is still down (#1053)
        self.breaker
            .sweep(breaker_idle_ttl(config.breaker.open_secs));
        self.metrics.breaker_entries.store(
            self.breaker.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.metrics
            .config_version
            .store(version, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .config_reloads_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// How long a closed breaker entry may sit with an unmoved failure count before
/// a reload retires it. Scaled off the configured open window so a deployment
/// that trips slowly is not swept out from under itself, with a floor so the
/// common case of a short window still keeps a meaningful history.
fn breaker_idle_ttl(open_secs: u64) -> std::time::Duration {
    const FLOOR: std::time::Duration = std::time::Duration::from_secs(900);
    std::time::Duration::from_secs(open_secs.saturating_mul(10)).max(FLOOR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    /// A decimal literal for tests. `rust_decimal`'s `dec!` macro would read
    /// slightly better, but its `macros` feature pulls `rust_decimal_macros`,
    /// `proc-macro-crate`, `toml_edit` and `borsh` into the dependency graph in
    /// production position, which is a poor trade for test ergonomics (#967).
    fn d(literal: &str) -> rust_decimal::Decimal {
        literal.parse().expect("a valid decimal literal")
    }

    fn price(model: &str, input: Decimal, output: Decimal) -> ModelPriceConfig {
        ModelPriceConfig {
            model: model.to_string(),
            input_per_mtok: input,
            output_per_mtok: output,
            cached_input_per_mtok: None,
            currency: "USD".to_string(),
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
            ("gpt".to_string(), price("gpt", d("2.0"), d("8.0"))),
            (
                "gpt-mini".to_string(),
                price("gpt-mini", d("0.1"), d("0.4")),
            ),
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
    fn a_non_base_price_is_converted_before_it_can_reach_a_budget() {
        // the #650 acceptance case: a EUR-priced model must accrue spend in the
        // USD budget at the configured rate, not at face value
        let mut config = GatewayConfig::default();
        config.currency.rates.insert("EUR".to_string(), d("1.10"));
        config.model_prices.push(ModelPriceConfig {
            model: "mistral-large".to_string(),
            input_per_mtok: d("2.0"),
            output_per_mtok: d("6.0"),
            cached_input_per_mtok: None,
            currency: "EUR".to_string(),
        });
        let loads = crate::load::LoadTracker::new();
        let snap = Snapshot::build(&config, &loads);

        let price = snap.prices.get("mistral-large").expect("price kept");
        assert_eq!(price.currency, "USD");
        // exact now, not within an epsilon: 2.0 * 1.10 and 6.0 * 1.10 are
        // both exact in decimal, and were not in binary (#967)
        assert_eq!(price.input_per_mtok, d("2.200"), "{price:?}");
        assert_eq!(price.output_per_mtok, d("6.600"), "{price:?}");
        // 1M input + 1M output: EUR 8.00 -> USD 8.80, not USD 8.00
        assert_eq!(price.cost(1_000_000, 1_000_000, 0), d("8.800000"));
        assert_eq!(snap.base_currency, "USD");
    }

    #[test]
    fn a_price_in_an_unconvertible_currency_is_dropped_not_mispriced() {
        // validate() rejects this config, so a served snapshot never contains
        // one; if it somehow does, an unpriced model is the only honest outcome
        let mut config = GatewayConfig::default();
        config.model_prices.push(ModelPriceConfig {
            model: "priced-in-doubloons".to_string(),
            input_per_mtok: d("2.0"),
            output_per_mtok: d("6.0"),
            cached_input_per_mtok: None,
            currency: "DBL".to_string(),
        });
        let loads = crate::load::LoadTracker::new();
        let snap = Snapshot::build(&config, &loads);
        assert!(!snap.prices.contains_key("priced-in-doubloons"));
    }

    #[test]
    fn a_non_usd_base_currency_converts_usd_prices_into_it() {
        // nothing assumes USD is the base: an operator settling in EUR gets
        // their USD-priced models converted the other way
        let mut config = GatewayConfig::default();
        config.currency.base = "EUR".to_string();
        config.currency.rates.insert("USD".to_string(), d("0.9"));
        config.model_prices.push(ModelPriceConfig {
            model: "gpt-4o".to_string(),
            input_per_mtok: d("10.0"),
            output_per_mtok: d("0.0"),
            cached_input_per_mtok: None,
            currency: "USD".to_string(),
        });
        let loads = crate::load::LoadTracker::new();
        let snap = Snapshot::build(&config, &loads);
        let price = snap.prices.get("gpt-4o").expect("price kept");
        assert_eq!(snap.base_currency, "EUR");
        assert_eq!(price.input_per_mtok, d("9.0"), "{price:?}");
    }

    #[test]
    fn reload_retunes_connect_time_egress_in_place() {
        // the resolver reads this handle on every lookup, so tightening the
        // policy must take effect without rebuilding a single pooled client
        let mut config = GatewayConfig::default();
        let state = AppState::with_logging(&config, None);
        assert!(!state.egress.load().block_private);

        config.egress.block_private = true;
        state.reload(&config, 1);
        assert!(state.egress.load().block_private);
        assert!(state
            .egress
            .load()
            .deny_reason("10.0.0.5".parse().unwrap())
            .is_some());
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

    /// The gateway hot-reloads config without restarting, so a long-lived
    /// process serves many config generations. Entries for models those reloads
    /// retired must not accumulate for the process lifetime (#1053).
    #[test]
    fn breaker_entries_do_not_accumulate_across_reloads_that_retire_models() {
        let mut config = GatewayConfig::default();
        config.breaker.enabled = true;
        config.breaker.failure_threshold = 3;
        config.breaker.open_secs = 30;
        let state = AppState::with_logging(&config, None);

        for generation in 0..50u64 {
            // each generation routes to its own model, which the next one retires
            let model = format!("retired-model-{generation}");
            // a partial failure count: below threshold, so the target stays
            // closed and is exactly the entry that used to linger forever
            state.breaker.on_failure(&model, 0);
            state.breaker.on_failure(&model, 1);
            // a sweep only retires entries whose failure count has not moved
            // for the idle window, so drive it by sweeping with a zero window —
            // the reload path uses the real one
            state.breaker.sweep(std::time::Duration::ZERO);
            state.reload(&config, generation + 1);
            assert!(
                state.breaker.is_empty(),
                "generation {generation} left {} entries behind",
                state.breaker.len()
            );
        }
    }

    /// A target under probe must survive the sweep: forgetting an open breaker
    /// would silently re-admit traffic to an upstream that is still down.
    #[test]
    fn a_sweep_never_forgets_an_open_or_half_open_target() {
        let mut config = GatewayConfig::default();
        config.breaker.enabled = true;
        config.breaker.failure_threshold = 1;
        config.breaker.open_secs = 3600;
        let state = AppState::with_logging(&config, None);

        // "open": tripped with a long window still to run
        assert!(state.breaker.on_failure("open-model", 0));
        assert!(!state.breaker.allows("open-model", 0));

        // "half-open": tripped with a window that has already elapsed, then
        // probed, which moves it to half-open
        state.breaker.reconfigure(true, 1, /* open_secs */ 0);
        assert!(state.breaker.on_failure("probing-model", 0));
        assert!(
            state.breaker.allows("probing-model", 0),
            "the elapsed window admits a half-open probe"
        );
        state.breaker.reconfigure(true, 1, 3600);

        // a closed entry alongside them, which *is* eligible
        state.breaker.reconfigure(true, 5, 3600);
        state.breaker.on_failure("healthy-model", 0);

        state.breaker.sweep(std::time::Duration::ZERO);

        assert!(
            !state.breaker.allows("open-model", 0),
            "the open target must still be skipped after a sweep"
        );
        assert!(
            state.breaker.allows("probing-model", 0),
            "the half-open target is admitted, but must still hold its entry"
        );
        // a failure on the half-open target re-opens it, which is only possible
        // if the sweep kept its phase
        state.breaker.reconfigure(true, 5, 3600);
        assert!(
            state.breaker.on_failure("probing-model", 0),
            "a half-open target that was forgotten would need 5 failures to trip"
        );
        assert_eq!(
            state.breaker.len(),
            2,
            "only the closed entry should have been retired"
        );
    }

    /// The gauge is what makes registry growth observable rather than inferred.
    #[test]
    fn reload_publishes_the_breaker_entry_count() {
        let mut config = GatewayConfig::default();
        config.breaker.enabled = true;
        config.breaker.failure_threshold = 1;
        config.breaker.open_secs = 3600;
        let state = AppState::with_logging(&config, None);
        let gauge = || {
            state
                .metrics
                .breaker_entries
                .load(std::sync::atomic::Ordering::Relaxed)
        };

        state.reload(&config, 1);
        assert_eq!(gauge(), 0);

        state.breaker.on_failure("m", 0);
        state.breaker.on_failure("m", 1);
        state.reload(&config, 2);
        assert_eq!(gauge(), 2, "two tripped targets should be reported");

        // recovery gives the slots back, and the next reload says so
        state.breaker.on_success("m", 0);
        state.breaker.on_success("m", 1);
        state.reload(&config, 3);
        assert_eq!(gauge(), 0);
    }

    /// The idle window scales off the configured open window, with a floor.
    #[test]
    fn breaker_idle_ttl_scales_with_the_open_window() {
        use std::time::Duration;
        // a short window still keeps a meaningful history
        assert_eq!(breaker_idle_ttl(0), Duration::from_secs(900));
        assert_eq!(breaker_idle_ttl(30), Duration::from_secs(900));
        // a slow-tripping deployment is not swept out from under itself
        assert_eq!(breaker_idle_ttl(600), Duration::from_secs(6000));
        // and an absurd window cannot overflow into a tiny one
        assert!(breaker_idle_ttl(u64::MAX) >= Duration::from_secs(900));
    }

    fn provider_cfg(name: &str, slug: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: name.to_string(),
            slug: slug.map(str::to_string),
            kind: rolter_core::ProviderKind::OpenaiCompatible,
            api_base: "http://x".to_string(),
            ..Default::default()
        }
    }

    fn pinning_snapshot() -> Snapshot {
        let mut config = GatewayConfig::default();
        // one provider with an explicit slug, one that must derive it from name
        config
            .providers
            .push(provider_cfg("vLLM SPB", Some("vllm-spb")));
        config.providers.push(provider_cfg("derived-name", None));
        // a route whose public name literally contains a slash
        config.routes.push(ModelRoute {
            model: "vllm-spb/named-route".to_string(),
            strategy: rolter_core::BalancingStrategy::default(),
            targets: vec![target("vLLM SPB", Some("real-model"))],
            params: Default::default(),
            param_policy: Default::default(),
            advanced: Default::default(),
            variants: Vec::new(),
            cache: None,
        });
        Snapshot::build(&config, &crate::load::LoadTracker::new())
    }

    #[test]
    fn pinning_resolves_explicit_slug_to_provider_and_upstream() {
        let snap = pinning_snapshot();
        let entry = snap.resolve_pinned("vllm-spb/qwen3").expect("resolves");
        assert_eq!(entry.route.targets.len(), 1);
        assert_eq!(entry.route.targets[0].provider, "vLLM SPB");
        assert_eq!(entry.route.targets[0].model.as_deref(), Some("qwen3"));
    }

    #[test]
    fn pinning_derives_slug_from_name_when_unset() {
        let snap = pinning_snapshot();
        let entry = snap.resolve_pinned("derived-name/m").expect("resolves");
        assert_eq!(entry.route.targets[0].provider, "derived-name");
        assert_eq!(entry.route.targets[0].model.as_deref(), Some("m"));
    }

    #[test]
    fn pinning_rejects_unknown_slug_and_malformed_addresses() {
        let snap = pinning_snapshot();
        assert!(snap.resolve_pinned("nope/model").is_none());
        assert!(snap.resolve_pinned("no-slash").is_none());
        assert!(snap.resolve_pinned("vllm-spb/").is_none()); // empty upstream
    }

    #[test]
    fn pinning_splits_on_the_first_slash_only() {
        let snap = pinning_snapshot();
        let entry = snap
            .resolve_pinned("vllm-spb/org/model:tag")
            .expect("resolves");
        // everything after the first '/' is the upstream model, verbatim
        assert_eq!(
            entry.route.targets[0].model.as_deref(),
            Some("org/model:tag")
        );
    }

    #[test]
    fn named_route_with_slash_is_indexed_and_shadows_pinning() {
        let snap = pinning_snapshot();
        // the route name containing '/' is a real route; the handler tries this
        // map before ever calling resolve_pinned, so the named route wins
        assert!(snap.routes.contains_key("vllm-spb/named-route"));
    }

    fn group_snapshot() -> Snapshot {
        use rolter_core::{GroupMember, ProviderGroupConfig};
        let mut config = GatewayConfig::default();
        config.providers.push(provider_cfg("vllm msk 1", None));
        config.providers.push(provider_cfg("vllm msk 2", None));
        // a group unifying both instances under one slug, passthrough models
        config.provider_groups.push(ProviderGroupConfig {
            name: "vLLM Cluster MSK".to_string(),
            slug: Some("vllm-cluster-msk".to_string()),
            strategy: rolter_core::BalancingStrategy::Weighted,
            members: vec![
                GroupMember {
                    provider: "vllm msk 1".to_string(),
                    model: None,
                    weight: 3,
                },
                GroupMember {
                    provider: "vllm msk 2".to_string(),
                    model: Some("qwen3-awq".to_string()),
                    weight: 1,
                },
            ],
        });
        // a group whose slug collides with a provider slug is dropped
        config.provider_groups.push(ProviderGroupConfig {
            name: "collision".to_string(),
            slug: Some("vllm-msk-1".to_string()),
            strategy: Default::default(),
            members: vec![GroupMember {
                provider: "vllm msk 1".to_string(),
                model: None,
                weight: 1,
            }],
        });
        // an empty group never routes
        config.provider_groups.push(ProviderGroupConfig {
            name: "empty".to_string(),
            slug: Some("empty-group".to_string()),
            strategy: Default::default(),
            members: Vec::new(),
        });
        Snapshot::build(&config, &crate::load::LoadTracker::new())
    }

    #[test]
    fn group_fans_out_across_members_with_passthrough_and_rewrite() {
        let snap = group_snapshot();
        let entry = snap
            .resolve_pinned("vllm-cluster-msk/qwen3")
            .expect("resolves");
        assert_eq!(entry.route.targets.len(), 2);
        // member without a rewrite forwards the requested model verbatim
        let m1 = &entry.route.targets[0];
        assert_eq!(m1.provider, "vllm msk 1");
        assert_eq!(m1.model.as_deref(), Some("qwen3"));
        assert_eq!(m1.weight, 3);
        // member with an explicit rewrite forwards its own upstream model
        let m2 = &entry.route.targets[1];
        assert_eq!(m2.provider, "vllm msk 2");
        assert_eq!(m2.model.as_deref(), Some("qwen3-awq"));
        assert_eq!(
            entry.route.strategy,
            rolter_core::BalancingStrategy::Weighted
        );
    }

    #[test]
    fn provider_slug_wins_a_collision_with_a_group_slug() {
        let snap = group_snapshot();
        // `vllm-msk-1` is a derived provider slug; the colliding group is dropped,
        // so the address pins the single provider (one target), not the group
        let entry = snap.resolve_pinned("vllm-msk-1/m").expect("resolves");
        assert_eq!(entry.route.targets.len(), 1);
        assert_eq!(entry.route.targets[0].provider, "vllm msk 1");
        assert!(!snap.groups_by_slug.contains_key("vllm-msk-1"));
    }

    #[test]
    fn empty_group_is_not_indexed() {
        let snap = group_snapshot();
        assert!(!snap.groups_by_slug.contains_key("empty-group"));
        assert!(snap.resolve_pinned("empty-group/m").is_none());
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
