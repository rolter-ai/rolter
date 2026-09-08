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
        // provider groups: readonly config groups win a slug collision, db groups
        // extend the effective set (ADR-0022). compare on the effective slug
        let group_slug = |g: &rolter_core::ProviderGroupConfig| {
            g.slug
                .clone()
                .unwrap_or_else(|| rolter_core::slug::slugify(&g.name))
        };
        let bootstrap_group_slugs: std::collections::HashSet<String> = self
            .bootstrap
            .provider_groups
            .iter()
            .map(group_slug)
            .collect();
        merged.provider_groups.extend(
            db.provider_groups
                .into_iter()
                .filter(|g| !bootstrap_group_slugs.contains(&group_slug(g))),
        );
        merged.virtual_keys.extend(
            db.virtual_keys
                .into_iter()
                .filter(|k| !self.bootstrap.virtual_keys.iter().any(|c| c.key == k.key)),
        );
        // db-only snapshot fields (#623): the inner store populates these, the
        // bootstrap toml does not own them, so they must be carried through or the
        // gateway silently loses every runtime virtual key, price, budget and
        // rate limit whenever a bootstrap config is present. same "bootstrap wins
        // a collision, db extends the rest" rule as above.
        merged.db_virtual_keys.extend(
            db.db_virtual_keys
                .into_iter()
                .filter(|k| !self.bootstrap.db_virtual_keys.iter().any(|c| c.id == k.id)),
        );
        // MCP authorization is database-owned. Never let a bootstrap file
        // override server policy or inject credential-bearing sessions into a
        // control-plane-managed snapshot.
        merged.mcp_servers = db.mcp_servers;
        merged.mcp_oauth_sessions = db.mcp_oauth_sessions;
        merged
            .model_prices
            .extend(db.model_prices.into_iter().filter(|p| {
                !self
                    .bootstrap
                    .model_prices
                    .iter()
                    .any(|c| c.model == p.model)
            }));
        merged.budgets.extend(
            db.budgets
                .into_iter()
                .filter(|b| !self.bootstrap.budgets.iter().any(|c| c.id == b.id)),
        );
        merged.rate_limits.extend(
            db.rate_limits
                .into_iter()
                .filter(|r| !self.bootstrap.rate_limits.iter().any(|c| c.id == r.id)),
        );
        // runtime policies are database-owned and hot reloadable. Carry them
        // through even when a bootstrap file supplies immutable providers and
        // routes, then apply the global gates to the complete effective route set.
        merged.retry = db.retry;
        merged.timeouts = db.timeouts;
        merged.queue = db.queue;
        merged.compatibility = db.compatibility;
        merged.adaptive_routing = db.adaptive_routing;
        merged.logging = db.logging;
        // file-owned guardrail rules remain immutable and win a name collision;
        // registry rules extend the ordered policy. This keeps adopting the
        // dashboard from erasing an existing deployment policy. Likewise, an
        // enabled file-owned webhook remains authoritative; the registry owns
        // the effective webhook only when the bootstrap hook is disabled.
        merged.guardrails.enabled |= db.guardrails.enabled;
        merged
            .guardrails
            .rules
            .extend(db.guardrails.rules.into_iter().filter(|rule| {
                !self
                    .bootstrap
                    .guardrails
                    .rules
                    .iter()
                    .any(|item| item.name == rule.name)
            }));
        if !self.bootstrap.guardrail_webhook.enabled {
            merged.guardrail_webhook = db.guardrail_webhook;
        }
        merged.feature_flags = db.feature_flags;
        merged.apply_feature_flags();
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
            advanced: Default::default(),
            cache: None,
            variants: Default::default(),
        }
    }

    fn provider(name: &str) -> rolter_core::ProviderConfig {
        rolter_core::ProviderConfig {
            name: name.to_string(),
            kind: rolter_core::ProviderKind::Openai,
            api_base: "https://example.com".to_string(),
            ..Default::default()
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

    fn group(slug: &str) -> rolter_core::ProviderGroupConfig {
        rolter_core::ProviderGroupConfig {
            name: slug.to_string(),
            slug: Some(slug.to_string()),
            strategy: Default::default(),
            members: vec![rolter_core::GroupMember {
                provider: "a".to_string(),
                model: None,
                weight: 1,
            }],
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_readonly_group_wins_and_db_groups_extend() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.provider_groups.push(group("vllm-cluster"));

        let mut db = GatewayConfig::default();
        db.provider_groups.push(group("vllm-cluster")); // collides → dropped
        db.provider_groups.push(group("vllm-nsk")); // db-only → kept

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();
        let slugs: Vec<_> = merged
            .provider_groups
            .iter()
            .map(|g| g.slug.as_deref().unwrap())
            .collect();
        assert_eq!(slugs, vec!["vllm-cluster", "vllm-nsk"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_preserves_file_guardrails_and_extends_registry_rules() {
        let rule = |name: &str| rolter_core::GuardrailRule {
            name: name.to_string(),
            builtin: Some(rolter_core::BuiltinRule::Email),
            pattern: None,
            stage: Default::default(),
            action: Default::default(),
            replacement: None,
            include_system: false,
        };
        let mut bootstrap = GatewayConfig::default();
        bootstrap.guardrails.enabled = true;
        bootstrap.guardrails.rules.push(rule("file-rule"));
        let mut db = GatewayConfig::default();
        db.guardrails.enabled = true;
        db.guardrails.rules.push(rule("file-rule"));
        db.guardrails.rules.push(rule("registry-rule"));

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let names: Vec<_> = store
            .load()
            .await
            .unwrap()
            .guardrails
            .rules
            .into_iter()
            .map(|rule| rule.name)
            .collect();
        assert_eq!(names, vec!["file-rule", "registry-rule"]);
    }

    fn db_vkey(id: &str, hash: &str) -> rolter_core::VirtualKeyRecord {
        rolter_core::VirtualKeyRecord {
            key_hash: hash.to_string(),
            id: id.to_string(),
            org_id: String::new(),
            team_id: String::new(),
            project_id: String::new(),
            user_id: String::new(),
            models: vec![],
            providers: vec![],
            disabled: false,
            expires_at: None,
            cache: None,
            business_unit_id: String::new(),
            customer_id: String::new(),
            access_policy: None,
        }
    }

    fn price_val(model: &str, input: f64) -> rolter_core::ModelPriceConfig {
        rolter_core::ModelPriceConfig {
            model: model.to_string(),
            input_per_mtok: input,
            output_per_mtok: 0.0,
            cached_input_per_mtok: None,
            currency: "USD".to_string(),
        }
    }

    fn budget(id: &str) -> rolter_core::BudgetConfig {
        rolter_core::BudgetConfig {
            scope: rolter_core::BudgetScope::Org,
            id: id.to_string(),
            limit_usd: 10.0,
            period: Default::default(),
            unpriced_policy: None,
        }
    }

    fn rate_limit(id: &str) -> rolter_core::RateLimitConfig {
        rolter_core::RateLimitConfig {
            scope: rolter_core::BudgetScope::Org,
            id: id.to_string(),
            rpm: Some(60),
            tpm: None,
        }
    }

    // regression for #623: a DB-backed inner store populates db_virtual_keys,
    // model_prices, budgets and rate_limits — none of which the bootstrap toml
    // owns. the merge must carry them through, or runtime virtual keys 401 at
    // the gateway (and prices/budgets/limits silently vanish) whenever a
    // bootstrap config is present.
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_carries_db_only_snapshot_fields() {
        let bootstrap = GatewayConfig::default();

        let mut db = GatewayConfig::default();
        db.db_virtual_keys.push(db_vkey("vk1", "hash-1"));
        db.model_prices.push(price_val("gpt-4o", 3.0));
        db.budgets.push(budget("b1"));
        db.rate_limits.push(rate_limit("rl1"));
        db.mcp_servers.push(rolter_core::McpServerConfig {
            id: "mcp1".to_string(),
            org_id: "org1".to_string(),
            slug: "docs".to_string(),
            url: "https://mcp.example.com".to_string(),
            transport: "streamable_http".to_string(),
            required_scopes: vec!["tools:read".to_string()],
        });
        db.mcp_oauth_sessions
            .push(rolter_core::McpOAuthSessionConfig {
                id: "session1".to_string(),
                server_id: "mcp1".to_string(),
                user_id: "user1".to_string(),
                scopes: vec!["tools:read".to_string()],
                expires_at: "2099-01-01T00:00:00Z".parse().unwrap(),
                access_token: "secret".to_string(),
            });

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();

        assert_eq!(merged.db_virtual_keys.len(), 1, "db virtual key dropped");
        assert_eq!(merged.db_virtual_keys[0].key_hash, "hash-1");
        assert_eq!(merged.model_prices.len(), 1, "db model price dropped");
        assert_eq!(merged.budgets.len(), 1, "db budget dropped");
        assert_eq!(merged.rate_limits.len(), 1, "db rate limit dropped");
        assert_eq!(merged.mcp_servers.len(), 1, "db MCP server dropped");
        assert_eq!(merged.mcp_oauth_sessions.len(), 1, "db MCP session dropped");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_applies_db_runtime_policies_and_feature_gates() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.cache.enabled = true;
        bootstrap.routes.push(route("cached"));
        bootstrap.routes[0].strategy = rolter_core::BalancingStrategy::CacheAware;

        let mut db = GatewayConfig::default();
        db.retry.max_retries = 7;
        db.timeouts.request_secs = 123;
        db.queue.capacity = 999;
        db.logging.sample_rate = 0.25;
        db.feature_flags.response_cache = false;
        db.feature_flags.cache_aware_routing = false;

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();

        assert_eq!(merged.retry.max_retries, 7);
        assert_eq!(merged.timeouts.request_secs, 123);
        assert_eq!(merged.queue.capacity, 999);
        assert_eq!(merged.logging.sample_rate, 0.25);
        assert!(!merged.cache.enabled);
        assert_eq!(
            merged.routes[0].strategy,
            rolter_core::BalancingStrategy::PowerOfTwo
        );
    }

    // the db-only fields follow the same "bootstrap wins a collision, db
    // extends the rest" rule as providers/routes/groups (#623 audit).
    #[tokio::test(flavor = "current_thread")]
    async fn merged_store_bootstrap_wins_over_db_prices_and_limits() {
        let mut bootstrap = GatewayConfig::default();
        bootstrap.model_prices.push(price_val("gpt-4o", 1.0));
        bootstrap.budgets.push(budget("b1"));
        bootstrap.rate_limits.push(rate_limit("rl1"));

        let mut db = GatewayConfig::default();
        db.model_prices.push(price_val("gpt-4o", 999.0)); // collides on model → dropped
        db.model_prices.push(price_val("claude", 2.0)); // db-only → kept
        db.budgets.push(budget("b1")); // collides on id → dropped
        db.budgets.push(budget("b2")); // db-only → kept
        db.rate_limits.push(rate_limit("rl1")); // collides on id → dropped
        db.rate_limits.push(rate_limit("rl2")); // db-only → kept

        let store = MergedConfigStore::new(bootstrap, Arc::new(InMemoryConfigStore::new(db)));
        let merged = store.load().await.unwrap();

        let prices: std::collections::HashMap<_, _> = merged
            .model_prices
            .iter()
            .map(|p| (p.model.clone(), p.input_per_mtok))
            .collect();
        assert_eq!(prices.get("gpt-4o"), Some(&1.0), "config price must win");
        assert_eq!(
            prices.get("claude"),
            Some(&2.0),
            "db-only price must survive"
        );
        assert_eq!(merged.model_prices.len(), 2);
        assert_eq!(merged.budgets.len(), 2);
        assert_eq!(merged.rate_limits.len(), 2);
    }
}
