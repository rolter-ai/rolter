//! Storage abstractions for rolter.
//!
//! The MVP ships an in-memory [`ConfigStore`]. Postgres (source of truth),
//! Redis (cache + pub/sub) and ClickHouse (logs) backends implement the same
//! traits behind cargo features as the control plane is built out.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use rolter_core::{GatewayConfig, Result};

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "postgres")]
pub use postgres::PostgresConfigStore;

/// Read/write access to the gateway configuration.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Load the current configuration snapshot.
    async fn load(&self) -> Result<GatewayConfig>;
    /// Persist a new configuration snapshot.
    async fn save(&self, config: GatewayConfig) -> Result<()>;
    /// The store's current config version, bumped on every write. Gateways
    /// poll this (see `GET /internal/snapshot?version=N` in rolter-control)
    /// to decide whether a fresh snapshot needs fetching.
    async fn current_version(&self) -> Result<i64> {
        Ok(1)
    }
}

/// An in-memory [`ConfigStore`] for development and tests.
pub struct InMemoryConfigStore {
    inner: Arc<RwLock<GatewayConfig>>,
    version: AtomicI64,
}

impl InMemoryConfigStore {
    /// Create a store seeded with `config`.
    pub fn new(config: GatewayConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
            version: AtomicI64::new(1),
        }
    }
}

#[async_trait]
impl ConfigStore for InMemoryConfigStore {
    async fn load(&self) -> Result<GatewayConfig> {
        Ok(self.inner.read().clone())
    }

    async fn save(&self, config: GatewayConfig) -> Result<()> {
        *self.inner.write() = config;
        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn current_version(&self) -> Result<i64> {
        Ok(self.version.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn roundtrips_config() {
        // note: tokio is pulled in transitively only for the test harness here;
        // keep this test self-contained without external services.
        let store = InMemoryConfigStore::new(GatewayConfig::default());
        let mut cfg = store.load().await.unwrap();
        cfg.server.port = 9999;
        store.save(cfg).await.unwrap();
        assert_eq!(store.load().await.unwrap().server.port, 9999);
    }
}
