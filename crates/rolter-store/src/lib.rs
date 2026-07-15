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

/// Layers a read-only bootstrap config over a mutable inner store,
/// LiteLLM-style: file-declared providers/routes are "config models" (owned
/// by the file, immutable at runtime), inner-store rows are "DB models"
/// (full runtime CRUD). `load()` returns the merged view; on a name/model
/// collision the config entry wins and the DB entry is dropped from the
/// effective set. Writes pass through to the inner store, so config entries
/// can never be edited or deleted through it.
pub struct MergedConfigStore {
    bootstrap: GatewayConfig,
    inner: Arc<dyn ConfigStore>,
}

impl MergedConfigStore {
    /// Create a store merging `bootstrap` (config-owned, wins conflicts)
    /// over `inner` (runtime-owned).
    pub fn new(bootstrap: GatewayConfig, inner: Arc<dyn ConfigStore>) -> Self {
        Self { bootstrap, inner }
    }
}

#[async_trait]
impl ConfigStore for MergedConfigStore {
    async fn load(&self) -> Result<GatewayConfig> {
        let db = self.inner.load().await?;
        let mut merged = self.bootstrap.clone();
        merged.providers.extend(
            db.providers
                .into_iter()
                .filter(|p| !self.bootstrap.providers.iter().any(|c| c.name == p.name)),
        );
        merged.routes.extend(
            db.routes
                .into_iter()
                .filter(|r| !self.bootstrap.routes.iter().any(|c| c.model == r.model)),
        );
        merged.virtual_keys.extend(
            db.virtual_keys
                .into_iter()
                .filter(|k| !self.bootstrap.virtual_keys.iter().any(|c| c.key == k.key)),
        );
        Ok(merged)
    }

    async fn save(&self, config: GatewayConfig) -> Result<()> {
        self.inner.save(config).await
    }

    async fn current_version(&self) -> Result<i64> {
        self.inner.current_version().await
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

    fn route(model: &str) -> rolter_core::ModelRoute {
        rolter_core::ModelRoute {
            model: model.to_string(),
            strategy: Default::default(),
            targets: vec![],
            params: Default::default(),
            param_policy: Default::default(),
            cache: None,
            variants: Default::default(),
        }
    }

    fn provider(name: &str) -> rolter_core::ProviderConfig {
        rolter_core::ProviderConfig {
            name: name.to_string(),
            slug: None,
            kind: rolter_core::ProviderKind::Openai,
            api_base: "https://example.com".to_string(),
            api_key: None,
            api_key_env: None,
            egress_proxy: None,
            ca_bundles: None,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_config_wins_and_db_extends() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.providers.push(provider("openai"));
        bootstrap.routes.push(route("gpt-4o"));

        let mut db = GatewayConfig::default();
        // colliding entries: config must win, these must be dropped
        db.providers.push(provider("openai"));
        db.routes.push(route("gpt-4o"));
        // db-only additions: must appear in the merged view
        db.providers.push(provider("anthropic"));
        db.routes.push(route("claude"));

        let inner = Arc::new(InMemoryConfigStore::new(db));
        let store = MergedConfigStore::new(bootstrap, inner.clone());

        let merged = store.load().await.unwrap();
        assert_eq!(merged.providers.len(), 2);
        assert_eq!(merged.routes.len(), 2);
        let models: Vec<_> = merged.routes.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, vec!["gpt-4o", "claude"]);

        // runtime additions land in the inner store and show up without restart
        let mut updated = inner.load().await.unwrap();
        updated.routes.push(route("mistral"));
        store.save(updated).await.unwrap();
        assert_eq!(store.load().await.unwrap().routes.len(), 3);
        assert_eq!(store.current_version().await.unwrap(), 2);
    }
}
