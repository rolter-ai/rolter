//! Storage abstractions for rolter.
//!
//! The MVP ships an in-memory [`ConfigStore`]. Postgres (source of truth),
//! Redis (cache + pub/sub) and ClickHouse (logs) backends implement the same
//! traits behind cargo features as the control plane is built out.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use rolter_core::{GatewayConfig, Result};

/// Read/write access to the gateway configuration.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    /// Load the current configuration snapshot.
    async fn load(&self) -> Result<GatewayConfig>;
    /// Persist a new configuration snapshot.
    async fn save(&self, config: GatewayConfig) -> Result<()>;
}

/// An in-memory [`ConfigStore`] for development and tests.
pub struct InMemoryConfigStore {
    inner: Arc<RwLock<GatewayConfig>>,
}

impl InMemoryConfigStore {
    /// Create a store seeded with `config`.
    pub fn new(config: GatewayConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
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
        Ok(())
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
