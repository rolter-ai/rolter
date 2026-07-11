//! rolter-gateway: the data-plane proxy.
//!
//! Loads a bootstrap config, builds an atomically-swappable routing snapshot and
//! serves OpenAI- and Anthropic-compatible endpoints, balancing across upstream
//! targets and streaming responses straight back to clients.
//!
//! The binary is a thin wrapper over [`run`]; the unified `rolter` launcher
//! reuses the same entrypoint as its `gateway` subcommand.

mod breaker;
mod budgets;
mod cooldowns;
mod error;
mod fake_llm;
mod handlers;
mod health;
mod health_events;
mod load;
mod logging;
mod metrics;
mod rate_limits;
mod state;
mod status_page;
mod trace;
mod upstream_metrics;
mod watcher;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use tower_http::trace::TraceLayer;

use rolter_core::GatewayConfig;
pub use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "rolter-gateway", version, about = "rolter data-plane gateway")]
pub struct Args {
    /// path to the bootstrap config file
    #[arg(short, long, env = "ROLTER_CONFIG", default_value = "rolter.toml")]
    pub config: PathBuf,
    /// override the bind host
    #[arg(long, env = "ROLTER_HOST")]
    pub host: Option<String>,
    /// override the bind port
    #[arg(long, env = "ROLTER_PORT")]
    pub port: Option<u16>,
    /// control-plane snapshot endpoint to poll for reload-free config
    /// updates, e.g. `http://control:4001/internal/snapshot`; polling is
    /// disabled when unset
    #[arg(long, env = "ROLTER_SNAPSHOT_URL")]
    pub snapshot_url: Option<String>,
    /// how often to poll the snapshot endpoint, in seconds
    #[arg(long, env = "ROLTER_SNAPSHOT_POLL_SECS", default_value_t = 5)]
    pub snapshot_poll_secs: u64,
    /// redis connection url; when set (together with --snapshot-url), config
    /// bumps published by the control plane trigger an immediate refetch
    /// instead of waiting for the poll interval
    #[arg(long, env = "ROLTER_REDIS_URL")]
    pub redis_url: Option<String>,
}

/// Run the data-plane gateway to completion. The caller owns argument parsing
/// and telemetry initialization.
pub async fn run(args: Args) -> anyhow::Result<()> {
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

    if let Err(problems) = config.validate() {
        tracing::warn!(
            count = problems.len(),
            "bootstrap config failed validation; requests to affected routes will error"
        );
        // enumerate each problem on its own line so an operator can fix the whole
        // config in one pass rather than one restart per error
        for (i, problem) in problems.iter().enumerate() {
            tracing::warn!("  config problem {}/{}: {}", i + 1, problems.len(), problem);
        }
    }

    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port).parse()?;
    if let Some(url) = &config.logging.clickhouse_url {
        tracing::info!(%url, "clickhouse request logging enabled");
    }
    let state = AppState::with_logging(&config, args.redis_url.as_deref());

    // start active upstream health probing when enabled; skips unhealthy targets
    if config.health.enabled {
        tracing::info!(
            interval_secs = config.health.interval_secs,
            path = %config.health.path,
            "active upstream health probing enabled"
        );
        health::spawn_prober(&config, state.clone());
        upstream_metrics::spawn_scraper(&config, state.clone());
    }

    // the status-page poller is an independent secondary signal: it runs whenever
    // any provider sets status_page_url, regardless of active probing
    status_page::spawn_poller(&config, state.clone());

    // start the reload-free config watcher when a control plane is configured
    if let Some(snapshot_url) = args.snapshot_url {
        let period = std::time::Duration::from_secs(args.snapshot_poll_secs.max(1));
        tracing::info!(%snapshot_url, poll_secs = args.snapshot_poll_secs, pubsub = args.redis_url.is_some(), "config watcher enabled");
        watcher::spawn(state.clone(), snapshot_url, period, args.redis_url);
    } else {
        tracing::info!("no snapshot url configured; running with static bootstrap config");
    }

    let app = build_router(state, &config.server.metrics_path);

    tracing::info!(%addr, metrics_path = %config.server.metrics_path, "rolter-gateway listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // drain in-flight requests on SIGINT/SIGTERM instead of dropping them
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("rolter-gateway shut down cleanly");
    Ok(())
}

/// Assemble the gateway's axum router over a built [`AppState`], serving the
/// prometheus endpoint on `metrics_path`. Extracted so integration tests can
/// drive the full request pipeline in-process without binding a socket. A
/// `metrics_path` that is empty, unrooted, or collides with a built-in route is
/// rejected in favour of the default `/metrics` so router construction never
/// panics on a bad config.
pub fn build_router(state: AppState, metrics_path: &str) -> Router {
    let metrics_path =
        if metrics_path.starts_with('/') && !rolter_core::RESERVED_PATHS.contains(&metrics_path) {
            metrics_path
        } else {
            tracing::warn!(
                path = %metrics_path,
                "invalid or colliding metrics_path; falling back to /metrics"
            );
            "/metrics"
        };
    Router::new()
        .route("/healthz", get(handlers::healthz))
        .route(metrics_path, get(handlers::metrics))
        .route("/v1/models", get(handlers::list_models))
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/completions", post(handlers::completions))
        .route("/v1/messages", post(handlers::messages))
        .route("/v1/embeddings", post(handlers::embeddings))
        // ensure every request carries an x-request-id (generated when absent)
        // and echo it on the response, for end-to-end correlation
        .layer(axum::middleware::from_fn(trace::ensure_request_id))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Build the gateway router directly from a config, assembling a fresh
/// [`AppState`] (logging and redis disabled). Convenience for integration tests
/// and embedders that just want a ready-to-serve `Router`. Must be called from
/// within a Tokio runtime.
pub fn build_router_from_config(config: &GatewayConfig) -> Router {
    build_router(
        AppState::with_logging(config, None),
        &config.server.metrics_path,
    )
}

/// Resolve once the process receives a shutdown signal (Ctrl-C on all platforms,
/// or `SIGTERM` on Unix — the signal orchestrators send on rollout/scale-down).
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(err) => tracing::warn!(%err, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received ctrl-c, draining"),
        _ = terminate => tracing::info!("received SIGTERM, draining"),
    }
}
