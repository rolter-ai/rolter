//! rolter-gateway: the data-plane proxy.
//!
//! Loads a bootstrap config, builds an atomically-swappable routing snapshot and
//! serves OpenAI- and Anthropic-compatible endpoints, balancing across upstream
//! targets and streaming responses straight back to clients.

mod fake_llm;
mod handlers;
mod metrics;
mod state;
mod watcher;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use tower_http::trace::TraceLayer;

use rolter_core::GatewayConfig;
use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "rolter-gateway", version, about = "rolter data-plane gateway")]
struct Args {
    /// path to the bootstrap config file
    #[arg(short, long, env = "ROLTER_CONFIG", default_value = "rolter.toml")]
    config: PathBuf,
    /// override the bind host
    #[arg(long, env = "ROLTER_HOST")]
    host: Option<String>,
    /// override the bind port
    #[arg(long, env = "ROLTER_PORT")]
    port: Option<u16>,
    /// control-plane snapshot endpoint to poll for reload-free config
    /// updates, e.g. `http://control:4001/internal/snapshot`; polling is
    /// disabled when unset
    #[arg(long, env = "ROLTER_SNAPSHOT_URL")]
    snapshot_url: Option<String>,
    /// how often to poll the snapshot endpoint, in seconds
    #[arg(long, env = "ROLTER_SNAPSHOT_POLL_SECS", default_value_t = 5)]
    snapshot_poll_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rolter_core::telemetry::init();
    let args = Args::parse();

    let mut config = if args.config.exists() {
        GatewayConfig::load(&args.config)?
    } else {
        tracing::warn!(path = %args.config.display(), "config file not found, starting with empty config");
        GatewayConfig::default()
    };
    if let Some(host) = args.host {
        config.server.host = host;
    }
    if let Some(port) = args.port {
        config.server.port = port;
    }

    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;
    let state = AppState::new(&config);

    // start the reload-free config watcher when a control plane is configured
    if let Some(snapshot_url) = args.snapshot_url {
        let period = std::time::Duration::from_secs(args.snapshot_poll_secs.max(1));
        tracing::info!(%snapshot_url, poll_secs = args.snapshot_poll_secs, "config watcher enabled");
        watcher::spawn(state.clone(), snapshot_url, period);
    } else {
        tracing::info!("no snapshot url configured; running with static bootstrap config");
    }

    let app = Router::new()
        .route("/healthz", get(handlers::healthz))
        .route("/metrics", get(handlers::metrics))
        .route("/v1/models", get(handlers::list_models))
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/completions", post(handlers::completions))
        .route("/v1/messages", post(handlers::messages))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!(%addr, "rolter-gateway listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
