//! rolter-control: the control plane.
//!
//! Hosts the management API used by the dashboard and serves the built SPA as
//! static assets. The MVP exposes health, a config read endpoint backed by an
//! in-memory store, and the role catalog; CRUD, RBAC enforcement, Postgres
//! persistence and Redis change publication are added in later phases.

#[cfg(feature = "postgres")]
mod crud;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::services::ServeDir;

use rolter_auth::Role;
use rolter_core::GatewayConfig;
#[cfg(feature = "postgres")]
use rolter_store::MergedConfigStore;
use rolter_store::{ConfigStore, InMemoryConfigStore};

#[derive(Parser, Debug)]
#[command(name = "rolter-control", version, about = "rolter control plane")]
struct Args {
    #[arg(long, env = "ROLTER_CONTROL_HOST", default_value = "0.0.0.0")]
    host: String,
    #[arg(long, env = "ROLTER_CONTROL_PORT", default_value_t = 4001)]
    port: u16,
    /// directory holding the built UI (index.html + assets)
    #[arg(long, env = "ROLTER_UI_DIR", default_value = "ui/dist")]
    ui_dir: PathBuf,
    /// optional bootstrap config. Without a database it seeds the in-memory
    /// store; with `--database-url` its providers/routes become read-only
    /// "config models" merged over the DB-defined ones (config wins on
    /// name conflicts)
    #[arg(short, long, env = "ROLTER_CONFIG")]
    config: Option<PathBuf>,
    /// postgres connection string; when set, the control plane reads/serves
    /// its config from the database instead of the bootstrap toml
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    database_url: Option<String>,
    /// redis connection url; when set, config-version bumps are published on
    /// the `rolter.config` channel so gateways refetch immediately instead of
    /// waiting for their poll interval
    #[arg(long, env = "ROLTER_REDIS_URL")]
    redis_url: Option<String>,
}

/// Names owned by the bootstrap config file: immutable at runtime,
/// LiteLLM-style. The CRUD API rejects mutations that collide with them.
// only read by the postgres-gated CRUD module
#[cfg_attr(not(feature = "postgres"), allow(dead_code))]
#[derive(Default)]
struct ConfigOwned {
    providers: std::collections::HashSet<String>,
    models: std::collections::HashSet<String>,
}

impl ConfigOwned {
    fn from_config(config: &GatewayConfig) -> Self {
        Self {
            providers: config.providers.iter().map(|p| p.name.clone()).collect(),
            models: config.routes.iter().map(|r| r.model.clone()).collect(),
        }
    }
}

#[derive(Clone)]
struct ControlState {
    store: Arc<dyn ConfigStore>,
    /// provider/model names declared in the bootstrap config; read-only via
    /// the API (empty when no bootstrap config was given)
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    config_owned: Arc<ConfigOwned>,
    /// set when `--redis-url` is configured; config-version bumps are
    /// published on [`rolter_core::CONFIG_CHANNEL`] (best-effort)
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    redis: Option<redis::Client>,
    /// set when `--database-url` is configured; backs the CRUD API, which
    /// needs direct repository access beyond what `ConfigStore` exposes
    #[cfg(feature = "postgres")]
    pool: Option<sqlx::PgPool>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    let args = Args::parse();

    let bootstrap = match &args.config {
        Some(path) if path.exists() => Some(GatewayConfig::load(path)?),
        _ => None,
    };
    let config_owned = Arc::new(
        bootstrap
            .as_ref()
            .map(ConfigOwned::from_config)
            .unwrap_or_default(),
    );

    let redis = match &args.redis_url {
        Some(url) => match redis::Client::open(url.as_str()) {
            Ok(client) => {
                tracing::info!(%url, "publishing config bumps to redis");
                Some(client)
            }
            Err(err) => {
                tracing::warn!(error = %err, "invalid redis url; config pub/sub disabled");
                None
            }
        },
        None => None,
    };

    #[allow(unused_variables)]
    let (store, pool) = build_store(&args, bootstrap).await?;
    #[cfg(feature = "postgres")]
    let state = ControlState {
        store,
        config_owned,
        redis,
        pool: pool.clone(),
    };
    #[cfg(not(feature = "postgres"))]
    let state = ControlState {
        store,
        config_owned,
        redis,
    };

    #[allow(unused_mut)]
    let mut api = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/v1/ping",
            get(|| async { Json(json!({"pong": true})) }),
        )
        .route("/api/v1/roles", get(list_roles))
        .route("/api/v1/config", get(get_config))
        .route("/internal/snapshot", get(get_snapshot));

    #[cfg(feature = "postgres")]
    if pool.is_some() {
        api = api.merge(crud::router());
    }

    let app = api
        .with_state(state)
        // anything not matched by the api falls through to the built SPA
        .fallback_service(ServeDir::new(&args.ui_dir));

    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    tracing::info!(%addr, ui_dir = %args.ui_dir.display(), "rolter-control listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the config store: postgres-backed when `--database-url` is set
/// (running migrations first; a bootstrap config, when given, is layered on
/// top as read-only config models via [`MergedConfigStore`]), otherwise an
/// in-memory store seeded from the bootstrap toml. Also returns the raw pool
/// (postgres builds only), which the CRUD API needs for direct repository
/// access.
#[cfg(feature = "postgres")]
async fn build_store(
    args: &Args,
    bootstrap: Option<GatewayConfig>,
) -> anyhow::Result<(Arc<dyn ConfigStore>, Option<sqlx::PgPool>)> {
    if let Some(database_url) = &args.database_url {
        let pool = rolter_store::postgres::connect(database_url).await?;
        rolter_store::postgres::run_migrations(&pool).await?;
        let db_store: Arc<dyn ConfigStore> =
            Arc::new(rolter_store::PostgresConfigStore::new(pool.clone()));
        let store: Arc<dyn ConfigStore> = match bootstrap {
            Some(config) => Arc::new(MergedConfigStore::new(config, db_store)),
            None => db_store,
        };
        return Ok((store, Some(pool)));
    }

    let config = bootstrap.unwrap_or_default();
    Ok((Arc::new(InMemoryConfigStore::new(config)), None))
}

#[cfg(not(feature = "postgres"))]
async fn build_store(
    _args: &Args,
    bootstrap: Option<GatewayConfig>,
) -> anyhow::Result<(Arc<dyn ConfigStore>, Option<()>)> {
    let config = bootstrap.unwrap_or_default();
    Ok((Arc::new(InMemoryConfigStore::new(config)), None))
}

async fn list_roles() -> Json<Value> {
    let roles = [Role::Admin, Role::Member, Role::Viewer];
    Json(serde_json::to_value(roles).unwrap_or_default())
}

async fn get_config(State(state): State<ControlState>) -> Json<GatewayConfig> {
    let config = state.store.load().await.unwrap_or_default();
    Json(config)
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    /// the gateway's last-seen config version; if it's already current, the
    /// control plane replies `304 Not Modified` with no body
    version: Option<i64>,
}

/// Runtime snapshot endpoint gateways poll to pick up config changes without
/// a restart. Returns `{"version": N, "config": GatewayConfig}`, or `304` if
/// the caller's `version` is already current.
async fn get_snapshot(
    State(state): State<ControlState>,
    Query(query): Query<SnapshotQuery>,
) -> Response {
    let version = match state.store.current_version().await {
        Ok(v) => v,
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"message": err.to_string()}})),
            )
                .into_response()
        }
    };
    if query.version.is_some_and(|requested| requested >= version) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    match state.store.load().await {
        Ok(config) => {
            // never distribute a broken config to gateways: reply with the
            // problems instead so the operator can fix the source
            if let Err(problems) = config.validate() {
                tracing::error!(?problems, "refusing to serve invalid config snapshot");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": {
                        "message": "config failed validation",
                        "problems": problems,
                    }})),
                )
                    .into_response();
            }
            Json(json!({"version": version, "config": config})).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": err.to_string()}})),
        )
            .into_response(),
    }
}
