//! Postgres-backed [`ConfigStore`], gated behind the `postgres` feature.

pub mod crypto;
pub mod models;
pub mod repo;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rolter_core::{
    BalancingStrategy, BudgetConfig, BudgetPeriod, BudgetScope, Error, GatewayConfig,
    ModelPriceConfig, ModelRoute, ProviderConfig, ProviderKind, RateLimitConfig, Result, Target,
    VirtualKeyRecord,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::postgres::models::{Budget, ModelPrice, RateLimit};
use crate::ConfigStore;

fn store_err(err: sqlx::Error) -> Error {
    Error::Store(err.to_string())
}

/// Connect to Postgres with a small pool sized for the control plane.
pub async fn connect(database_url: &str) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .map_err(store_err)
}

/// Run pending migrations against `pool`. The migration set lives in this
/// crate's own `migrations/` directory so it is embedded at compile time and
/// packaged with the published crate; `docker-compose` mounts the same dir for
/// its initdb bootstrap.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|err| Error::Store(err.to_string()))
}

#[derive(FromRow)]
struct ProviderRow {
    name: String,
    kind: String,
    api_base: String,
    api_key_env: Option<String>,
    egress_proxy: Option<String>,
    /// sealed runtime credential from `provider_keys`, when one is stored
    ciphertext: Option<Vec<u8>>,
    nonce: Option<Vec<u8>>,
}

impl ProviderRow {
    /// Convert a row into config, opening the sealed credential with `kek`
    /// when both are present. A missing KEK or an undecryptable credential
    /// degrades to `api_key: None` with a warning rather than failing the
    /// whole config load, so one bad key cannot take down snapshot serving.
    fn into_config(self, kek: Option<&crypto::Kek>) -> Result<ProviderConfig> {
        let row = self;
        let kind = match row.kind.as_str() {
            "openai" => ProviderKind::Openai,
            "anthropic" => ProviderKind::Anthropic,
            "openai_compatible" => ProviderKind::OpenaiCompatible,
            "ollama" => ProviderKind::Ollama,
            "ollama_cloud" => ProviderKind::OllamaCloud,
            "llama_cpp" => ProviderKind::LlamaCpp,
            "openrouter" => ProviderKind::Openrouter,
            "tei" => ProviderKind::Tei,
            "azure_openai" => ProviderKind::AzureOpenai,
            "bedrock" => ProviderKind::Bedrock,
            "vertex" => ProviderKind::Vertex,
            other => return Err(Error::Store(format!("unknown provider kind '{other}'"))),
        };
        let api_key = match (row.ciphertext.as_deref(), row.nonce.as_deref(), kek) {
            (Some(ciphertext), Some(nonce), Some(kek)) => match kek.decrypt(ciphertext, nonce) {
                Ok(plaintext) => Some(plaintext),
                Err(err) => {
                    tracing::warn!(provider = %row.name, error = %err,
                        "stored provider key could not be decrypted; serving provider without it");
                    None
                }
            },
            (Some(_), _, None) => {
                tracing::warn!(provider = %row.name,
                    "provider has a stored key but {} is unset; serving provider without it",
                    crypto::KEK_ENV);
                None
            }
            _ => None,
        };
        Ok(ProviderConfig {
            name: row.name,
            kind,
            api_base: row.api_base,
            api_key,
            api_key_env: row.api_key_env,
            egress_proxy: row.egress_proxy,
            api_keys: Vec::new(),
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
        })
    }
}

#[derive(FromRow)]
struct RouteRow {
    id: Uuid,
    model: String,
    strategy: String,
    params: serde_json::Value,
    param_policy: serde_json::Value,
}

#[derive(FromRow)]
struct TargetRow {
    route_id: Uuid,
    provider_name: String,
    upstream_model: Option<String>,
    weight: i32,
}

#[derive(FromRow)]
struct VirtualKeyRow {
    id: Uuid,
    key_hash: String,
    models: Vec<String>,
    disabled: bool,
    expires_at: Option<DateTime<Utc>>,
    cache_enabled: Option<bool>,
    project_id: Uuid,
    team_id: Uuid,
    org_id: Uuid,
}

fn parse_strategy(s: &str) -> Result<BalancingStrategy> {
    Ok(match s {
        "round_robin" => BalancingStrategy::RoundRobin,
        "random" => BalancingStrategy::Random,
        "power_of_two" => BalancingStrategy::PowerOfTwo,
        "consistent_hash" => BalancingStrategy::ConsistentHash,
        "cache_aware" => BalancingStrategy::CacheAware,
        "weighted" => BalancingStrategy::Weighted,
        "pipeline" => BalancingStrategy::Pipeline,
        "cheapest" => BalancingStrategy::Cheapest,
        "fastest" => BalancingStrategy::Fastest,
        other => {
            return Err(Error::Store(format!(
                "unknown balancing strategy '{other}'"
            )))
        }
    })
}

/// A [`ConfigStore`] backed by Postgres. `load` composes a [`GatewayConfig`]
/// from the `providers`, `routes`/`route_targets`, `model_prices` and
/// `virtual_keys` tables.
///
/// Virtual keys are exposed as [`rolter_core::VirtualKeyRecord`]s carrying only
/// the one-way `key_hash` plus scope identity — never the plaintext. Since the
/// gateway authenticates by peppered digest, the stored hash is sufficient to
/// verify presented keys (the control plane must hash with the same pepper).
pub struct PostgresConfigStore {
    pool: PgPool,
    /// key-encryption key for opening sealed provider credentials; read from
    /// [`crypto::KEK_ENV`] at construction. `None` serves providers without
    /// their stored keys (with a warning)
    kek: Option<crypto::Kek>,
}

impl PostgresConfigStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            kek: crypto::Kek::from_env(),
        }
    }

    /// Construct with an explicit KEK instead of reading the environment;
    /// mainly for tests, where mutating process-wide env vars races.
    pub fn with_kek(pool: PgPool, kek: Option<crypto::Kek>) -> Self {
        Self { pool, kek }
    }

    async fn load_providers(&self) -> Result<Vec<ProviderConfig>> {
        let rows: Vec<ProviderRow> = sqlx::query_as(
            "select p.name, p.kind, p.api_base, p.api_key_env, p.egress_proxy,
                    pk.ciphertext, pk.nonce
             from providers p
             left join provider_keys pk on pk.provider_id = p.id
             order by p.name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        rows.into_iter()
            .map(|row| row.into_config(self.kek.as_ref()))
            .collect()
    }

    async fn load_routes(&self) -> Result<Vec<ModelRoute>> {
        let route_rows: Vec<RouteRow> = sqlx::query_as(
            "select id, model, strategy, params, param_policy
             from routes where enabled order by model",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        let target_rows: Vec<TargetRow> = sqlx::query_as(
            "select rt.route_id, p.name as provider_name, rt.upstream_model, rt.weight
             from route_targets rt
             join providers p on p.id = rt.provider_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;

        route_rows
            .into_iter()
            .map(|r| {
                let strategy = parse_strategy(&r.strategy)?;
                let targets = target_rows
                    .iter()
                    .filter(|t| t.route_id == r.id)
                    .map(|t| Target {
                        provider: t.provider_name.clone(),
                        model: t.upstream_model.clone(),
                        weight: t.weight.max(0) as u32,
                    })
                    .collect();
                // jsonb → typed; a malformed value falls back to the permissive
                // default rather than failing the whole config load
                let params = serde_json::from_value(r.params).unwrap_or_default();
                let param_policy = serde_json::from_value(r.param_policy).unwrap_or_default();
                Ok(ModelRoute {
                    model: r.model,
                    strategy,
                    targets,
                    params,
                    param_policy,
                    // db-backed variants land with their own store follow-up
                    variants: Default::default(),
                    // response-cache opt-in is config-only for now; a db-backed
                    // cache policy lands with its own store follow-up
                    cache: None,
                })
            })
            .collect()
    }

    /// Load database-defined virtual keys with their resolved scope chain
    /// (project → team → org). Only the one-way `key_hash` is exposed; the
    /// gateway matches presented keys against it by peppered digest.
    async fn load_virtual_keys(&self) -> Result<Vec<VirtualKeyRecord>> {
        let rows: Vec<VirtualKeyRow> = sqlx::query_as(
            "select vk.id, vk.key_hash, vk.models, vk.disabled, vk.expires_at, \
                    vk.cache_enabled, vk.project_id, p.team_id, t.org_id \
             from virtual_keys vk \
             join projects p on p.id = vk.project_id \
             join teams t on t.id = p.team_id \
             order by vk.created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|r| VirtualKeyRecord {
                key_hash: r.key_hash,
                id: r.id.to_string(),
                org_id: r.org_id.to_string(),
                team_id: r.team_id.to_string(),
                project_id: r.project_id.to_string(),
                models: r.models,
                disabled: r.disabled,
                expires_at: r.expires_at,
                cache: r.cache_enabled,
            })
            .collect())
    }

    async fn load_model_prices(&self) -> Result<Vec<ModelPriceConfig>> {
        let rows: Vec<ModelPrice> = sqlx::query_as(
            "select id, model, input_per_mtok, output_per_mtok, cached_input_per_mtok, currency, created_at
             from model_prices order by model",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .map(|r| ModelPriceConfig {
                model: r.model,
                // decimals are stored as text; a malformed value prices at zero
                input_per_mtok: r.input_per_mtok.parse().unwrap_or(0.0),
                output_per_mtok: r.output_per_mtok.parse().unwrap_or(0.0),
                cached_input_per_mtok: r.cached_input_per_mtok.and_then(|v| v.parse().ok()),
            })
            .collect())
    }

    async fn load_budgets(&self) -> Result<Vec<BudgetConfig>> {
        let rows: Vec<Budget> = sqlx::query_as(
            "select id, scope_type, scope_id, limit_usd, period, created_at
             from budgets order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let scope = match r.scope_type.as_str() {
                    "org" => BudgetScope::Org,
                    "team" => BudgetScope::Team,
                    "project" => BudgetScope::Project,
                    "virtual_key" => BudgetScope::Key,
                    // unknown scope: skip rather than mis-enforce
                    _ => return None,
                };
                Some(BudgetConfig {
                    scope,
                    id: r.scope_id.to_string(),
                    // decimal stored as text; a malformed value disables the cap
                    limit_usd: r.limit_usd.parse().unwrap_or(f64::INFINITY),
                    period: parse_period(&r.period),
                })
            })
            .collect())
    }

    async fn load_rate_limits(&self) -> Result<Vec<RateLimitConfig>> {
        let rows: Vec<RateLimit> = sqlx::query_as(
            "select id, scope_type, scope_id, rpm, tpm, created_at
             from rate_limits order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let scope = match r.scope_type.as_str() {
                    "org" => BudgetScope::Org,
                    "team" => BudgetScope::Team,
                    "project" => BudgetScope::Project,
                    "virtual_key" => BudgetScope::Key,
                    // unknown scope: skip rather than mis-enforce
                    _ => return None,
                };
                Some(RateLimitConfig {
                    scope,
                    id: r.scope_id.to_string(),
                    // non-positive caps are meaningless; treat them as unset
                    rpm: r.rpm.filter(|v| *v > 0).map(|v| v as u32),
                    tpm: r.tpm.filter(|v| *v > 0).map(|v| v as u32),
                })
            })
            .collect())
    }
}

/// Map the free-text `budgets.period` column to a [`BudgetPeriod`]. Accepts both
/// the human names and the legacy duration shorthands (`1d`, `30d`), defaulting
/// to monthly for anything unrecognized.
fn parse_period(period: &str) -> BudgetPeriod {
    match period.trim().to_ascii_lowercase().as_str() {
        "daily" | "1d" | "24h" => BudgetPeriod::Daily,
        "total" | "lifetime" | "all" => BudgetPeriod::Total,
        _ => BudgetPeriod::Monthly,
    }
}

/// Read the current global config version. Bumping happens in the database
/// itself: migration 0003 installs triggers that increment the version
/// atomically with any write to providers/routes/route_targets/virtual_keys.
pub async fn current_version(pool: &PgPool) -> Result<i64> {
    sqlx::query_scalar("select version from config_version where id = 1")
        .fetch_one(pool)
        .await
        .map_err(store_err)
}

#[async_trait]
impl ConfigStore for PostgresConfigStore {
    async fn load(&self) -> Result<GatewayConfig> {
        let providers = self.load_providers().await?;
        let routes = self.load_routes().await?;
        let model_prices = self.load_model_prices().await?;
        let db_virtual_keys = self.load_virtual_keys().await?;
        let budgets = self.load_budgets().await?;
        let rate_limits = self.load_rate_limits().await?;
        Ok(GatewayConfig {
            providers,
            routes,
            model_prices,
            db_virtual_keys,
            budgets,
            rate_limits,
            ..GatewayConfig::default()
        })
    }

    async fn save(&self, _config: GatewayConfig) -> Result<()> {
        Err(Error::Store(
            "PostgresConfigStore is read-only; use the control-plane CRUD API to mutate providers/routes"
                .into(),
        ))
    }

    async fn current_version(&self) -> Result<i64> {
        sqlx::query_scalar("select version from config_version where id = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(store_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database_url() -> Option<String> {
        std::env::var("ROLTER_TEST_DATABASE_URL").ok()
    }

    async fn fresh_pool() -> PgPool {
        let url = database_url().expect("ROLTER_TEST_DATABASE_URL not set; skipping");
        let pool = connect(&url).await.expect("connect");
        // drop the whole schema (including sqlx's own _sqlx_migrations bookkeeping
        // table) so every test run re-applies migrations from a clean slate
        sqlx::query("drop schema public cascade")
            .execute(&pool)
            .await
            .expect("reset schema");
        sqlx::query("create schema public")
            .execute(&pool)
            .await
            .expect("recreate schema");
        run_migrations(&pool).await.expect("run migrations");
        pool
    }

    #[tokio::test]
    async fn triggers_bump_version_atomically_with_writes() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
        let v0 = current_version(&pool).await.unwrap();

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // orgs don't feed the gateway snapshot: no bump
        assert_eq!(current_version(&pool).await.unwrap(), v0);

        sqlx::query(
            "insert into providers (org_id, name, kind, api_base)
             values ($1, 'openai', 'openai', 'https://api.openai.com')",
        )
        .bind(org_id)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        // a rolled-back write must not bump the version
        let mut txn = pool.begin().await.unwrap();
        sqlx::query(
            "insert into providers (org_id, name, kind, api_base)
             values ($1, 'ghost', 'openai', 'https://ghost.example.com')",
        )
        .bind(org_id)
        .execute(&mut *txn)
        .await
        .unwrap();
        txn.rollback().await.unwrap();
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 1);

        sqlx::query("delete from providers where name = 'openai'")
            .execute(&pool)
            .await
            .unwrap();
        // the provider delete cascades to provider_keys, whose statement trigger
        // bumps the version even when no key rows exist
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 3);
    }

    #[tokio::test]
    async fn loads_providers_and_routes_from_db() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;

        let org_id: Uuid = sqlx::query_scalar(
            "insert into orgs (name, slug) values ('acme', 'acme') returning id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let team_id: Uuid =
            sqlx::query_scalar("insert into teams (org_id, name) values ($1, 'core') returning id")
                .bind(org_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let project_id: Uuid = sqlx::query_scalar(
            "insert into projects (team_id, name) values ($1, 'default') returning id",
        )
        .bind(team_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let provider_id: Uuid = sqlx::query_scalar(
            "insert into providers (org_id, name, kind, api_base, api_key_env)
             values ($1, 'openai', 'openai', 'https://api.openai.com', 'OPENAI_API_KEY')
             returning id",
        )
        .bind(org_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let route_id: Uuid = sqlx::query_scalar(
            "insert into routes (project_id, model, strategy) values ($1, 'gpt-4o', 'power_of_two') returning id",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "insert into route_targets (route_id, provider_id, upstream_model, weight)
             values ($1, $2, 'gpt-4o-2024-08-06', 2)",
        )
        .bind(route_id)
        .bind(provider_id)
        .execute(&pool)
        .await
        .unwrap();

        let store = PostgresConfigStore::new(pool);
        let config = store.load().await.unwrap();

        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "openai");
        assert_eq!(config.providers[0].kind, ProviderKind::Openai);
        assert_eq!(
            config.providers[0].api_key_env.as_deref(),
            Some("OPENAI_API_KEY")
        );

        assert_eq!(config.routes.len(), 1);
        assert_eq!(config.routes[0].model, "gpt-4o");
        assert_eq!(config.routes[0].strategy, BalancingStrategy::PowerOfTwo);
        assert_eq!(config.routes[0].targets.len(), 1);
        assert_eq!(config.routes[0].targets[0].provider, "openai");
        assert_eq!(
            config.routes[0].targets[0].model.as_deref(),
            Some("gpt-4o-2024-08-06")
        );
        assert_eq!(config.routes[0].targets[0].weight, 2);
    }

    #[tokio::test]
    async fn save_is_read_only() {
        let Some(_) = database_url() else {
            eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
            return;
        };
        let pool = fresh_pool().await;
        let store = PostgresConfigStore::new(pool);
        assert!(store.save(GatewayConfig::default()).await.is_err());
    }
}
