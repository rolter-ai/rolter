use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use rolter_balancer::{build, LoadBalancer};
use rolter_core::{GatewayConfig, ModelRoute, ProviderConfig, VirtualKeyConfig};
use rolter_proxy::Forwarder;

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
    pub keys: HashMap<String, VirtualKeyConfig>,
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
        let keys = config
            .virtual_keys
            .iter()
            .cloned()
            .map(|k| (k.key.clone(), k))
            .collect();
        Self {
            providers,
            routes,
            keys,
        }
    }
}

/// Shared state handed to every request handler. Cheap to clone (all `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub snapshot: Arc<ArcSwap<Snapshot>>,
    pub forwarder: Arc<Forwarder>,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    /// Build state from an initial configuration.
    pub fn new(config: &GatewayConfig) -> Self {
        Self {
            snapshot: Arc::new(ArcSwap::from_pointee(Snapshot::build(config))),
            forwarder: Arc::new(Forwarder::new()),
            metrics: Arc::new(Metrics::default()),
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
