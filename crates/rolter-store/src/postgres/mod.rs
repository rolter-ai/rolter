//! Postgres-backed [`ConfigStore`], gated behind the `postgres` feature.

pub mod models;
pub mod repo;

use async_trait::async_trait;
use rolter_core::{
    BalancingStrategy, Error, GatewayConfig, ModelPriceConfig, ModelRoute, ProviderConfig,
    ProviderKind, Result, Target,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::postgres::models::ModelPrice;
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

/// Run pending migrations against `pool`. The migration set lives in the
/// repo-root `migrations/` directory, shared with the `docker-compose` initdb
/// bootstrap.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("../../migrations")
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
}

impl TryFrom<ProviderRow> for ProviderConfig {
    type Error = Error;

    fn try_from(row: ProviderRow) -> Result<Self> {
        let kind = match row.kind.as_str() {
            "openai" => ProviderKind::Openai,
            "anthropic" => ProviderKind::Anthropic,
            "openai_compatible" => ProviderKind::OpenaiCompatible,
            other => return Err(Error::Store(format!("unknown provider kind '{other}'"))),
        };
        Ok(ProviderConfig {
            name: row.name,
            kind,
            api_base: row.api_base,
            api_key: None,
            api_key_env: row.api_key_env,
            egress_proxy: row.egress_proxy,
        })
    }
}

#[derive(FromRow)]
struct RouteRow {
    id: Uuid,
    model: String,
    strategy: String,
}

#[derive(FromRow)]
struct TargetRow {
    route_id: Uuid,
    provider_name: String,
    upstream_model: Option<String>,
    weight: i32,
}

fn parse_strategy(s: &str) -> Result<BalancingStrategy> {
    Ok(match s {
        "round_robin" => BalancingStrategy::RoundRobin,
        "random" => BalancingStrategy::Random,
        "power_of_two" => BalancingStrategy::PowerOfTwo,
        "consistent_hash" => BalancingStrategy::ConsistentHash,
        "cache_aware" => BalancingStrategy::CacheAware,
        other => {
            return Err(Error::Store(format!(
                "unknown balancing strategy '{other}'"
            )))
        }
    })
}

/// A [`ConfigStore`] backed by Postgres. `load` composes a [`GatewayConfig`]
/// from the `providers` and `routes`/`route_targets` tables.
///
/// Virtual keys are intentionally **not** reconstructed here: `virtual_keys`
/// stores only a one-way hash of each key (`key_hash`), so the plaintext
/// needed to populate [`rolter_core::VirtualKeyConfig::key`] cannot be
/// recovered from the database. Hash-based key verification in the gateway
/// request path is tracked separately (virtual-key hardening).
pub struct PostgresConfigStore {
    pool: PgPool,
}

impl PostgresConfigStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn load_providers(&self) -> Result<Vec<ProviderConfig>> {
        let rows: Vec<ProviderRow> = sqlx::query_as(
            "select name, kind, api_base, api_key_env, egress_proxy from providers order by name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(store_err)?;
        rows.into_iter().map(ProviderConfig::try_from).collect()
    }

    async fn load_routes(&self) -> Result<Vec<ModelRoute>> {
        let route_rows: Vec<RouteRow> =
            sqlx::query_as("select id, model, strategy from routes where enabled order by model")
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
                Ok(ModelRoute {
                    model: r.model,
                    strategy,
                    targets,
                })
            })
            .collect()
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
        Ok(GatewayConfig {
            providers,
            routes,
            model_prices,
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
        assert_eq!(current_version(&pool).await.unwrap(), v0 + 2);
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
