use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use rolter_balancer::{build, LoadBalancer};
use rolter_core::{GatewayConfig, ModelRoute, ProviderConfig, VirtualKeyConfig};
use rolter_proxy::Forwarder;

use crate::logging::LogSink;
use crate::metrics::Metrics;

/// A resolved route plus its constructed balancer.
pub struct RouteEntry {
    pub route: ModelRoute,
    pub balancer: Box<dyn LoadBalancer>,
}

/// Immutable routing state. Hot-reload swaps a whole new snapshot in atomically
/// so request handlers never block on a lock or observe a half-applied config.
pub struct Snapshot {
    pub providers: HashMap<String, ProviderConfig>,
    pub routes: HashMap<String, RouteEntry>,
    /// virtual keys indexed by their peppered digest ([`rolter_auth::hash_key`]),
    /// never by plaintext — the raw key is not retained in gateway memory
    pub keys: HashMap<String, VirtualKeyConfig>,
    /// deployment secret used to derive the key digests above
    pub pepper: String,
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
        let keys = config
            .virtual_keys
            .iter()
            .cloned()
            .map(|k| (rolter_auth::hash_key(&pepper, &k.key), k))
            .collect();
        Self {
            providers,
            routes,
            keys,
            pepper,
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
