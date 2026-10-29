//! rolter-gateway: the data-plane proxy.
//!
//! Loads a bootstrap config, builds an atomically-swappable routing snapshot and
//! serves OpenAI- and Anthropic-compatible endpoints, balancing across upstream
//! targets and streaming responses straight back to clients.
//!
//! The binary is a thin wrapper over [`run`]; the unified `rolter` launcher
//! reuses the same entrypoint as its `gateway` subcommand.
//!
//! **Internal crate.** It is published only so `cargo install rolter` can
//! resolve, and it offers no stable Rust API: any public item here may change
//! or disappear in any release, including a patch release. Build against
//! rolter's HTTP surfaces instead — see
//! [ADR-0032](https://github.com/rolter-ai/rolter/blob/master/docs/dev-docs/adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

mod adaptive_telemetry;
mod admin_proxy;
// admission registries are `pub` so the criterion benches under `benches/` can
// exercise them directly; they carry no stable API promise. a `///` here would
// make rustdoc resolve the module's own intra-doc links in this scope instead
// of the module's, so keep it a plain comment
#[doc(hidden)]
pub mod breaker;
mod breaker_report;
mod budgets;
mod cache;
#[cfg(test)]
mod cache_policy_tests;
mod cache_telemetry;
mod cancel;
mod clickhouse_client;
#[doc(hidden)]
pub mod cooldowns;
mod egress_client;
mod error;
mod example_key;
mod failover;
mod fake_llm;
mod genai;
mod guardrail_webhook;
mod guardrails;
mod handlers;
pub mod health;
mod health_events;
mod load;
mod local_classifier;
mod logging;
mod mcp_list_filter;
mod mcp_log;
mod mcp_policy;
mod mcp_proxy;
mod metrics;
pub mod model_catalog;
mod multipart;
mod openapi;
mod openapi_schemas;
#[cfg(test)]
mod org_namespace_tests;
mod pii_sanitizer;
mod pii_stream;
#[cfg(test)]
mod pii_stream_tests;
mod plugin_dispatch;
mod prompt_affinity;
mod prompt_templates;
mod queue;
mod rate_limits;
mod realtime;
mod realtime_guard;
mod realtime_metering;
mod redis_conn;
pub mod request_shape;
mod response_registry;
mod route_limits;
mod semantic;
mod semantic_guards;
mod sink_drain;
mod state;
mod status_page;
mod system_one;
pub mod target_mask;
mod trace;
mod unit_billing;
mod upstream_failure;
mod upstream_metrics;
mod usage_recording;
mod watcher;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::routing::{get, post};
use axum::Router;
use clap::Parser;
use tower_http::trace::TraceLayer;

use rolter_core::GatewayConfig;
pub use state::AppState;

#[derive(Parser, Debug)]
#[command(name = "rolter-gateway", version, about = "rolter data-plane gateway")]
pub struct Args {
    /// path to the bootstrap config file. Defaults to `rolter.toml`, which
    /// may be missing; a path given here or in `ROLTER_CONFIG` must exist
    /// (#3511)
    #[arg(short, long, env = "ROLTER_CONFIG")]
    pub config: Option<PathBuf>,
    /// override the bind host
    #[arg(long, env = "ROLTER_HOST")]
    pub host: Option<String>,
    /// override the bind port
    #[arg(long, env = "ROLTER_PORT")]
    pub port: Option<u16>,
    /// control-plane snapshot endpoint to poll for reload-free config
    /// updates, e.g. `http://control:4001/internal/snapshot`; polling is
    /// disabled when unset
    #[arg(long, env = "ROLTER_SNAPSHOT_URL", hide_env_values = true)]
    pub snapshot_url: Option<String>,
    /// how often to poll the snapshot endpoint, in seconds
    #[arg(long, env = "ROLTER_SNAPSHOT_POLL_SECS", default_value_t = 5)]
    pub snapshot_poll_secs: u64,
    /// redis connection url; when set (together with --snapshot-url), config
    /// bumps published by the control plane trigger an immediate refetch
    /// instead of waiting for the poll interval
    #[arg(long, env = "ROLTER_REDIS_URL", hide_env_values = true)]
    pub redis_url: Option<String>,
    /// control-plane base url (e.g. `http://control:4001`); when set, the
    /// gateway serves `/admin/*` by proxying to the control plane's
    /// `/api/v1/*` management API, so providers/models/keys can be managed
    /// through the gateway port
    #[arg(long, env = "ROLTER_ADMIN_URL", hide_env_values = true)]
    pub admin_url: Option<String>,
    /// bearer token sent on snapshot polls (and required by the control
    /// plane's management API when it sets the same `ROLTER_ADMIN_TOKEN`)
    #[arg(long, env = "ROLTER_ADMIN_TOKEN", hide_env_values = true)]
    pub admin_token: Option<String>,
    /// bearer token sent on snapshot polls when the control plane gates
    /// `/internal/*` on a distinct internal token (`ROLTER_INTERNAL_TOKEN`
    /// there). Falls back to `--admin-token` when unset
    #[arg(long, env = "ROLTER_INTERNAL_TOKEN", hide_env_values = true)]
    pub internal_token: Option<String>,
    /// directory every local guardrail model must resolve under; wins over
    /// `ROLTER_SEMANTIC_MODEL_ROOT` and `[semantic_guards] model_root`
    /// (#3511). Read once at startup, so a snapshot never widens it
    #[arg(long)]
    pub semantic_model_root: Option<PathBuf>,
}

/// The config file the gateway reads when no `--config` or `ROLTER_CONFIG`
/// names one.
pub const DEFAULT_CONFIG: &str = "rolter.toml";

/// The environment variable that sets `[semantic_guards] model_root` below
/// `--semantic-model-root` and above the file (#3511).
pub const MODEL_ROOT_ENV: &str = "ROLTER_SEMANTIC_MODEL_ROOT";

/// Read the bootstrap config. A path the operator named (`explicit`) must
/// exist, so a typo cannot silently drop every setting it holds, a
/// `model_root` among them (#3511); only the default path may be missing,
/// and then the gateway starts empty. A blank path counts as unset.
fn bootstrap_config(explicit: Option<&Path>, default: &Path) -> anyhow::Result<GatewayConfig> {
    let explicit = explicit.filter(|path| !path.to_string_lossy().trim().is_empty());
    let path = explicit.unwrap_or(default);
    if path.exists() {
        // `load` also warns about every key in the file rolter does not read
        // (#1434); unknown keys stay non-fatal, so this never blocks the boot
        return Ok(GatewayConfig::load(path)?);
    }
    if explicit.is_some() {
        anyhow::bail!(
            "config file {} not found; it was given with --config or ROLTER_CONFIG, so the \
             gateway does not start without it",
            path.display()
        );
    }
    tracing::warn!(path = %path.display(), "config file not found, starting with empty config");
    Ok(GatewayConfig::default())
}

/// Where the effective `model_root` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelRootSource {
    Flag,
    Env,
    File,
}

impl ModelRootSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "--semantic-model-root",
            Self::Env => MODEL_ROOT_ENV,
            Self::File => "[semantic_guards] model_root",
        }
    }
}

/// Fix the effective `model_root` in `config` from the flag, the
/// environment variable's value (`env`, as [`std::env::var`] read it) and the
/// file, and log it once (#3511). A value that is not UTF-8 is refused rather
/// than dropped, since dropping it would leave the models unconfined.
fn apply_model_root(
    config: &mut GatewayConfig,
    flag: Option<&Path>,
    env: Result<String, std::env::VarError>,
) -> anyhow::Result<()> {
    let flag = match flag {
        Some(flag) => Some(flag.to_str().ok_or_else(|| {
            anyhow::anyhow!("--semantic-model-root {} is not UTF-8", flag.display())
        })?),
        None => None,
    };
    let env = match env {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("{MODEL_ROOT_ENV} is not UTF-8")
        }
    };
    config.semantic_guards.model_root = match effective_model_root(
        flag,
        env.as_deref(),
        config.semantic_guards.model_root.as_deref(),
    ) {
        Some((root, source)) => {
            tracing::info!(model_root = %root, source = source.as_str(), "local guardrail model_root");
            Some(root)
        }
        None => {
            tracing::info!(
                "no local guardrail model_root: local classifier model files are read unconfined"
            );
            None
        }
    };
    Ok(())
}

/// The effective `model_root`: the flag, else the environment variable, else
/// the file's. An empty or blank value counts as unset.
fn effective_model_root(
    flag: Option<&str>,
    env: Option<&str>,
    file: Option<&str>,
) -> Option<(String, ModelRootSource)> {
    let set = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    flag.and_then(set)
        .map(|root| (root, ModelRootSource::Flag))
        .or_else(|| env.and_then(set).map(|root| (root, ModelRootSource::Env)))
        .or_else(|| file.and_then(set).map(|root| (root, ModelRootSource::File)))
}

/// Run the data-plane gateway to completion. The caller owns argument parsing
/// and telemetry initialization.
pub async fn run(args: Args) -> anyhow::Result<()> {
    let mut config = bootstrap_config(args.config.as_deref(), Path::new(DEFAULT_CONFIG))?;
    // fixed here, before the classifiers are configured: a snapshot never
    // carries a root, so nothing after startup can widen it
    apply_model_root(
        &mut config,
        args.semantic_model_root.as_deref(),
        std::env::var(MODEL_ROOT_ENV),
    )?;
    if let Some(host) = args.host {
        config.server.host = host;
    }
    if let Some(port) = args.port {
        config.server.port = port;
    }

    for warning in config
        .project_scoped_warnings()
        .into_iter()
        .chain(config.mixed_decision_route_warnings())
    {
        tracing::warn!("{warning}");
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

    // #929: a managed gateway with no `[logging].clickhouse_url` of its own
    // used to write nowhere while the dashboard queried the control plane's
    // ClickHouse — permanently empty analytics screens that looked exactly like
    // a quiet deployment. The control plane now publishes its own destination
    // in the snapshot, and this is the one read early enough to open a sink
    // with it: the writer is spawned once, here, and the hot-swappable snapshot
    // carries routing rather than background tasks.
    //
    // Best-effort: a control plane that is not up yet costs one short timeout
    // and the gateway starts exactly as it did before. A local
    // `clickhouse_url` short-circuits it entirely — an explicit decision in
    // this process's own config is never overridden by an inherited one.
    // `CLICKHOUSE_URL` is documented as overriding `[logging].clickhouse_url`
    // (docs/user-docs/configuration/environment-variables.mdx) and the control plane
    // has always read it, but the gateway never did — so the one variable a
    // compose file or a Helm chart sets on both processes reached only one of
    // them. It does now, below an explicit value in this gateway's own config.
    if config.logging.clickhouse_url.is_none() {
        match std::env::var("CLICKHOUSE_URL") {
            Ok(url) if !url.trim().is_empty() => {
                config.logging.clickhouse_url = Some(url.trim().to_string());
            }
            _ => {}
        }
    }

    if config.logging.clickhouse_url.is_none() {
        if let Some(snapshot_url) = args.snapshot_url.as_deref() {
            let token = args
                .internal_token
                .as_deref()
                .or(args.admin_token.as_deref());
            if let Some(url) = watcher::fetch_log_destination(
                snapshot_url,
                token,
                std::time::Duration::from_secs(5),
            )
            .await
            {
                log_endpoint("inheriting the control plane's log destination", &url);
                config.logging.clickhouse_url = Some(url);
            }
        }
    }

    if let Some(url) = &config.logging.clickhouse_url {
        log_endpoint("clickhouse request logging enabled", url);
    } else if args.snapshot_url.is_some() {
        // an empty analytics screen and a quiet deployment look identical, so
        // say which one this is while the operator is still reading the log
        tracing::warn!(
            "no clickhouse destination: request logs are not written, and the \
             dashboard's usage, cost and log screens will stay empty. Set \
             CLICKHOUSE_URL on the control plane (the fleet inherits it) or \
             [logging].clickhouse_url on this gateway"
        );
    }
    let mut state = AppState::with_logging(&config, args.redis_url.as_deref());
    // a control-plane-managed gateway fails closed when the key set is empty:
    // revoking the last virtual key must lock the data plane, not open it.
    // `server.require_auth` still overrides this in either direction
    state.managed_auth = args.snapshot_url.is_some();
    // reloads re-check the public example key against this (#1965)
    state.exposed_bind = example_key::bind_is_exposed(addr.ip());
    example_key::warn_if_exposed(&config, state.exposed_bind);
    let state = state;

    // export the counters the Prometheus endpoint already renders over OTLP too
    // (#805). observable instruments, so nothing is pushed on the request path:
    // the SDK reads the same atomics on its own interval. `None` — and no cost —
    // when no OTLP endpoint is configured. held for the lifetime of `run` so
    // metrics flush on shutdown
    let _metrics_export = {
        let scalars = state.metrics.clone();
        let labelled = state.metrics.clone();
        let guard =
            rolter_core::telemetry::install_metrics(rolter_core::telemetry::MetricsExport {
                scalars: move || scalars.scalars_for_export(),
                labelled_families: metrics::LABELLED_FAMILIES,
                labelled_gauge_families: metrics::LABELLED_GAUGE_FAMILIES,
                labelled: move || labelled.labelled_for_export(),
                latency_buckets_ms: metrics::Metrics::latency_buckets_ms(),
            });
        // the latency/TTFT histograms are the one instrument that cannot be
        // observable, so the request path needs a handle to record into. absent
        // this call — and on every deployment without OTLP — it records nothing
        if let Some(guard) = &guard {
            state.metrics.set_otlp_histograms(guard.histograms());
        }
        guard
    };

    // the health prober is always spawned; it self-gates on the live snapshot so a
    // hot-reload can enable/disable and re-tune probing without a restart (ROL-125)
    if config.health.enabled {
        tracing::info!(
            interval_secs = config.health.interval_secs,
            path = %config.health.path,
            "active upstream health probing enabled"
        );
    } else {
        tracing::info!("active upstream health probing disabled (enable via config hot-reload)");
    }
    health::spawn_prober(state.clone());
    // a local guardrail model that failed to load is read again with backoff
    // rather than only on the next config change (#3452)
    state.local_classifiers.spawn_retries();
    // the model catalogue behind /v1/models is refreshed on its own slow loop,
    // so it does not depend on active health checks being enabled (#1729)
    model_catalog::spawn_refresher(state.clone());
    // the metrics scraper self-gates on config.metrics_scrape.enabled (its
    // reload-awareness is tracked separately from the prober's)
    upstream_metrics::spawn_scraper(&config, state.clone());

    // the status-page poller is an independent secondary signal: it runs whenever
    // any provider sets status_page_url, regardless of active probing
    status_page::spawn_poller(&config, state.clone());

    // start the reload-free config watcher when a control plane is configured
    if let Some(snapshot_url) = args.snapshot_url {
        let telemetry_snapshot_url = snapshot_url.clone();
        let period = std::time::Duration::from_secs(args.snapshot_poll_secs.max(1));
        tracing::info!(snapshot_url = %rolter_core::redact::redact_url(&snapshot_url), poll_secs = args.snapshot_poll_secs, pubsub = args.redis_url.is_some(), "config watcher enabled");
        // both the cluster inventory and the adaptive-routing scoreboard are
        // keyed on this node's id, and a node that cannot name itself is
        // dropped by the control plane; say so here rather than leaving two
        // dashboard screens empty with nothing logged (#1644)
        watcher::log_node_identity();
        watcher::spawn(
            state.clone(),
            snapshot_url,
            period,
            args.redis_url,
            // the snapshot channel carries decrypted provider credentials, so
            // the control plane can gate it on a token separate from the
            // operator's; prefer that one and fall back to the admin token
            args.internal_token
                .clone()
                .or_else(|| args.admin_token.clone()),
        );
        // sample the adaptive balancers over the same channel, so the control
        // plane can serve the routing dashboard without reaching into the data
        // plane itself (#751)
        adaptive_telemetry::spawn(
            state.clone(),
            &telemetry_snapshot_url,
            args.internal_token
                .clone()
                .or_else(|| args.admin_token.clone()),
        );
        // and each target's circuit-breaker state, so the dashboard can show the
        // fleet's breakers (#2254)
        breaker_report::spawn(
            state.clone(),
            &telemetry_snapshot_url,
            args.internal_token
                .clone()
                .or_else(|| args.admin_token.clone()),
        );
    } else {
        tracing::info!("no snapshot url configured; running with static bootstrap config");
    }

    // kept past the router so shutdown can reach the realtime sessions, which
    // axum's drain does not wait for
    let realtime = state.clone();
    let mut app = build_router(
        state,
        &config.server.metrics_path,
        config.server.max_body_bytes,
    );

    // opt-in management surface on the gateway port, proxied to the control
    // plane (which owns persistence and authentication)
    if let Some(admin_url) = &args.admin_url {
        tracing::info!(%admin_url, "admin api enabled: /admin/* proxies to the control plane");
        app = app.merge(admin_proxy::router(admin_url));
    }

    tracing::info!(%addr, metrics_path = %config.server.metrics_path, "rolter-gateway listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // drain in-flight requests on SIGINT/SIGTERM instead of dropping them
    let closing = realtime.realtime_sessions.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            rolter_core::shutdown::shutdown_signal().await;
            // an upgraded socket is outside axum's drain, so realtime sessions
            // start closing now, alongside the http requests it does wait for
            closing.close();
        })
        .await?;
    if !realtime.drain_realtime_sessions(REALTIME_DRAIN_GRACE).await {
        tracing::warn!(
            grace_secs = REALTIME_DRAIN_GRACE.as_secs(),
            "realtime sessions still open after the drain grace; their unflushed usage is lost"
        );
    }
    // last, so the rows the requests and realtime meters above produced are
    // already queued when the sinks are told to stop
    if !realtime.drain_sinks(SINK_DRAIN_GRACE).await {
        tracing::warn!(
            grace_secs = SINK_DRAIN_GRACE.as_secs(),
            "request-log, health-event or usage sinks did not finish flushing; what they still held is lost"
        );
    }
    tracing::info!("rolter-gateway shut down cleanly");
    Ok(())
}

/// Assemble the gateway's axum router over a built [`AppState`], serving the
/// prometheus endpoint on `metrics_path`. Extracted so integration tests can
/// drive the full request pipeline in-process without binding a socket. A
/// `metrics_path` that is empty, unrooted, or collides with a built-in route is
/// rejected in favour of the default `/metrics` so router construction never
/// panics on a bad config.
pub fn build_router(state: AppState, metrics_path: &str, max_body_bytes: usize) -> Router {
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
        .route("/", get(openapi::root))
        .route("/healthz", get(handlers::healthz))
        .route("/readyz", get(handlers::readyz))
        .route(metrics_path, get(handlers::metrics))
        .route("/v1/models", get(handlers::list_models))
        .route("/openapi.json", get(openapi::openapi_json))
        .route("/docs", get(openapi::docs))
        .route(openapi::DOCS_BUNDLE_PATH, get(openapi::docs_bundle))
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/completions", post(handlers::completions))
        .route("/v1/responses", post(handlers::responses))
        .route(
            "/v1/responses/{response_id}",
            get(handlers::retrieve_response).delete(handlers::delete_response),
        )
        .route(
            "/v1/responses/{response_id}/cancel",
            post(handlers::cancel_response),
        )
        .route(
            "/v1/responses/{response_id}/input_items",
            get(handlers::response_input_items),
        )
        .route(
            "/v1/responses/{response_id}/compact",
            post(handlers::unsupported_response_lifecycle),
        )
        .route(
            "/v1/responses/{response_id}/input_tokens",
            get(handlers::unsupported_response_lifecycle),
        )
        .route("/v1/messages", post(handlers::messages))
        .route("/v1/embeddings", post(handlers::embeddings))
        .route("/v1/rerank", post(handlers::rerank))
        .route(
            rolter_core::upstream::SYSTEM_ONE_PATH,
            post(handlers::system_one),
        )
        .route("/v1/images/generations", post(handlers::images_generations))
        .route("/v1/audio/speech", post(handlers::audio_speech))
        .route(
            "/v1/audio/transcriptions",
            post(handlers::audio_transcriptions),
        )
        .route("/v1/audio/translations", post(handlers::audio_translations))
        .route("/v1/realtime", get(realtime::realtime))
        .route(
            "/mcp/{server}",
            get(mcp_proxy::proxy_root)
                .post(mcp_proxy::proxy_root)
                .delete(mcp_proxy::proxy_root),
        )
        .route(
            "/mcp/{server}/{*path}",
            get(mcp_proxy::proxy_tail)
                .post(mcp_proxy::proxy_tail)
                .delete(mcp_proxy::proxy_tail),
        )
        // cap request bodies (raising axum's 2 MiB default) so large LLM
        // payloads pass but a pathological body is rejected. the mapper is added
        // next, so it wraps the limit layer and rewrites its 413 into json
        .layer(axum::extract::DefaultBodyLimit::max(max_body_bytes))
        .layer(axum::middleware::from_fn(handlers::map_payload_too_large))
        // the operator's ingress filter (#1162). added after the body limit so
        // it wraps it: a request that fails the filter is refused without the
        // gateway reading its body at all
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            handlers::enforce_required_headers,
        ))
        // ensure every request carries a request id in the configured header
        // (generated when absent) and echo it on the response, for end-to-end
        // correlation. outside the ingress filter, so a refusal carries one too
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            trace::ensure_request_id,
        ))
        // surface 4xx/5xx on the terminal at the default `info` filter; the stock
        // hook logs every response at `debug`, hiding errors in `uvx rolter`
        // (ROL-230). on_failure is disabled so classified 5xx are not double-logged
        .layer(
            TraceLayer::new_for_http()
                // builds the request span at INFO and adopts the caller's trace
                // context as its parent when a pipeline is installed (#805); the
                // stock DEBUG span is disabled at the default filter, and a
                // disabled span cannot be parented or exported
                .make_span_with(trace::GatewayMakeSpan)
                .on_response(trace::StatusAwareOnResponse)
                .on_failure(()),
        )
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
        config.server.max_body_bytes,
    )
}

/// How long shutdown waits for realtime sessions to close and their meters to
/// flush once the HTTP drain is done. Well inside the 30 seconds an
/// orchestrator usually allows between `SIGTERM` and `SIGKILL`: the sessions
/// were told to close when the signal arrived, so this only covers a meter
/// still waiting on Redis.
const REALTIME_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long shutdown waits for the request-log writer, the health-event writer
/// and the usage-recording workers to flush and stop, after the realtime drain.
/// A healthy ClickHouse or Redis takes milliseconds; the bound only matters when
/// one is unreachable, and it is kept short so the HTTP drain, the realtime
/// grace and this still fit inside the 30 seconds an orchestrator usually allows
/// before `SIGKILL`.
const SINK_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Log that a datastore endpoint is in use, with the credentials in its url
/// masked (#2406). Every startup line that names one goes through here.
fn log_endpoint(what: &str, url: &str) {
    tracing::info!(url = %rolter_core::redact::redact_url(url), "{what}");
}

#[cfg(test)]
mod startup_log_tests {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct Fields(String);
            impl tracing::field::Visit for Fields {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.push_str(&format!("{}={value:?} ", field.name()));
                }
            }
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0.lock().push(fields.0);
        }
    }

    /// A throwaway secret built at run time, so no credential-shaped literal
    /// sits in the source for secret scanners to flag
    fn throwaway_secret(tag: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        format!("{tag}{}x{nanos}", std::process::id())
    }

    #[test]
    fn startup_lines_keep_the_clickhouse_password_out() {
        let user_secret = throwaway_secret("u");
        let query_secret = throwaway_secret("q");
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());
        tracing::subscriber::with_default(subscriber, || {
            super::log_endpoint(
                "clickhouse request logging enabled",
                &format!("http://default:{user_secret}@ch:8123/?password={query_secret}"),
            );
        });
        let seen = captured.0.lock().join("\n");
        for secret in [&user_secret, &query_secret] {
            // the message names neither the secret nor the captured line, so a
            // failure cannot itself print the credential
            assert!(
                !seen.contains(secret.as_str()),
                "a test credential reached the startup log"
            );
        }
        assert!(seen.contains("ch:8123"), "{seen}");
    }
}

#[cfg(test)]
mod help_tests {
    use clap::CommandFactory;

    /// `--help` prints `[env: NAME=value]` for every flag backed by a variable,
    /// so a token or a url with a password in it must opt out (#2793). Walks the
    /// whole command, so a flag added later is caught without anyone extending a
    /// list
    #[test]
    fn secret_env_values_stay_out_of_help() {
        let checked = rolter_core::cli_guard::assert_secret_envs_hidden(&super::Args::command());
        assert!(checked > 0, "the walk found no secret-looking env flags");
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use std::path::{Path, PathBuf};

    use clap::Parser;

    use super::{apply_model_root, bootstrap_config, effective_model_root, ModelRootSource};
    use rolter_core::GatewayConfig;

    fn missing() -> PathBuf {
        std::env::temp_dir().join(format!("rolter-missing-{}.toml", std::process::id()))
    }

    #[tokio::test]
    async fn an_explicit_config_path_that_does_not_exist_refuses_startup() {
        let path = missing();
        let error = bootstrap_config(Some(&path), Path::new("unused.toml"))
            .expect_err("a named file that is missing is an error");
        assert!(
            error.to_string().contains(&*path.to_string_lossy()),
            "{error}"
        );
        // the whole startup stops there, before it binds anything
        let args = super::Args::parse_from(["rolter-gateway", "--config", path.to_str().unwrap()]);
        let error = super::run(args)
            .await
            .expect_err("the gateway does not start");
        assert!(error.to_string().contains("not found"), "{error}");
    }

    #[test]
    fn the_default_config_path_may_be_missing_and_starts_empty() {
        let config = bootstrap_config(None, &missing()).expect("the default path may be missing");
        assert!(config.semantic_guards.model_root.is_none());
    }

    #[test]
    fn an_explicit_config_path_that_exists_is_read() {
        let path = std::env::temp_dir().join(format!("rolter-root-{}.toml", std::process::id()));
        std::fs::write(&path, "[semantic_guards]\nmodel_root = \"/srv/models\"\n").unwrap();
        let config = bootstrap_config(Some(&path), Path::new("unused.toml"));
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            config.unwrap().semantic_guards.model_root.as_deref(),
            Some("/srv/models")
        );
    }

    #[test]
    fn a_blank_explicit_config_path_counts_as_unset() {
        for blank in ["", "  "] {
            let config = bootstrap_config(Some(Path::new(blank)), &missing())
                .expect("a blank path falls back to the default");
            assert!(config.semantic_guards.model_root.is_none());
        }
    }

    #[test]
    fn the_model_root_is_applied_to_the_config_from_each_source() {
        use std::env::VarError;
        let file = || {
            let mut config = GatewayConfig::default();
            config.semantic_guards.model_root = Some("/file".to_string());
            config
        };
        let applied =
            |mut config: GatewayConfig, flag: Option<&Path>, env: Result<String, VarError>| {
                apply_model_root(&mut config, flag, env).map(|()| config.semantic_guards.model_root)
            };
        assert_eq!(
            applied(file(), Some(Path::new("/flag")), Ok("/env".into())).unwrap(),
            Some("/flag".to_string())
        );
        assert_eq!(
            applied(file(), None, Ok("/env".into())).unwrap(),
            Some("/env".to_string())
        );
        assert_eq!(
            applied(file(), None, Err(VarError::NotPresent)).unwrap(),
            Some("/file".to_string())
        );
        assert_eq!(
            applied(GatewayConfig::default(), None, Err(VarError::NotPresent)).unwrap(),
            None
        );
        // a value that is not UTF-8 is refused, never dropped
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let bad = std::ffi::OsString::from_vec(vec![b'/', 0xff]);
            let error = applied(file(), None, Err(VarError::NotUnicode(bad.clone()))).unwrap_err();
            assert!(error.to_string().contains("not UTF-8"), "{error}");
            let flag = std::path::PathBuf::from(bad);
            assert!(applied(file(), Some(&flag), Ok("/env".into())).is_err());
        }
    }

    #[test]
    fn the_model_root_flag_wins_over_the_env_which_wins_over_the_file() {
        let flag = "/flag";
        let pick = |flag: Option<&str>, env: Option<&str>, file: Option<&str>| {
            effective_model_root(flag, env, file)
        };
        assert_eq!(
            pick(Some(flag), Some("/env"), Some("/file")),
            Some(("/flag".to_string(), ModelRootSource::Flag))
        );
        assert_eq!(
            pick(None, Some("/env"), Some("/file")),
            Some(("/env".to_string(), ModelRootSource::Env))
        );
        assert_eq!(
            pick(None, None, Some("/file")),
            Some(("/file".to_string(), ModelRootSource::File))
        );
        // a blank source is unset and falls through to the next
        assert_eq!(
            pick(Some(" "), Some(""), Some(" /file ")),
            Some(("/file".to_string(), ModelRootSource::File))
        );
        assert_eq!(pick(None, None, None), None);
    }

    #[test]
    fn the_model_root_flag_parses() {
        let args = super::Args::parse_from(["rolter-gateway", "--semantic-model-root", "/srv/m"]);
        assert_eq!(
            args.semantic_model_root.as_deref(),
            Some(Path::new("/srv/m"))
        );
    }
}
