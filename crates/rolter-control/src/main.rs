//! rolter-control: the control plane.
//!
//! Hosts the management API used by the dashboard and serves the built SPA as
//! static assets. The MVP exposes health, a config read endpoint backed by an
//! in-memory store, and the role catalog; CRUD, RBAC enforcement, Postgres
//! persistence and Redis change publication are added in later phases.

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
    /// optional bootstrap config to seed the in-memory store (ignored if
    /// `--database-url`/`ROLTER_DATABASE_URL` is set)
    #[arg(short, long, env = "ROLTER_CONFIG")]
    config: Option<PathBuf>,
    /// postgres connection string; when set, the control plane reads/serves
    /// its config from the database instead of the bootstrap toml
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    database_url: Option<String>,
}

#[derive(Clone)]
struct ControlState {
    store: Arc<dyn ConfigStore>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    let args = Args::parse();

    let store: Arc<dyn ConfigStore> = build_store(&args).await?;
    let state = ControlState { store };

    let api = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/v1/ping",
            get(|| async { Json(json!({"pong": true})) }),
        )
        .route("/api/v1/roles", get(list_roles))
        .route("/api/v1/config", get(get_config))
        .route("/internal/snapshot", get(get_snapshot))
        .with_state(state);

    // anything not matched by the api falls through to the built SPA
    let app = api.fallback_service(ServeDir::new(&args.ui_dir));

    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    tracing::info!(%addr, ui_dir = %args.ui_dir.display(), "rolter-control listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the config store: postgres-backed when `--database-url` is set
/// (running migrations first), otherwise an in-memory store seeded from the
/// bootstrap toml.
async fn build_store(args: &Args) -> anyhow::Result<Arc<dyn ConfigStore>> {
    #[cfg(feature = "postgres")]
    if let Some(database_url) = &args.database_url {
        let pool = rolter_store::postgres::connect(database_url).await?;
        rolter_store::postgres::run_migrations(&pool).await?;
        return Ok(Arc::new(rolter_store::PostgresConfigStore::new(pool)));
    }

    let config = match &args.config {
        Some(path) if path.exists() => GatewayConfig::load(path)?,
        _ => GatewayConfig::default(),
    };
    Ok(Arc::new(InMemoryConfigStore::new(config)))
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
        Ok(config) => Json(json!({"version": version, "config": config})).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": {"message": err.to_string()}})),
        )
            .into_response(),
    }
}
