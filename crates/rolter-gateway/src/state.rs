use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::{DateTime, Utc};
use rolter_balancer::{build, LoadBalancer};
use rolter_core::{GatewayConfig, ModelPriceConfig, ModelRoute, ProviderConfig};
use rolter_proxy::Forwarder;

use crate::logging::LogSink;
use crate::metrics::Metrics;

/// A resolved route plus its constructed balancer.
pub struct RouteEntry {
    pub route: ModelRoute,
    pub balancer: Box<dyn LoadBalancer>,
}

/// A virtual key as the request path sees it: identity/scope for attribution
/// plus the allow-list and validity window. Indexed by peppered digest in the
/// snapshot; the plaintext key is never retained.
#[derive(Debug, Clone, Default)]
pub struct KeyMeta {
    pub id: String,
    pub org_id: String,
    pub team_id: String,
    pub project_id: String,
    pub models: Vec<String>,
    pub disabled: bool,
    pub expires_at: Option<DateTime<Utc>>,
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
}

impl Snapshot {
    /// Build a snapshot from a configuration.
    pub fn build(config: &GatewayConfig) -> Self {
        let providers = config
            .providers
            .iter()
            .cloned()
            .map(|p| (p.name.clone(), p))
            .collect();
        let mut routes = HashMap::new();
        for route in &config.routes {
            let balancer = build(route.strategy, route.targets.len());
            routes.insert(
                route.model.clone(),
                RouteEntry {
                    route: route.clone(),
                    balancer,
                },
            );
        }
        let pepper = config.server.resolve_key_pepper();
        let mut keys: HashMap<String, KeyMeta> = HashMap::new();
        // config-defined keys: digest derived from the plaintext, no scope ids
        for k in &config.virtual_keys {
            keys.insert(
                rolter_auth::hash_key(&pepper, &k.key),
                KeyMeta {
                    models: k.models.clone(),
                    disabled: k.disabled,
                    expires_at: k.expires_at,
                    ..Default::default()
                },
            );
        }
        // database-defined keys: digest already stored, carry scope identity
        for k in &config.db_virtual_keys {
            keys.insert(
                k.key_hash.clone(),
                KeyMeta {
                    id: k.id.clone(),
                    org_id: k.org_id.clone(),
                    team_id: k.team_id.clone(),
                    project_id: k.project_id.clone(),
                    models: k.models.clone(),
                    disabled: k.disabled,
                    expires_at: k.expires_at,
                },
            );
        }
        let prices = config
            .model_prices
            .iter()
            .cloned()
            .map(|p| (p.model.clone(), p))
            .collect();
        Self {
            providers,
            routes,
            keys,
            pepper,
            prices,
        }
    }
}

/// Shared state handed to every request handler. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub forwarder: Arc<Forwarder>,
    pub metrics: Arc<Metrics>,
    pub log: LogSink,
}

impl AppState {
    /// Build state with logging disabled. Used by tests and any caller that
    /// does not need the ClickHouse writer.
    #[cfg(test)]
    pub fn new(config: &GatewayConfig) -> Self {
        let metrics = Arc::new(Metrics::default());
        let log = LogSink::disabled(metrics.clone());
        Self::assemble(config, metrics, log)
    }

    /// Build state and, when a `clickhouse_url` is configured, spawn the async
    /// batched log writer. Must be called from within a Tokio runtime.
    pub fn with_logging(config: &GatewayConfig) -> Self {
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
        Self::assemble(config, metrics, log)
    }

    fn assemble(config: &GatewayConfig, metrics: Arc<Metrics>, log: LogSink) -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::build(config))),
            forwarder: Arc::new(Forwarder::new()),
            metrics,
            log,
        }
    }

    /// Atomically replace the routing snapshot (used by the config watcher).
    /// Records `version` in metrics and bumps the reload counter.
    pub fn reload(&self, config: &GatewayConfig, version: u64) {
        self.snapshot.store(Arc::new(Snapshot::build(config)));
        self.metrics
            .config_version
            .store(version, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .config_reloads_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}
