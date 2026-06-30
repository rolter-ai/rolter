//! rolter-control: the control plane.
//!
//! Hosts the management API used by the dashboard and serves the built SPA as
//! static assets. The MVP exposes health, a config read endpoint backed by an
//! in-memory store, and the role catalog; CRUD, RBAC enforcement, Postgres
//! persistence and Redis change publication are added in later phases.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
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
    /// optional bootstrap config to seed the in-memory store
    #[arg(short, long, env = "ROLTER_CONFIG")]
    config: Option<PathBuf>,
}

#[derive(Clone)]
struct ControlState {
    store: Arc<InMemoryConfigStore>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    let args = Args::parse();

    let config = match &args.config {
        Some(path) if path.exists() => GatewayConfig::load(path)?,
        _ => GatewayConfig::default(),
    };
    let state = ControlState {
        store: Arc::new(InMemoryConfigStore::new(config)),
    };

    let api = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/v1/ping",
            get(|| async { Json(json!({"pong": true})) }),
        )
        .route("/api/v1/roles", get(list_roles))
        .route("/api/v1/config", get(get_config))
        .with_state(state);

    // anything not matched by the api falls through to the built SPA
    let app = api.fallback_service(ServeDir::new(&args.ui_dir));

    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    tracing::info!(%addr, ui_dir = %args.ui_dir.display(), "rolter-control listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn list_roles() -> Json<Value> {
    let roles = [Role::Admin, Role::Member, Role::Viewer];
    Json(serde_json::to_value(roles).unwrap_or_default())
}

async fn get_config(State(state): State<ControlState>) -> Json<GatewayConfig> {
    let config = state.store.load().await.unwrap_or_default();
    Json(config)
}
