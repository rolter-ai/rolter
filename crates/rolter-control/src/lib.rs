//! rolter-control: the control plane.
//!
//! Hosts the management API used by the dashboard and serves the built SPA as
//! static assets. The MVP exposes health, a config read endpoint backed by an
//! in-memory store, and the role catalog; CRUD, RBAC enforcement, Postgres
//! persistence and Redis change publication are added in later phases.
//!
//! The binary is a thin wrapper over [`run`]; the unified `rolter` launcher
//! reuses the same entrypoint as its `control` subcommand.
//!
//! **Internal crate.** It is published only so `cargo install rolter` can
//! resolve, and it offers no stable Rust API: any public item here may change
//! or disappear in any release, including a patch release. Build against
//! rolter's HTTP surfaces instead — see
//! [ADR-0032](https://github.com/rolter-ai/rolter/blob/master/docs/adr/2026-09-09-one-point-oh-compatibility-guarantees.md).

#[cfg(feature = "postgres")]
mod access_control;
#[cfg(feature = "postgres")]
mod adaptive_policy;
#[cfg(feature = "postgres")]
mod adaptive_telemetry;
#[cfg(feature = "postgres")]
mod alerting;
mod analytics;
#[cfg(feature = "postgres")]
mod auth;
#[cfg(feature = "postgres")]
mod auth_policy;
#[cfg(feature = "postgres")]
mod client_settings;
#[cfg(feature = "postgres")]
mod cluster;
#[cfg(feature = "postgres")]
mod collector_config;
#[cfg(feature = "postgres")]
mod compatibility_policy;
// the renderer is pure and compiles without a store so its determinism and
// secret-stripping are covered by the default-feature test run too; only the
// route it backs needs postgres
pub mod config_export;
#[cfg(feature = "postgres")]
mod connectors;
mod cors;
#[cfg(feature = "postgres")]
mod crud;
#[cfg(feature = "postgres")]
mod feature_flags;
#[cfg(feature = "postgres")]
mod guardrails;
mod health;
#[cfg(feature = "postgres")]
mod invitations;
#[cfg(feature = "postgres")]
mod labels;
pub mod ldap;
#[cfg(feature = "postgres")]
mod logging_settings;
mod login_throttle;
#[cfg(feature = "postgres")]
mod mcp_logs;
#[cfg(feature = "postgres")]
mod mcp_oauth;
#[cfg(feature = "postgres")]
mod mcp_oauth_flow;
#[cfg(feature = "postgres")]
mod me;
/// TOTP second factor for local accounts (#1078). `pub` for the break-glass
/// reset the `rolter` launcher's `mfa reset` subcommand runs.
#[cfg(feature = "postgres")]
pub mod mfa;
#[cfg(feature = "postgres")]
mod model_defaults;
mod open_mode;
mod openapi;
#[cfg(feature = "postgres")]
mod plugins;
mod proxy;
#[cfg(feature = "postgres")]
mod rbac;
#[cfg(feature = "postgres")]
mod rbac_matrix;
#[cfg(feature = "postgres")]
mod runtime_policy;
#[cfg(feature = "postgres")]
mod scim;
#[cfg(feature = "postgres")]
mod scim_groups;
#[cfg(feature = "postgres")]
mod security;
#[cfg(feature = "postgres")]
pub mod seed;
#[cfg(feature = "postgres")]
mod sso;
#[cfg(feature = "postgres")]
mod stability;
mod telemetry;
mod ui_config;
#[cfg(feature = "postgres")]
mod ui_events;
pub mod update_check;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use clap::Parser;
use serde::Deserialize;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

use rolter_auth::Role;
use rolter_core::GatewayConfig;
#[cfg(feature = "postgres")]
use rolter_store::MergedConfigStore;
use rolter_store::{ConfigStore, InMemoryConfigStore};

#[derive(Parser, Debug)]
#[command(name = "rolter-control", version, about = "rolter control plane")]
pub struct Args {
    /// address the management API and dashboard bind to. Loopback by default:
    /// the control plane runs unauthenticated when no `--admin-token` is set,
    /// and that state must not reach a public interface by omission (#970).
    /// Containers and clusters set this explicitly — see `charts/` and
    /// `docker/docker-compose.yml`
    #[arg(long, env = "ROLTER_CONTROL_HOST", default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, env = "ROLTER_CONTROL_PORT", default_value_t = 4001)]
    pub port: u16,
    /// directory holding the built UI (index.html + assets)
    #[arg(long, env = "ROLTER_UI_DIR", default_value = "ui/dist")]
    pub ui_dir: PathBuf,
    /// OTLP/HTTP traces endpoint for the *dashboard's* browser tracing, e.g.
    /// `http://localhost:4318/v1/traces`. Injected into the served HTML as
    /// `window.__ROLTER_CONFIG__`; unset (the default) leaves browser tracing
    /// off, loading no SDK and changing no headers. This is deliberately not
    /// `OTEL_EXPORTER_OTLP_ENDPOINT`: that one is the backend's own gRPC
    /// exporter, while a browser needs an HTTP endpoint reachable from the
    /// user's machine rather than from inside the cluster
    #[arg(long, env = "ROLTER_UI_OTEL_ENDPOINT")]
    pub ui_otel_endpoint: Option<String>,
    /// `service.name` reported by the dashboard's spans; the dashboard falls
    /// back to `rolter-ui` when this is unset
    #[arg(long, env = "ROLTER_UI_OTEL_SERVICE_NAME")]
    pub ui_otel_service_name: Option<String>,
    /// base URL of the rolter-gateway data plane; the dashboard Playground's
    /// `/gw/*` calls are reverse-proxied here (see `crate::proxy`)
    #[arg(
        long,
        env = "ROLTER_GATEWAY_URL",
        default_value = "http://localhost:4000"
    )]
    pub gateway_url: String,
    /// optional bootstrap config. Without a database it seeds the in-memory
    /// store; with `--database-url` its providers/routes become read-only
    /// "config models" merged over the DB-defined ones (config wins on
    /// name conflicts)
    #[arg(short, long, env = "ROLTER_CONFIG")]
    pub config: Option<PathBuf>,
    /// postgres connection string; when set, the control plane reads/serves
    /// its config from the database instead of the bootstrap toml
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DATABASE_URL")]
    pub database_url: Option<String>,
    /// upper bound on open postgres connections. One pool serves everything:
    /// `/internal/snapshot` (polled by every gateway in the fleet), the whole
    /// dashboard CRUD surface, SCIM, SSO, MCP and the RBAC guard's per-request
    /// membership lookups. Size it against the database's own
    /// `max_connections` divided across control replicas — not against gateway
    /// count, since snapshot polls are short (#1052)
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DB_MAX_CONNECTIONS", default_value_t = 10)]
    pub db_max_connections: u32,
    /// connections kept open while idle. Above zero, the first request after a
    /// quiet period skips the connect handshake
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DB_MIN_CONNECTIONS", default_value_t = 0)]
    pub db_min_connections: u32,
    /// how long a request waits for a free connection before failing. Bounded
    /// on purpose: pool exhaustion should surface as a fast, attributable
    /// error rather than an opaque stall that looks like a slow database
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DB_ACQUIRE_TIMEOUT_SECS", default_value_t = 30)]
    pub db_acquire_timeout_secs: u64,
    /// close a connection idle for longer than this; `0` keeps idle
    /// connections forever
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DB_IDLE_TIMEOUT_SECS", default_value_t = 600)]
    pub db_idle_timeout_secs: u64,
    /// retire a connection older than this regardless of use, so a pool cannot
    /// pin itself to one database instance across a failover; `0` disables
    #[cfg(feature = "postgres")]
    #[arg(long, env = "ROLTER_DB_MAX_LIFETIME_SECS", default_value_t = 1800)]
    pub db_max_lifetime_secs: u64,
    /// redis connection url; when set, config-version bumps are published on
    /// the `rolter.config` channel so gateways refetch immediately instead of
    /// waiting for their poll interval
    #[arg(long, env = "ROLTER_REDIS_URL")]
    pub redis_url: Option<String>,
    /// turn failed-login throttling off. Only for a deployment that fronts the
    /// control plane with its own rate limiter; without it
    /// `POST /api/v1/auth/login` answers unlimited password guesses and each
    /// one buys a full argon2 verification (#1079)
    #[arg(long, env = "ROLTER_LOGIN_THROTTLE_DISABLED", default_value_t = false)]
    pub login_throttle_disabled: bool,
    /// failed logins one account tolerates inside the window before it is
    /// locked for a while
    #[arg(long, env = "ROLTER_LOGIN_MAX_FAILURES", default_value_t = 5)]
    pub login_max_failures: u32,
    /// failed logins one client address tolerates inside the window, across
    /// every account it tries. Higher than the per-account budget on purpose:
    /// one office egress address is a whole floor of legitimate typos
    #[arg(long, env = "ROLTER_LOGIN_IP_MAX_FAILURES", default_value_t = 50)]
    pub login_ip_max_failures: u32,
    /// how long a failure counter survives without a new failure
    #[arg(long, env = "ROLTER_LOGIN_FAILURE_WINDOW_SECS", default_value_t = 900)]
    pub login_failure_window_secs: u64,
    /// the first lock once the budget is spent; each further failure doubles it
    #[arg(long, env = "ROLTER_LOGIN_LOCK_SECS", default_value_t = 60)]
    pub login_lock_secs: u64,
    /// ceiling on that doubling, so a lock is always a bounded outage and never
    /// a state an attacker can park an account in
    #[arg(long, env = "ROLTER_LOGIN_MAX_LOCK_SECS", default_value_t = 900)]
    pub login_max_lock_secs: u64,
    /// ceiling on the deliberate delay added to a rejected login, in
    /// milliseconds. Bounded so a guessing run cannot pin connections open
    #[arg(long, env = "ROLTER_LOGIN_MAX_DELAY_MS", default_value_t = 2000)]
    pub login_max_delay_ms: u64,
    /// count failed logins against the `X-Forwarded-For` client rather than the
    /// socket peer. Off by default: an unverified forwarded header hands an
    /// attacker unlimited budgets *and* the ability to spend someone else's, so
    /// turn this on only when every request reaches rolter through a proxy that
    /// rewrites the header
    #[arg(
        long,
        env = "ROLTER_LOGIN_TRUST_FORWARDED_FOR",
        default_value_t = false
    )]
    pub login_trust_forwarded_for: bool,
    /// clickhouse http url; when set, the dashboard usage/cost analytics
    /// endpoints (`/api/v1/analytics/*`) query the `request_logs` table
    #[arg(long, env = "CLICKHOUSE_URL")]
    pub clickhouse_url: Option<String>,
    /// bearer token required on the CRUD API and `/internal/snapshot`; when
    /// unset those endpoints are open (a warning is logged at startup)
    #[arg(long, env = "ROLTER_ADMIN_TOKEN")]
    pub admin_token: Option<String>,
    /// bearer token required on `/internal/*` — the control↔data-plane channel,
    /// which carries decrypted provider credentials. When set, the operator
    /// admin token no longer opens that channel; when unset it falls back to
    /// `--admin-token` (the historical behavior)
    #[arg(long, env = "ROLTER_INTERNAL_TOKEN")]
    pub internal_token: Option<String>,
    /// when set (e.g. `127.0.0.1:4002`), `/internal/*` moves off the public API
    /// listener onto its own socket, so the plaintext-credential channel is not
    /// reachable on the port the dashboard and management API are served from
    #[arg(long, env = "ROLTER_INTERNAL_ADDR")]
    pub internal_addr: Option<SocketAddr>,
    /// acknowledge running with no `--admin-token` on a non-loopback bind.
    /// Without it the control plane refuses to start in that combination,
    /// because every request would be served as superadmin with no credential
    /// (see `crate::open_mode`).
    ///
    /// `FalseyValueParser` rather than the derived bool parser: from the
    /// environment this is set as `=1` at least as often as `=true`, and a
    /// container that fails to start over which spelling it used is a worse
    /// outcome than either
    #[arg(
        long,
        env = "ROLTER_ALLOW_OPEN_MODE",
        action = clap::ArgAction::SetTrue,
        value_parser = clap::builder::FalseyValueParser::new(),
    )]
    pub allow_open_mode: bool,
}

/// Every value here mirrors the `default_value` on the field above it, so an
/// embedder that hand-builds `Args` — `rolter easy-up` does — lands on exactly
/// the configuration `rolter-control` would have parsed for itself.
///
/// It also lets that embedder write `..Default::default()` instead of an
/// exhaustive literal, which is what keeps it compiling when `postgres` is
/// enabled on this crate alone and the field set grows underneath it (#1295).
impl Default for Args {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 4001,
            ui_dir: PathBuf::from("ui/dist"),
            ui_otel_endpoint: None,
            ui_otel_service_name: None,
            gateway_url: "http://localhost:4000".to_string(),
            config: None,
            #[cfg(feature = "postgres")]
            database_url: None,
            #[cfg(feature = "postgres")]
            db_max_connections: 10,
            #[cfg(feature = "postgres")]
            db_min_connections: 0,
            #[cfg(feature = "postgres")]
            db_acquire_timeout_secs: 30,
            #[cfg(feature = "postgres")]
            db_idle_timeout_secs: 600,
            #[cfg(feature = "postgres")]
            db_max_lifetime_secs: 1800,
            redis_url: None,
            login_throttle_disabled: false,
            login_max_failures: 5,
            login_ip_max_failures: 50,
            login_failure_window_secs: 900,
            login_lock_secs: 60,
            login_max_lock_secs: 900,
            login_max_delay_ms: 2000,
            login_trust_forwarded_for: false,
            clickhouse_url: None,
            admin_token: None,
            internal_token: None,
            internal_addr: None,
            allow_open_mode: false,
        }
    }
}

/// Names owned by the bootstrap config file: immutable at runtime,
/// LiteLLM-style. The CRUD API rejects mutations that collide with them.
// only read by the postgres-gated CRUD module
#[cfg_attr(not(feature = "postgres"), allow(dead_code))]
#[derive(Default)]
struct ConfigOwned {
    providers: std::collections::HashSet<String>,
    models: std::collections::HashSet<String>,
    /// readonly provider-group slugs; the CRUD API rejects mutations against
    /// these (ADR-0022). default-tier groups are DB-owned and absent here
    groups: std::collections::HashSet<String>,
}

impl ConfigOwned {
    fn from_config(config: &GatewayConfig) -> Self {
        // config.providers / config.routes / config.provider_groups hold only the
        // readonly (effective) tier; the default tiers are seeded to the DB and
        // are deliberately editable, so they must not be tracked as config-owned
        Self {
            providers: config.providers.iter().map(|p| p.name.clone()).collect(),
            models: config.routes.iter().map(|r| r.model.clone()).collect(),
            groups: config
                .provider_groups
                .iter()
                .map(|g| {
                    g.slug
                        .clone()
                        .unwrap_or_else(|| rolter_core::slug::slugify(&g.name))
                })
                .collect(),
        }
    }
}

/// Default externally reachable base URL when `ROLTER_PUBLIC_URL` is unset.
const DEFAULT_PUBLIC_URL: &str = "http://localhost:4001";

/// Normalize a configured public base URL: blank counts as unset, and the
/// trailing slash is dropped so callers can always append an absolute path.
fn normalize_public_url(configured: Option<String>) -> String {
    configured
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PUBLIC_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Read `ROLTER_PUBLIC_URL` once, at startup, for [`ControlState::public_url`].
fn public_url_from_env() -> Arc<String> {
    Arc::new(normalize_public_url(
        std::env::var("ROLTER_PUBLIC_URL").ok(),
    ))
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
    /// set when `--clickhouse-url` is configured; backs the usage/cost
    /// analytics endpoints
    clickhouse: Option<analytics::ClickHouseClient>,
    /// egress policy from the bootstrap config (defaults when there is none);
    /// the CRUD API rejects a provider whose `api_base` it denies, so an SSRF
    /// target never reaches the database in the first place
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    egress: Arc<rolter_core::EgressPolicy>,
    /// settlement currency + rate table from the bootstrap config (defaults
    /// when there is none); the CRUD API rejects a model price in a currency
    /// this cannot convert, so an unconvertible price never reaches the
    /// database and never silently charges the wrong amount (#650)
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    currency: Arc<rolter_core::CurrencyConfig>,
    /// when set, the CRUD API and `/internal/snapshot` require
    /// `Authorization: Bearer <token>`
    admin_token: Option<Arc<String>>,
    /// when set, `/internal/*` requires this token *instead of* the admin one,
    /// so an operator credential no longer unlocks decrypted provider keys.
    /// Falls back to `admin_token` when unset
    internal_token: Option<Arc<String>>,
    /// shared client for the `/gw/*` reverse proxy to the gateway data plane
    http: reqwest::Client,
    /// base URL of the rolter-gateway the `/gw/*` proxy forwards to
    gateway_url: Arc<String>,
    /// the control plane's own externally reachable base URL, from
    /// `ROLTER_PUBLIC_URL`. Resolved once here rather than read from the
    /// environment inside each handler: the SSO redirect URI, the MCP OAuth
    /// callback and an invitation's accept link must all agree for the whole
    /// life of a flow, and a value that can change between two reads of the
    /// same request is one more thing that can disagree (#1418)
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    public_url: Arc<String>,
    /// set when `--database-url` is configured; backs the CRUD API, which
    /// needs direct repository access beyond what `ConfigStore` exposes
    #[cfg(feature = "postgres")]
    pool: Option<sqlx::PgPool>,
    /// live cross-origin policy from `security_settings` (#813), swapped on
    /// every settings write so an edit applies without a restart. Empty by
    /// default, which is the same-origin deployment and adds no headers
    cors: Arc<arc_swap::ArcSwap<cors::CorsPolicy>>,
    /// OTLP histograms for snapshot generation and API latency (#845). Inert,
    /// and free, on every deployment without an OTLP endpoint
    metrics: rolter_core::telemetry::ControlHistograms,
    /// failed-login counters guarding the password endpoints (#1079). Shared
    /// across replicas through redis when one is configured, process-local
    /// otherwise; a default value counts nothing
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    login_throttle: login_throttle::LoginThrottle,
    /// whether `X-Forwarded-For` may name the client the throttle counts
    /// against; see [`login_throttle::ClientAddr`]
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    trust_forwarded_for: bool,
    /// the last successful check for a newer release (#902), served from
    /// `GET /api/v1/version`. Disabled, and never on the network, when
    /// `ROLTER_UPDATE_CHECK` is falsy
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    update_check: update_check::UpdateChecker,
}

/// Run the control plane to completion. The caller owns argument parsing and
/// telemetry initialization.
pub async fn run(args: Args) -> anyhow::Result<()> {
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

    let clickhouse = args.clickhouse_url.as_deref().map(|url| {
        tracing::info!(%url, "usage/cost analytics enabled");
        analytics::ClickHouseClient::new(url)
    });

    let admin_token = args
        .admin_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| Arc::new(t.to_string()));
    // decided before anything binds: an unauthenticated control plane reachable
    // from off the machine is refused outright rather than served and warned
    // about (#970)
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    let listeners: Vec<SocketAddr> = std::iter::once(addr)
        .chain(args.internal_addr)
        .collect::<Vec<_>>();
    let open_mode = open_mode::evaluate(admin_token.is_some(), &listeners, args.allow_open_mode)?;
    open_mode::warn(open_mode, &listeners);

    let internal_token = args
        .internal_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| Arc::new(t.to_string()));
    if internal_token.is_none() && admin_token.is_some() {
        tracing::warn!(
            "ROLTER_INTERNAL_TOKEN is unset: /internal/snapshot, which carries decrypted \
             provider credentials, accepts the operator admin token; set a distinct internal \
             token (and ROLTER_INTERNAL_ADDR) to separate the two trust boundaries"
        );
    }

    let egress = Arc::new(
        bootstrap
            .as_ref()
            .map(|c| c.egress.clone())
            .unwrap_or_default(),
    );
    let currency = Arc::new(
        bootstrap
            .as_ref()
            .map(|c| c.currency.clone())
            .unwrap_or_default(),
    );
    // snapshot generation is fleet-wide config-propagation delay and CRUD
    // latency is what makes a slow dashboard write attributable, and neither
    // was measured (#845). `None` — and no cost — without an OTLP endpoint;
    // held for the lifetime of `run` so metrics flush on shutdown
    //
    // the store is built first so the pool gauges can be registered against a
    // pool that already exists; an observable instrument installed later would
    // miss the interval it was installed in (#1052)
    #[allow(unused_variables)]
    let (store, pool) = build_store(&args, bootstrap).await?;
    #[cfg(feature = "postgres")]
    let pool_sampler: Option<rolter_core::telemetry::PoolSampler> = pool.clone().map(|pool| {
        Arc::new(move || {
            let stats = rolter_store::postgres::pool_stats(&pool);
            rolter_core::telemetry::PoolGauges {
                connections: stats.connections,
                idle: stats.idle,
                max: stats.max,
            }
        }) as rolter_core::telemetry::PoolSampler
    });
    #[cfg(not(feature = "postgres"))]
    let pool_sampler: Option<rolter_core::telemetry::PoolSampler> = None;
    let metrics_guard = rolter_core::telemetry::install_control_metrics_with_pool(pool_sampler);
    let metrics = metrics_guard
        .as_ref()
        .map(rolter_core::telemetry::MetricsGuard::control_histograms)
        .unwrap_or_default();
    let _metrics_guard = metrics_guard;

    // acquire wait is what an operator actually feels when the ceiling is too
    // low, and the gauges alone cannot show it. Sampled on a timer rather than
    // per call so nothing is added to the request path
    #[cfg(feature = "postgres")]
    if let Some(pool) = pool.clone() {
        if metrics.is_active() {
            let metrics = metrics.clone();
            tokio::spawn(async move { sample_pool_acquire(pool, metrics).await });
        }
    }
    let http = reqwest::Client::new();

    // the throttle shares redis with config pub/sub when there is one, so every
    // replica counts against the same budget. without redis it is process-local
    // — correct for the single-node deployment rolter is usually run as, and a
    // multiplied budget on a fleet, which is why that case says so out loud
    let login_throttle = if args.login_throttle_disabled {
        tracing::warn!(
            "failed-login throttling is disabled; /api/v1/auth/login will answer \
             unlimited password guesses"
        );
        login_throttle::LoginThrottle::disabled()
    } else {
        if redis.is_none() {
            tracing::info!(
                "failed-login counters are process-local (no --redis-url); with more \
                 than one control-plane replica each holds its own budget"
            );
        }
        login_throttle::LoginThrottle::new(
            login_throttle::ThrottleConfig {
                account_max_failures: args.login_max_failures,
                address_max_failures: args.login_ip_max_failures,
                window: std::time::Duration::from_secs(args.login_failure_window_secs),
                base_lock: std::time::Duration::from_secs(args.login_lock_secs),
                max_lock: std::time::Duration::from_secs(args.login_max_lock_secs),
                max_delay: std::time::Duration::from_millis(args.login_max_delay_ms),
                ..Default::default()
            },
            // same deployment secret the session tokens use, read directly
            // rather than through `auth::session_pepper` so the throttle stays
            // available in a build without the postgres feature. it only ever
            // salts the counter keys, so two deployments sharing one redis do
            // not share each other's lockouts
            std::env::var("ROLTER_SESSION_PEPPER").unwrap_or_default(),
            redis.clone(),
        )
    };

    let gateway_url = Arc::new(args.gateway_url.trim_end_matches('/').to_string());
    tracing::info!(gateway_url = %gateway_url, "proxying /gw/* to the gateway");
    // one bounded request to github at boot and every few hours, so the
    // dashboard's version hint never has a browser call github itself (#902).
    // opt-out for air-gapped deployments; offline is a debug line, not an error
    let update_check = update_check::UpdateChecker::from_env();
    update_check.spawn(http.clone());
    #[cfg(feature = "postgres")]
    let state = ControlState {
        store,
        config_owned,
        redis,
        clickhouse,
        egress: egress.clone(),
        currency: currency.clone(),
        admin_token,
        internal_token,
        http,
        gateway_url,
        public_url: public_url_from_env(),
        cors: Arc::default(),
        metrics: metrics.clone(),
        login_throttle: login_throttle.clone(),
        trust_forwarded_for: args.login_trust_forwarded_for,
        update_check: update_check.clone(),
        pool: pool.clone(),
    };
    #[cfg(not(feature = "postgres"))]
    let state = ControlState {
        store,
        config_owned,
        redis,
        clickhouse,
        egress: egress.clone(),
        currency: currency.clone(),
        admin_token,
        internal_token,
        http,
        gateway_url,
        public_url: public_url_from_env(),
        cors: Arc::default(),
        metrics: metrics.clone(),
        login_throttle: login_throttle.clone(),
        trust_forwarded_for: args.login_trust_forwarded_for,
        update_check: update_check.clone(),
    };

    // converge the clickhouse ttl with the stored retention policy. spawned
    // rather than awaited: clickhouse is optional and may be slower to come up
    // than the control plane, and serving traffic must not wait on it
    #[cfg(feature = "postgres")]
    {
        let state = state.clone();
        tokio::spawn(async move { logging_settings::reconcile_retention(&state).await });
    }

    // load the cross-origin policy before the listener opens rather than in a
    // task: a split-origin dashboard hitting a control plane that has not read
    // its own settings yet would be refused, which looks exactly like the
    // misconfiguration this feature exists to fix (#813)
    #[cfg(feature = "postgres")]
    cors::refresh(&state).await;

    // when the operator gave /internal/* its own socket, it is not mounted on
    // the public router at all — the plaintext-credential channel is absent
    // from that surface rather than merely gated on it
    let split_internal = args.internal_addr.is_some();
    // the dashboard is built ahead of time, so per-deployment values are
    // injected into its html on the way out rather than baked in at build
    // time. rendered once: none of it can change without a restart
    // one kill switch covers every signal, the dashboard's browser tracing
    // included (#812): a deployment that turned telemetry off must not have the
    // SPA quietly shipping spans to a collector from the user's machine
    let telemetry_enabled = rolter_core::telemetry::export_enabled();
    if !telemetry_enabled {
        tracing::info!(
            "{} is off; no telemetry is exported",
            rolter_core::telemetry::TELEMETRY_ENABLED_ENV
        );
    }
    let ui_runtime = ui_config::UiRuntimeConfig {
        otel_endpoint: telemetry_enabled
            .then(|| args.ui_otel_endpoint.clone())
            .flatten(),
        otel_service_name: args.ui_otel_service_name.clone(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        open_mode: open_mode.is_open(),
    };
    if ui_runtime.is_configured() {
        tracing::info!(
            endpoint = ui_runtime.otel_endpoint.as_deref().unwrap_or_default(),
            "dashboard browser tracing enabled"
        );
    }
    let index = load_index(&args.ui_dir, &ui_runtime);

    let app = build_app_with(state.clone(), !split_internal)
        .fallback_service(spa_fallback(&args.ui_dir, index));

    tracing::info!(%addr, ui_dir = %args.ui_dir.display(), "rolter-control listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;

    // connect info carries the socket peer address into the request, which is
    // the only client identity the failed-login throttle can trust (#1079)
    let app = app.into_make_service_with_connect_info::<SocketAddr>();

    let Some(internal_addr) = args.internal_addr else {
        axum::serve(listener, app).await?;
        return Ok(());
    };

    let internal = internal_routes(&state).with_state(state);
    tracing::info!(addr = %internal_addr, "rolter-control internal api listening");
    let internal_listener = tokio::net::TcpListener::bind(internal_addr).await?;
    // both listeners share the process: if either dies the control plane is
    // degraded (no config propagation, or no dashboard), so exit rather than
    // limp on with half a control plane
    tokio::try_join!(async { axum::serve(listener, app).await }, async {
        axum::serve(internal_listener, internal).await
    },)?;
    Ok(())
}

/// Everything the API did not match falls through to the built SPA.
///
/// `ServeDir` answers first, so a real file in `ui_dir` — the JS bundle, the CSS,
/// the tab icons (#941) — is served as itself. Only a path with no file behind
/// it reaches the inner fallback and gets `index.html`, which is what lets a
/// deep link like `/models` survive a refresh instead of 404ing.
///
/// `ServeDir` must not serve `index.html` on its own, hence
/// `append_index_html_on_directories(false)`: every route that ends at the SPA
/// has to come through the fallback, or it arrives without the injected runtime
/// config and the dashboard's tracing stays inert.
///
/// The API surfaces are carved out ahead of `ServeDir` (#928): a typo'd
/// `/api/v1/keys` used to answer `200 text/html`, so a client checking
/// `response.ok` read a wrong path as success and only failed later, inside
/// `JSON.parse`. Those prefixes belong to the API, and an unmatched path under
/// them is a `404` in the same JSON shape every other API error uses.
fn spa_fallback(ui_dir: &std::path::Path, index: Option<String>) -> axum::routing::MethodRouter {
    let files = ServeDir::new(ui_dir)
        .append_index_html_on_directories(false)
        .fallback(get(move || {
            let index = index.clone();
            async move {
                match index {
                    Some(html) => Html(html).into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
        }));
    any(move |request: axum::extract::Request| {
        let files = files.clone();
        async move {
            if is_api_path(request.uri().path()) {
                return api_not_found(request.uri().path());
            }
            match files.oneshot(request).await {
                Ok(response) => response.into_response(),
                Err(err) => {
                    tracing::warn!(%err, "serving the dashboard failed");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
    })
}

/// Paths owned by the API rather than by the dashboard's client-side router.
///
/// Trailing-slash-free prefixes would also match `/apiary`, so each carries its
/// separator; the bare prefix itself is included because `/internal` with no
/// suffix is just as much an API path as `/internal/snapshot`.
fn is_api_path(path: &str) -> bool {
    const PREFIXES: [&str; 2] = ["/api", "/internal"];
    PREFIXES.iter().any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|r| r.starts_with('/'))
    })
}

/// The API's 404, in the error shape `ApiError` renders everywhere else.
fn api_not_found(path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": {
            "message": format!("no such endpoint: {path}"),
            // stable code so the dashboard can tell "this route is not mounted
            // on this control plane" (a store-less deployment) from any other 404
            "code": "no_such_endpoint",
        }})),
    )
        .into_response()
}

/// Read the dashboard's `index.html` out of `ui_dir` and inject the runtime
/// config into it.
///
/// Returns `None` when the dashboard was never built into `ui_dir`. The control
/// plane is a perfectly useful API without a dashboard — `rolter-control` is
/// run headless in plenty of deployments — so a missing SPA is a warning and a
/// 404 on the dashboard routes, not a refusal to start.
fn load_index(ui_dir: &std::path::Path, config: &ui_config::UiRuntimeConfig) -> Option<String> {
    let path = ui_dir.join("index.html");
    match std::fs::read_to_string(&path) {
        Ok(html) => Some(ui_config::inject(&html, config)),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "no dashboard index.html; serving the api without a dashboard"
            );
            None
        }
    }
}

/// How long the readiness probe waits for a pooled connection before calling
/// the database unreachable. Deliberately shorter than a typical probe
/// `timeoutSeconds`, so an exhausted pool answers `503` instead of hanging
/// until kubelet gives up and reports a timeout that looks like a dead process.
#[cfg(feature = "postgres")]
const READY_DB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Readiness: can this process actually serve control-plane traffic?
///
/// Distinct from `/healthz`, which answers only "is the process alive". The
/// two questions have opposite failure handling, which is why they are two
/// endpoints (#1081):
///
/// - a failing **readiness** probe takes the pod out of the Service, so a pod
///   that cannot reach its database stops receiving dashboard and
///   `/internal/snapshot` traffic while it recovers
/// - a failing **liveness** probe *kills* the pod, so it must never depend on
///   Postgres — a database blip would otherwise restart every control pod at
///   once and turn a recoverable dependency outage into a restart storm
///
/// Ready requires: the Postgres pool hands out a connection, every embedded
/// migration is applied, and the KEK parses when `ROLTER_KEK` is set. Redis and
/// ClickHouse are deliberately *not* checked — the control plane serves
/// configuration without either, so their absence is degraded, not unready.
///
/// Without a database (`--config` only, no `--database-url`) there is nothing
/// to wait on and the answer is always ready.
async fn readyz(State(state): State<ControlState>) -> Response {
    let mut checks = serde_json::Map::new();
    #[allow(unused_mut)]
    let mut ready = true;

    #[cfg(feature = "postgres")]
    if let Some(pool) = state.pool.as_ref() {
        match tokio::time::timeout(READY_DB_TIMEOUT, pool.acquire()).await {
            Ok(Ok(_conn)) => {
                checks.insert("database".into(), json!("ok"));
                match rolter_store::postgres::pending_migrations(pool).await {
                    Ok(pending) if pending.is_empty() => {
                        checks.insert("migrations".into(), json!("ok"));
                    }
                    Ok(pending) => {
                        ready = false;
                        checks.insert(
                            "migrations".into(),
                            json!(format!("{} pending", pending.len())),
                        );
                    }
                    Err(err) => {
                        ready = false;
                        checks.insert("migrations".into(), json!(err.to_string()));
                    }
                }
            }
            Ok(Err(err)) => {
                ready = false;
                checks.insert("database".into(), json!(err.to_string()));
                checks.insert("migrations".into(), json!("unknown"));
            }
            Err(_) => {
                ready = false;
                checks.insert("database".into(), json!("acquire timed out"));
                checks.insert("migrations".into(), json!("unknown"));
            }
        }
        // a KEK that is set but unusable seals nothing and decrypts nothing, so
        // a pod holding one cannot serve credentials on /internal/snapshot
        checks.insert(
            "kek".into(),
            match std::env::var(rolter_store::postgres::crypto::KEK_ENV) {
                Ok(secret) if secret.trim().is_empty() => {
                    ready = false;
                    json!("set but empty")
                }
                Ok(_) => json!("ok"),
                Err(_) => json!("not configured"),
            },
        );
    } else {
        checks.insert("database".into(), json!("not configured"));
    }

    #[cfg(not(feature = "postgres"))]
    {
        let _ = &state;
        checks.insert("database".into(), json!("not configured"));
    }

    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = json!({
        "status": if ready { "ready" } else { "not_ready" },
        "checks": Value::Object(checks),
    });
    (status, Json(body)).into_response()
}

/// Assemble the control-plane API router (no SPA fallback) with `state` applied.
/// The CRUD routes are only mounted when a postgres pool is present.
/// `mount_internal` controls whether `/internal/*` is served on this router at
/// all. It is `false` when the operator gave the internal routes their own
/// listener, so the plaintext-credential channel is simply absent from the
/// public API surface rather than merely token-gated.
fn build_app_with(state: ControlState, mount_internal: bool) -> Router {
    #[allow(unused_mut)]
    let mut api = Router::new()
        // liveness: the process is up and the runtime is not wedged. No
        // dependency checks live here on purpose — see `readyz`
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route(
            "/api/v1/ping",
            get(|| async { Json(json!({"pong": true})) }),
        )
        .route("/api/v1/roles", get(list_roles))
        .route("/api/v1/config", get(get_config))
        .route("/api/v1/provider-kinds", get(get_provider_kinds))
        .route("/api/v1/currency", get(get_currency))
        .route("/api/v1/config/problems", get(get_config_problems))
        .merge(analytics::router())
        .merge(health::router())
        // the served schema and its Scalar reference. mounted unconditionally:
        // the document describes the surface this binary can serve, so it must
        // not vary with whether a pool happened to be configured
        .merge(openapi::router())
        // reverse-proxy the gateway data plane for the dashboard Playground;
        // authenticated by the virtual key the gateway itself checks
        .merge(proxy::router());
    // login is authenticated by the request body (email/password), not the
    // admin token, so /api/v1/auth/* sits on the open router alongside
    // everything else here; `me` still requires a valid session bearer token
    // via the `CurrentUser` extractor, it's just not gated by admin_token.
    //
    // the CRUD API enforces RBAC per handler (see `crate::rbac`): each handler
    // resolves a `Principal` and checks the caller's role at the resource's
    // scope, so it is NOT behind the blanket admin-token layer. open mode (no
    // admin token) is preserved inside the `Principal` extractor.
    #[cfg(feature = "postgres")]
    if state.pool.is_some() {
        alerting::start_evaluator(state.clone());
        // renew MCP OAuth sessions before they lapse, so a user consents once
        // rather than every hour (#707)
        mcp_oauth_flow::start_refresher(state.clone());
        api = api
            .merge(access_control::router())
            .merge(alerting::router())
            .merge(auth::router())
            .merge(auth_policy::router())
            .merge(mfa::router())
            .merge(invitations::router())
            .merge(crud::router())
            .merge(me::router())
            .merge(plugins::router())
            .merge(mcp_logs::router())
            .merge(ui_events::router())
            .merge(mcp_oauth::router())
            .merge(mcp_oauth_flow::router())
            .merge(feature_flags::router())
            .merge(guardrails::router())
            .merge(labels::router())
            .merge(logging_settings::router())
            .merge(runtime_policy::router())
            .merge(compatibility_policy::router())
            .merge(client_settings::router())
            .merge(config_export::router())
            .merge(model_defaults::router())
            .merge(adaptive_policy::router())
            .merge(adaptive_telemetry::router())
            .merge(rbac_matrix::router())
            .merge(scim::router())
            .merge(scim_groups::router())
            .merge(sso::router())
            .merge(cluster::router())
            .merge(connectors::router())
            .merge(collector_config::router())
            .merge(security::router())
            .merge(stability::router())
            .merge(update_check::router());
    }

    if mount_internal {
        api = api.merge(internal_routes(&state));
    }
    // the cross-origin policy wraps everything, including the internal routes
    // and the SPA fallback, because a browser that is denied the API must be
    // denied it uniformly. it is inert until an operator configures an origin
    // (#813)
    // per-request spans and latency histograms wrap the whole API, outside the
    // cors layer so a preflight rejection is measured like any other response
    // (#845). inert without an OTLP endpoint
    api.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        cors::apply_cors,
    ))
    .layer(axum::middleware::from_fn_with_state(
        state.clone(),
        telemetry::record_request,
    ))
    .with_state(state)
}

/// The control↔data-plane routes. They carry decrypted provider credentials,
/// so they sit behind the internal token (falling back to the admin token) and
/// can be served on their own listener instead of the public API port — see
/// [`Args::internal_addr`].
fn internal_routes(state: &ControlState) -> Router<ControlState> {
    #[allow(unused_mut)]
    let mut routes = Router::new().route("/internal/snapshot", get(get_snapshot));
    // the reverse direction of the same channel: gateways push adaptive-routing
    // samples the dashboard reads back out of the control plane (#751)
    #[cfg(feature = "postgres")]
    {
        routes = routes.merge(adaptive_telemetry::internal_router());
    }
    routes.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        require_internal_token,
    ))
}

/// Reject requests to `/internal/*` lacking the internal bearer token.
///
/// The expected token is `--internal-token` when set, so an operator holding
/// only the admin token cannot read decrypted provider credentials. It falls
/// back to the admin token when no internal token is configured (the historical
/// behavior, kept so an existing deployment does not break on upgrade), and
/// passes through when neither is set.
async fn require_internal_token(
    State(state): State<ControlState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let expected = state
        .internal_token
        .as_deref()
        .or(state.admin_token.as_deref());
    let Some(expected) = expected else {
        return next.run(request).await;
    };
    let presented = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    // constant-time comparison so the token can't be recovered byte by byte
    let matches: bool =
        subtle::ConstantTimeEq::ct_eq(presented.as_bytes(), expected.as_bytes()).into();
    if !matches {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "missing or invalid internal token"}})),
        )
            .into_response();
    }
    next.run(request).await
}

/// Build a postgres-backed control-plane API router for integration tests.
/// Runs migrations on `pool`, mounts the full CRUD API and `/internal/snapshot`,
/// and omits redis/clickhouse and the SPA fallback. Intended to be served on an
/// ephemeral port by the test harness.
#[cfg(feature = "postgres")]
pub async fn test_app(pool: sqlx::PgPool) -> anyhow::Result<Router> {
    test_app_with_admin_token(pool, None).await
}

/// [`test_app`] with an admin token, for exercising the auth guard on the
/// CRUD API and `/internal/snapshot`.
#[cfg(feature = "postgres")]
pub async fn test_app_with_admin_token(
    pool: sqlx::PgPool,
    admin_token: Option<String>,
) -> anyhow::Result<Router> {
    rolter_store::postgres::run_migrations(&pool).await?;
    Ok(build_app_with(test_state(pool, admin_token, None), true))
}

/// [`test_app_with_admin_token`] with the control plane's public base URL
/// injected instead of read from `ROLTER_PUBLIC_URL`.
///
/// The SSO and MCP OAuth flows derive their redirect URI from that URL, and a
/// test that needs it to name its own ephemeral listener used to `set_var` it.
/// The environment is process-wide: under `cargo nextest` each test owns its
/// process and that is harmless, but the coverage job runs plain `cargo test`,
/// where every test in this binary is a thread sharing one environment, so one
/// test's address became another test's redirect URI. Passing the value in
/// makes the two runners behave identically (#1418).
#[cfg(feature = "postgres")]
pub async fn test_app_with_public_url(
    pool: sqlx::PgPool,
    admin_token: Option<String>,
    public_url: &str,
) -> anyhow::Result<Router> {
    rolter_store::postgres::run_migrations(&pool).await?;
    Ok(build_app_with(
        test_state(pool, admin_token, Some(public_url.to_string())),
        true,
    ))
}

/// [`test_app`] with the migrations deliberately *not* run, for exercising
/// `/readyz` against a database whose schema is behind the binary (#1081).
#[cfg(feature = "postgres")]
pub fn test_app_unmigrated(pool: sqlx::PgPool) -> Router {
    build_app_with(test_state(pool, None, None), true)
}

#[cfg(feature = "postgres")]
fn test_state(
    pool: sqlx::PgPool,
    admin_token: Option<String>,
    public_url: Option<String>,
) -> ControlState {
    let store: Arc<dyn ConfigStore> =
        Arc::new(rolter_store::PostgresConfigStore::new(pool.clone()));
    ControlState {
        store,
        config_owned: Arc::new(ConfigOwned::default()),
        redis: None,
        clickhouse: None,
        egress: Arc::new(Default::default()),
        currency: Arc::new(Default::default()),
        admin_token: admin_token.map(Arc::new),
        internal_token: None,
        http: reqwest::Client::new(),
        gateway_url: Arc::new("http://localhost:4000".to_string()),
        public_url: Arc::new(normalize_public_url(public_url)),
        cors: Arc::default(),
        metrics: Default::default(),
        // real, with production defaults: the wiring is part of what these
        // tests cover, and no test signs in wrongly five times by accident
        login_throttle: login_throttle::LoginThrottle::new(Default::default(), String::new(), None),
        trust_forwarded_for: false,
        // never on the network from a test; the endpoint's disabled shape is
        // what the integration test asserts
        update_check: update_check::UpdateChecker::disabled(),
        pool: Some(pool),
    }
}

/// Build the config store: postgres-backed when `--database-url` is set
/// (running migrations first; a bootstrap config, when given, is layered on
/// top as read-only config models via [`MergedConfigStore`]), otherwise an
/// in-memory store seeded from the bootstrap toml. Also returns the raw pool
/// (postgres builds only), which the CRUD API needs for direct repository
/// access.
/// How often the acquire-wait probe takes a sample. Long enough that the probe
/// itself is not measurable load, short enough to catch a saturation episode
/// that lasts a minute.
#[cfg(feature = "postgres")]
const POOL_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Periodically time one `acquire()` and record it, so pool saturation shows up
/// as a latency distribution an operator can alert on (#1052).
///
/// It takes a connection and drops it immediately. That costs one slot for the
/// duration of the acquire, which is exactly the cost of the thing being
/// measured: if taking a single connection every 15s is disruptive, the pool
/// is already far too small and that is the finding.
#[cfg(feature = "postgres")]
async fn sample_pool_acquire(
    pool: sqlx::PgPool,
    metrics: rolter_core::telemetry::ControlHistograms,
) {
    let mut ticker = tokio::time::interval(POOL_PROBE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let started = std::time::Instant::now();
        let outcome = match pool.acquire().await {
            Ok(_conn) => "ok",
            // the only expected error here is the acquire timeout, which is the
            // saturation signal itself rather than a fault to log per tick
            Err(_) => "timeout",
        };
        metrics.record_pool_acquire(outcome, started.elapsed().as_millis() as u64);
    }
}

/// Translate the pool flags into a [`rolter_store::postgres::PoolConfig`].
///
/// A zero timeout means "never" for the two expiry knobs and is passed through
/// as `None`; a zero `max_connections` or `acquire_timeout` is rejected at
/// startup instead of producing a pool that can never hand out a connection —
/// that failure would otherwise appear much later as every request timing out,
/// which reads like a database outage rather than a typo.
#[cfg(feature = "postgres")]
fn pool_config_from(args: &Args) -> anyhow::Result<rolter_store::postgres::PoolConfig> {
    use std::time::Duration;

    anyhow::ensure!(
        args.db_max_connections > 0,
        "ROLTER_DB_MAX_CONNECTIONS must be at least 1"
    );
    anyhow::ensure!(
        args.db_acquire_timeout_secs > 0,
        "ROLTER_DB_ACQUIRE_TIMEOUT_SECS must be at least 1"
    );
    anyhow::ensure!(
        args.db_min_connections <= args.db_max_connections,
        "ROLTER_DB_MIN_CONNECTIONS ({}) exceeds ROLTER_DB_MAX_CONNECTIONS ({})",
        args.db_min_connections,
        args.db_max_connections
    );

    let optional = |secs: u64| (secs > 0).then(|| Duration::from_secs(secs));
    Ok(rolter_store::postgres::PoolConfig {
        max_connections: args.db_max_connections,
        min_connections: args.db_min_connections,
        acquire_timeout: Duration::from_secs(args.db_acquire_timeout_secs),
        idle_timeout: optional(args.db_idle_timeout_secs),
        max_lifetime: optional(args.db_max_lifetime_secs),
    })
}

#[cfg(feature = "postgres")]
async fn build_store(
    args: &Args,
    bootstrap: Option<GatewayConfig>,
) -> anyhow::Result<(Arc<dyn ConfigStore>, Option<sqlx::PgPool>)> {
    if let Some(database_url) = &args.database_url {
        let pool_config = pool_config_from(args)?;
        tracing::info!(
            max_connections = pool_config.max_connections,
            min_connections = pool_config.min_connections,
            acquire_timeout_secs = pool_config.acquire_timeout.as_secs(),
            "connecting to postgres"
        );
        let pool = rolter_store::postgres::connect_with(database_url, pool_config).await?;
        rolter_store::postgres::run_migrations(&pool).await?;
        if let Some(config) = bootstrap.as_ref() {
            // providers first: a models.default target or a provider_groups.default
            // member may reference a provider that providers.default just seeded
            seed_default_providers(&pool, config).await?;
            seed_default_provider_groups(&pool, config).await?;
            seed_default_models(&pool, config).await?;
        }
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

/// Seed editable `[[models.default]]` routes exactly once. Defaults deliberately
/// target the bootstrap `default/default/default` tenancy created by `rolter
/// seed`; a deployment without that project is left untouched rather than
/// guessing a tenant. Existing rows are never overwritten on a restart.
#[cfg(feature = "postgres")]
async fn seed_default_models(pool: &sqlx::PgPool, config: &GatewayConfig) -> anyhow::Result<()> {
    if config.models.defaults.is_empty() {
        return Ok(());
    }
    let project: Option<(uuid::Uuid, uuid::Uuid)> = sqlx::query_as(
        "select p.id, o.id from projects p \
         join teams t on t.id = p.team_id \
         join orgs o on o.id = t.org_id \
         where o.slug = 'default' and t.name = 'default' and p.name = 'default' \
         limit 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((project_id, org_id)) = project else {
        tracing::warn!(
            "models.default was not seeded: create the default org/team/project first with rolter-seed"
        );
        return Ok(());
    };
    let routes = rolter_store::postgres::repo::RouteRepo(pool);
    let targets = rolter_store::postgres::repo::RouteTargetRepo(pool);
    let providers = rolter_store::postgres::repo::ProviderRepo(pool)
        .list(org_id)
        .await?;
    for route in &config.models.defaults {
        if routes
            .list(project_id)
            .await?
            .iter()
            .any(|existing| existing.model == route.model)
        {
            continue;
        }
        let strategy = match route.strategy {
            rolter_core::BalancingStrategy::RoundRobin => "round_robin",
            rolter_core::BalancingStrategy::Random => "random",
            rolter_core::BalancingStrategy::PowerOfTwo => "power_of_two",
            rolter_core::BalancingStrategy::ConsistentHash => "consistent_hash",
            rolter_core::BalancingStrategy::CacheAware => "cache_aware",
            rolter_core::BalancingStrategy::Weighted => "weighted",
            rolter_core::BalancingStrategy::Pipeline => "pipeline",
            rolter_core::BalancingStrategy::Cheapest => "cheapest",
            rolter_core::BalancingStrategy::Fastest => "fastest",
            rolter_core::BalancingStrategy::PreciseCacheAware => "precise_cache_aware",
            rolter_core::BalancingStrategy::LmcacheAware => "lmcache_aware",
            rolter_core::BalancingStrategy::Adaptive => "adaptive",
            rolter_core::BalancingStrategy::LoraAware => "lora_aware",
            rolter_core::BalancingStrategy::PredictedLatency => "predicted_latency",
        };
        let created = routes.create(project_id, &route.model, strategy).await?;
        let params = serde_json::to_value(&route.params)?;
        let policy = serde_json::to_value(&route.param_policy)?;
        routes.set_params(created.id, &params, &policy).await?;
        for target in &route.targets {
            if let Some(provider) = providers.iter().find(|p| p.name == target.provider) {
                targets
                    .create(
                        created.id,
                        provider.id,
                        target.model.as_deref(),
                        target.weight as i32,
                    )
                    .await?;
            } else {
                tracing::warn!(
                    model = %route.model,
                    provider = %target.provider,
                    "models.default target was not seeded because the provider is not DB-owned"
                );
            }
        }
        tracing::info!(model = %route.model, "seeded editable default model");
    }
    Ok(())
}

/// Resolve the bootstrap `default/default/default` `(project_id, org_id)`, or
/// `None` when it has not been created yet (a deployment without `rolter-seed`
/// is left untouched rather than guessing a tenant).
#[cfg(feature = "postgres")]
async fn default_project(pool: &sqlx::PgPool) -> anyhow::Result<Option<(uuid::Uuid, uuid::Uuid)>> {
    Ok(sqlx::query_as(
        "select p.id, o.id from projects p \
         join teams t on t.id = p.team_id \
         join orgs o on o.id = t.org_id \
         where o.slug = 'default' and t.name = 'default' and p.name = 'default' \
         limit 1",
    )
    .fetch_optional(pool)
    .await?)
}

/// The database `kind` string for a [`rolter_core::ProviderKind`], matching the
/// values decoded in `rolter_store::postgres`.
#[cfg(feature = "postgres")]
fn provider_kind_str(kind: &rolter_core::ProviderKind) -> &'static str {
    use rolter_core::ProviderKind::*;
    match kind {
        Openai => "openai",
        Anthropic => "anthropic",
        OpenaiCompatible => "openai_compatible",
        Ollama => "ollama",
        OllamaCloud => "ollama_cloud",
        LlamaCpp => "llama_cpp",
        Openrouter => "openrouter",
        Tei => "tei",
        AzureOpenai => "azure_openai",
        Bedrock => "bedrock",
        Vertex => "vertex",
        Gemini => "gemini",
        GeminiNative => "gemini_native",
        GeminiInteractions => "gemini_interactions",
        Mistral => "mistral",
        Groq => "groq",
        Xai => "xai",
        MetaLlamaApi => "meta_llama_api",
        Cohere => "cohere",
        Perplexity => "perplexity",
        Together => "together",
        Fireworks => "fireworks",
        Databricks => "databricks",
        AlephAlpha => "aleph_alpha",
        Nebius => "nebius",
        Ovhcloud => "ovhcloud",
        Scaleway => "scaleway",
        Deepseek => "deepseek",
        Qwen => "qwen",
        Zhipu => "zhipu",
        Kimi => "kimi",
        Ernie => "ernie",
        Doubao => "doubao",
        Hunyuan => "hunyuan",
        Yi => "yi",
        Minimax => "minimax",
        Baichuan => "baichuan",
        Gigachat => "gigachat",
        YandexGpt => "yandex_gpt",
        CloudRu => "cloud_ru",
        MtsAi => "mts_ai",
        Naver => "naver",
        Upstage => "upstage",
        Rinna => "rinna",
        Rakuten => "rakuten",
        Sarvam => "sarvam",
        Krutrim => "krutrim",
        Falcon => "falcon",
    }
}

/// Seed editable `[providers.default]` providers exactly once (ADR-0022). Like
/// `seed_default_models`, defaults target the bootstrap `default/default/default`
/// org; an existing provider with the same slug or name is never overwritten.
/// Seeded providers are DB-owned and editable — not config-owned.
#[cfg(feature = "postgres")]
async fn seed_default_providers(pool: &sqlx::PgPool, config: &GatewayConfig) -> anyhow::Result<()> {
    if config.provider_defaults.is_empty() {
        return Ok(());
    }
    let Some((_project_id, org_id)) = default_project(pool).await? else {
        tracing::warn!(
            "providers.default was not seeded: create the default org/team/project first with rolter-seed"
        );
        return Ok(());
    };
    let repo = rolter_store::postgres::repo::ProviderRepo(pool);
    let keys = rolter_store::postgres::repo::ProviderKeyRepo(pool);
    let existing = repo.list(org_id).await?;
    for provider in &config.provider_defaults {
        let slug = provider
            .slug
            .clone()
            .unwrap_or_else(|| rolter_core::slug::slugify(&provider.name));
        if existing
            .iter()
            .any(|p| p.name == provider.name || p.slug == slug)
        {
            continue;
        }
        let row = repo
            .create(
                org_id,
                &provider.name,
                &slug,
                provider_kind_str(&provider.kind),
                &provider.api_base,
                provider.api_key_env.as_deref(),
                provider.egress_proxy.as_deref(),
                &provider.egress_proxies,
            )
            .await?;
        // seal an inline api_key at rest; api_key_env stays a plaintext var name
        if let Some(api_key) = provider.api_key.as_deref() {
            use rolter_store::postgres::crypto::{Kek, KEK_ENV};
            match Kek::from_env() {
                Some(kek) => {
                    let (ciphertext, nonce) = kek.encrypt(api_key)?;
                    keys.set(row.id, &ciphertext, &nonce).await?;
                }
                None => tracing::warn!(
                    provider = %provider.name,
                    "providers.default inline api_key not sealed: {KEK_ENV} is unset; set api_key_env or the KEK"
                ),
            }
        }
        tracing::info!(provider = %provider.name, %slug, "seeded editable default provider");
    }
    Ok(())
}

/// Seed editable `[provider_groups.default]` groups exactly once (ADR-0022).
/// Like the other default seeds, groups target the bootstrap default org and an
/// existing group with the same slug is never overwritten. A member referencing
/// a provider that is not DB-owned is skipped with a warning. Runs after the
/// provider seed so a group member can reference a just-seeded provider.
#[cfg(feature = "postgres")]
async fn seed_default_provider_groups(
    pool: &sqlx::PgPool,
    config: &GatewayConfig,
) -> anyhow::Result<()> {
    if config.provider_group_defaults.is_empty() {
        return Ok(());
    }
    let Some((_project_id, org_id)) = default_project(pool).await? else {
        tracing::warn!(
            "provider_groups.default was not seeded: create the default org/team/project first with rolter-seed"
        );
        return Ok(());
    };
    let groups = rolter_store::postgres::repo::ProviderGroupRepo(pool);
    let providers = rolter_store::postgres::repo::ProviderRepo(pool)
        .list(org_id)
        .await?;
    let existing = groups.list(org_id).await?;
    for group in &config.provider_group_defaults {
        let slug = group
            .slug
            .clone()
            .unwrap_or_else(|| rolter_core::slug::slugify(&group.name));
        if existing.iter().any(|g| g.slug == slug) {
            continue;
        }
        let strategy = balancing_strategy_str(group.strategy);
        let created = groups.create(org_id, &group.name, &slug, strategy).await?;
        let mut members = Vec::with_capacity(group.members.len());
        for member in &group.members {
            match providers.iter().find(|p| p.name == member.provider) {
                Some(provider) => members.push((
                    provider.id,
                    member.model.clone(),
                    member.weight.max(1) as i32,
                )),
                None => tracing::warn!(
                    group = %group.name,
                    provider = %member.provider,
                    "provider_groups.default member skipped: provider is not DB-owned"
                ),
            }
        }
        groups.set_members(created.id, &members).await?;
        tracing::info!(group = %group.name, %slug, "seeded editable default provider group");
    }
    Ok(())
}

/// The database `strategy` string for a [`rolter_core::BalancingStrategy`],
/// matching the values `rolter_store::postgres::parse_strategy` decodes.
#[cfg(feature = "postgres")]
fn balancing_strategy_str(strategy: rolter_core::BalancingStrategy) -> &'static str {
    use rolter_core::BalancingStrategy::*;
    match strategy {
        RoundRobin => "round_robin",
        Random => "random",
        PowerOfTwo => "power_of_two",
        ConsistentHash => "consistent_hash",
        CacheAware => "cache_aware",
        Weighted => "weighted",
        Pipeline => "pipeline",
        Cheapest => "cheapest",
        Fastest => "fastest",
        PreciseCacheAware => "precise_cache_aware",
        LmcacheAware => "lmcache_aware",
        Adaptive => "adaptive",
        LoraAware => "lora_aware",
        PredictedLatency => "predicted_latency",
    }
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
    let mut config = state.store.load().await.unwrap_or_default();
    redact_config_for_dashboard(&mut config);
    Json(config)
}

/// Strip everything a caller of the dashboard's config view must not learn.
///
/// This endpoint sits on the open router, so it answers without a session or
/// the admin token. It used to blank only the provider credentials, and shipped
/// the rest of the snapshot as-is: the plaintext of every `[[virtual_keys]]`
/// entry from `rolter.toml`, every live MCP OAuth access token, the peppered
/// digests of the database keys, and any userinfo embedded in the ClickHouse
/// or egress-proxy URLs. Secrets stay between the store and the gateway, which
/// reads the token-guarded `/internal/snapshot` instead.
fn redact_config_for_dashboard(config: &mut GatewayConfig) {
    const REDACTED: &str = "[redacted]";
    for provider in &mut config.providers {
        provider.api_key = None;
        for key in &mut provider.api_keys {
            key.key = None;
        }
        provider.egress_proxy = provider.egress_proxy.as_deref().map(strip_userinfo);
        for proxy in &mut provider.egress_proxies {
            *proxy = strip_userinfo(proxy);
        }
    }
    for provider in &mut config.provider_defaults {
        provider.api_key = None;
        for key in &mut provider.api_keys {
            key.key = None;
        }
    }
    for key in &mut config.virtual_keys {
        key.key = REDACTED.to_string();
    }
    for key in &mut config.db_virtual_keys {
        key.key_hash.clear();
    }
    // the dashboard never reads these; the gateway takes them from the snapshot
    config.mcp_oauth_sessions.clear();
    config.logging.clickhouse_url = config.logging.clickhouse_url.as_deref().map(strip_userinfo);
}

/// `scheme://user:pass@host/…` -> `scheme://host/…`; anything unparsable is
/// returned untouched, since a URL the gateway could not use leaks nothing.
fn strip_userinfo(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) if !parsed.username().is_empty() || parsed.password().is_some() => {
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        _ => url.to_string(),
    }
}

/// What the dashboard needs to know about a provider kind to configure it.
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
struct ProviderKindInfo {
    /// the wire value stored in `providers.kind`
    kind: String,
    /// whether `api_base` must already end in the API version prefix
    ///
    /// The base-URL field used to show one static `.../v1` hint for every kind.
    /// That is right for the ~38 kinds rolter strips the prefix for and wrong
    /// for the openai-shaped ones — including the default — where it produces
    /// `/v1/v1/chat/completions` (#947).
    base_includes_v1: bool,
}

/// The per-kind `api_base` rule, so the dashboard states it instead of guessing.
///
/// Derived from `ProviderKind::ALL` rather than re-listed, so a kind added to
/// core shows up here without a second edit.
async fn get_provider_kinds() -> Json<Vec<ProviderKindInfo>> {
    Json(
        rolter_core::ProviderKind::ALL
            .iter()
            .map(|kind| ProviderKindInfo {
                // the enum serializes to exactly the stored wire value
                kind: serde_json::to_value(kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default(),
                base_includes_v1: kind.base_includes_v1(),
            })
            .collect(),
    )
}

/// The deployment's settlement currency and the codes it can price in.
///
/// Served separately from `/api/v1/config` on purpose. That endpoint answers
/// from the *store*, and the postgres store has no currency table — it holds a
/// currency per model price and nothing else — so a database-backed deployment
/// reads back the `CurrencyConfig` default (USD, no rates) no matter what the
/// bootstrap config says. This answers from `ControlState.currency`, the same
/// value `require_known_currency` validates writes against.
#[derive(Debug, serde::Serialize, PartialEq)]
struct CurrencySettings {
    /// currency budgets and accumulated spend are denominated in
    base: String,
    /// every code a price may be stored in, base first (see
    /// [`rolter_core::CurrencyConfig::codes`])
    codes: Vec<String>,
    /// units of `base` per unit of the keyed currency, for the codes that have
    /// one; the base is implicitly 1.0 and is not listed
    rates: std::collections::HashMap<String, f64>,
}

/// The currency table the dashboard drives its chooser from.
///
/// Unauthenticated alongside `/api/v1/config` and `/api/v1/roles`: an operator's
/// rate table is deployment configuration the pricing screens already display,
/// not a credential.
async fn get_currency(State(state): State<ControlState>) -> Json<CurrencySettings> {
    let codes = state.currency.codes();
    // report rates under the same normalized spelling as `codes`, so the
    // dashboard can look one up by the code it was handed
    let rates = codes
        .iter()
        .filter_map(|code| state.currency.rate(code).map(|rate| (code.clone(), rate)))
        .collect();
    Json(CurrencySettings {
        base: state.currency.base_code(),
        codes,
        rates,
    })
}

/// What the snapshot is *not* serving, and why (#926).
///
/// Since a malformed entry is now dropped from `/internal/snapshot` rather than
/// withholding the whole fleet's config, nothing in the dashboard would
/// otherwise say a provider stopped being served — the failure would be silent
/// until someone wondered why a change never took effect. This is the same
/// computation the snapshot runs, so the two cannot report different things.
///
/// Unguarded, alongside [`get_config`]: it carries no credentials, only the
/// names and reasons an operator needs to fix their own config.
async fn get_config_problems(State(state): State<ControlState>) -> Json<Value> {
    let mut config = state.store.load().await.unwrap_or_default();
    let mut problems = config.sanitize_for_snapshot();
    // structural problems never reach a gateway at all — the snapshot refuses
    // outright — so an operator needs to see those here too, not just in a log
    if let Err(fatal) = config.validate() {
        problems.extend(fatal);
    }
    Json(json!({ "problems": problems }))
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    /// the gateway's last-seen config version; if it's already current, the
    /// control plane replies `304 Not Modified` with no body
    version: Option<i64>,
}

/// Attach the polling node's operator-requested state, so a drain reaches it on
/// the channel it already polls. Absent when the caller is not a known node.
#[cfg(feature = "postgres")]
fn with_node_state(mut response: Response, desired_state: &Option<String>) -> Response {
    if let Some(value) = desired_state
        .as_deref()
        .and_then(|state| axum::http::HeaderValue::from_str(state).ok())
    {
        response
            .headers_mut()
            .insert(cluster::NODE_STATE_HEADER, value);
    }
    response
}

/// Without the store there is no inventory, so nothing is attached.
#[cfg(not(feature = "postgres"))]
fn with_node_state(response: Response, _desired_state: &Option<String>) -> Response {
    response
}

/// Runtime snapshot endpoint gateways poll to pick up config changes without
/// a restart. Returns `{"version": N, "config": GatewayConfig}`, or `304` if
/// the caller's `version` is already current.
async fn get_snapshot(
    State(state): State<ControlState>,
    headers: axum::http::HeaderMap,
    Query(query): Query<SnapshotQuery>,
) -> Response {
    use tracing::Instrument as _;

    // snapshot generation is fleet-wide propagation delay: every gateway waits
    // on it for its config, and until #809 it was invisible. the stage span
    // costs nothing when no OTLP pipeline is installed.
    //
    // the fields are declared `Empty` and recorded after the build, because
    // what makes this span answer an operator's question — which version, how
    // big, did it succeed — is only known at the end (#845)
    let span = rolter_core::stage_span!(
        "snapshot.build",
        config_version = tracing::field::Empty,
        payload_bytes = tracing::field::Empty,
        outcome = tracing::field::Empty,
    );
    let metrics = state.metrics.clone();
    let started = std::time::Instant::now();

    // `.instrument`, not a held guard: a span that is created but never entered
    // does not parent the spans created inside it, so `snapshot.sanitize` came
    // out as a sibling of `snapshot.build` rather than a child of it
    let built = build_snapshot(state, headers, query)
        .instrument(span.clone())
        .await;

    span.record("config_version", built.version);
    span.record("payload_bytes", built.bytes);
    span.record("outcome", built.outcome);
    metrics.record_snapshot(
        built.outcome,
        started.elapsed().as_millis() as u64,
        built.bytes,
    );
    built.response
}

/// One snapshot build, with the numbers the span and histograms need.
struct BuiltSnapshot {
    response: Response,
    /// Bounded (`ok` / `not_modified` / `error`), so it is safe as a metric
    /// attribute — unlike `version`, which is unbounded and stays on the span.
    outcome: &'static str,
    version: i64,
    /// Serialized payload size. `0` for a 304, which transfers no body.
    bytes: u64,
}

impl BuiltSnapshot {
    fn error(response: Response, version: i64) -> Self {
        Self {
            response,
            outcome: "error",
            version,
            bytes: 0,
        }
    }
}

async fn build_snapshot(
    state: ControlState,
    headers: axum::http::HeaderMap,
    query: SnapshotQuery,
) -> BuiltSnapshot {
    // a node that identifies itself is recorded in the cluster inventory; the
    // poll it already makes is the heartbeat, so there is no second channel
    #[cfg(feature = "postgres")]
    let desired_state = cluster::record_heartbeat(&state, &headers, query.version).await;
    #[cfg(not(feature = "postgres"))]
    let desired_state: Option<String> = None;
    let _ = &headers;
    let version = match state.store.current_version().await {
        Ok(v) => v,
        Err(err) => {
            return BuiltSnapshot::error(
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": {"message": err.to_string()}})),
                )
                    .into_response(),
                0,
            )
        }
    };
    if query.version.is_some_and(|requested| requested >= version) {
        return BuiltSnapshot {
            response: with_node_state(StatusCode::NOT_MODIFIED.into_response(), &desired_state),
            outcome: "not_modified",
            version,
            bytes: 0,
        };
    }
    match state.store.load().await {
        Ok(mut config) => {
            // prune rows that are only unservable by themselves (a route whose
            // targets aren't set up yet) so one half-built entry can't 500 the
            // snapshot and freeze config propagation for every other tenant
            let sanitize = rolter_core::stage_span!("snapshot.sanitize");
            let omitted = {
                let _entered = sanitize.enter();
                config.sanitize_for_snapshot()
            };
            if !omitted.is_empty() {
                tracing::warn!(
                    ?omitted,
                    "omitted unservable entries from the config snapshot"
                );
            }
            // #929: the gateway learned where to write usage rows from its own
            // bootstrap TOML while the dashboard read them from the control
            // plane's `CLICKHOUSE_URL`, and nothing connected the two — so a
            // fleet could log nowhere while the analytics screens queried a
            // table nobody filled, with the snapshot itself carrying
            // `"clickhouse_url": null`. The control plane's own destination is
            // now what the fleet inherits. A gateway that names one in its
            // bootstrap config keeps it: an explicit local decision still wins.
            if config.logging.clickhouse_url.is_none() {
                if let Some(url) = state
                    .clickhouse
                    .as_ref()
                    .map(analytics::ClickHouseClient::base)
                {
                    config.logging.clickhouse_url = Some(url.to_string());
                }
            }
            // what survives here is structural: duplicate names, a bad
            // metrics_path, an unreadable CA bundle. Those genuinely cannot be
            // served — dropping one of two colliding rows would be a guess —
            // so they still refuse. Row-local defects were pruned above and
            // ride out in `problems` instead of withholding the fleet (#926)
            if let Err(problems) = config.validate() {
                tracing::error!(?problems, "refusing to serve invalid config snapshot");
                return BuiltSnapshot::error(
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": {
                            "message": "config failed validation",
                            "problems": problems,
                        }})),
                    )
                        .into_response(),
                    version,
                );
            }
            // serialized here rather than by `Json`'s `IntoResponse` so the
            // payload size is known: it is what every gateway in the fleet
            // transfers on every poll, and the thing to look at first when
            // propagation gets slow without generation getting slower
            let encode = rolter_core::stage_span!("snapshot.encode");
            let body = {
                let _entered = encode.enter();
                // `problems` rides along so the gateway can log what is not
                // being served and the dashboard can say "this provider is not
                // being served, because ...". Omitted when empty: it is on
                // every poll of every gateway in the fleet
                if omitted.is_empty() {
                    serde_json::to_vec(&json!({"version": version, "config": config}))
                } else {
                    serde_json::to_vec(
                        &json!({"version": version, "config": config, "problems": omitted}),
                    )
                }
            };
            let body = match body {
                Ok(body) => body,
                Err(err) => {
                    return BuiltSnapshot::error(
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": {"message": err.to_string()}})),
                        )
                            .into_response(),
                        version,
                    )
                }
            };
            let bytes = body.len() as u64;
            let mut response = axum::response::Response::new(axum::body::Body::from(body));
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            BuiltSnapshot {
                response: with_node_state(response, &desired_state),
                outcome: "ok",
                version,
                bytes,
            }
        }
        Err(err) => BuiltSnapshot::error(
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"message": err.to_string()}})),
            )
                .into_response(),
            version,
        ),
    }
}

#[cfg(all(test, feature = "postgres"))]
mod pool_config_tests {
    use super::*;
    use std::time::Duration;

    /// Parse `Args` from a flag list, with the required-in-practice bits set.
    /// `try_parse_from` so a bad value is a value to assert on, not a process
    /// exit inside the test binary.
    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["rolter-control", "--database-url", "postgres://x/y"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("parse args")
    }

    /// The whole point of the defaults: an existing deployment that configures
    /// nothing keeps exactly the budget it had before this was configurable.
    #[test]
    fn defaults_reproduce_the_previously_hardcoded_budget() {
        let config = pool_config_from(&args(&[])).expect("valid");
        let hardcoded = rolter_store::postgres::PoolConfig::default();
        assert_eq!(config.max_connections, 10);
        assert_eq!(config.max_connections, hardcoded.max_connections);
        assert_eq!(config.acquire_timeout, hardcoded.acquire_timeout);
        assert_eq!(config.idle_timeout, hardcoded.idle_timeout);
        assert_eq!(config.max_lifetime, hardcoded.max_lifetime);
    }

    #[test]
    fn flags_override_every_knob() {
        let config = pool_config_from(&args(&[
            "--db-max-connections",
            "50",
            "--db-min-connections",
            "5",
            "--db-acquire-timeout-secs",
            "3",
            "--db-idle-timeout-secs",
            "60",
            "--db-max-lifetime-secs",
            "120",
        ]))
        .expect("valid");
        assert_eq!(config.max_connections, 50);
        assert_eq!(config.min_connections, 5);
        assert_eq!(config.acquire_timeout, Duration::from_secs(3));
        assert_eq!(config.idle_timeout, Some(Duration::from_secs(60)));
        assert_eq!(config.max_lifetime, Some(Duration::from_secs(120)));
    }

    /// Zero means "never expire" for the two expiry knobs — sqlx's own
    /// encoding of it is `None`, and an operator should not have to know that.
    #[test]
    fn zero_expiry_means_never() {
        let config = pool_config_from(&args(&[
            "--db-idle-timeout-secs",
            "0",
            "--db-max-lifetime-secs",
            "0",
        ]))
        .expect("valid");
        assert_eq!(config.idle_timeout, None);
        assert_eq!(config.max_lifetime, None);
    }

    /// A pool that can never hand out a connection must fail at startup, not
    /// later as every request timing out — which reads like a database outage
    /// rather than a typo in one env var.
    #[test]
    fn impossible_budgets_are_rejected_at_startup() {
        for (flags, expected) in [
            (vec!["--db-max-connections", "0"], "MAX_CONNECTIONS"),
            (vec!["--db-acquire-timeout-secs", "0"], "ACQUIRE_TIMEOUT"),
            (vec!["--db-min-connections", "11"], "MIN_CONNECTIONS"),
        ] {
            let err = pool_config_from(&args(&flags)).expect_err("should be rejected");
            assert!(
                err.to_string().contains(expected),
                "error should name the offending variable, got: {err}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `Args::default()` restates clap's `default_value`s, so a knob retuned on
    // the field but not in the impl would silently give an embedder a
    // different budget than `rolter control` runs with. read the declared
    // defaults off the `Command` rather than parsing an empty argv: parsing
    // would pick up whatever `ROLTER_*` the test machine exports
    #[test]
    fn default_args_match_the_declared_clap_defaults() {
        use clap::CommandFactory;

        let command = Args::command();
        let declared = |name: &str| -> Option<String> {
            command
                .get_arguments()
                .find(|arg| arg.get_id() == name)
                .and_then(|arg| arg.get_default_values().first().cloned())
                .map(|value| value.to_string_lossy().into_owned())
        };

        let args = Args::default();
        #[cfg_attr(not(feature = "postgres"), allow(unused_mut))]
        let mut expected = vec![
            ("host", args.host.clone()),
            ("port", args.port.to_string()),
            ("ui_dir", args.ui_dir.display().to_string()),
            ("gateway_url", args.gateway_url.clone()),
            (
                "login_throttle_disabled",
                args.login_throttle_disabled.to_string(),
            ),
            ("login_max_failures", args.login_max_failures.to_string()),
            (
                "login_ip_max_failures",
                args.login_ip_max_failures.to_string(),
            ),
            (
                "login_failure_window_secs",
                args.login_failure_window_secs.to_string(),
            ),
            ("login_lock_secs", args.login_lock_secs.to_string()),
            ("login_max_lock_secs", args.login_max_lock_secs.to_string()),
            ("login_max_delay_ms", args.login_max_delay_ms.to_string()),
            (
                "login_trust_forwarded_for",
                args.login_trust_forwarded_for.to_string(),
            ),
        ];
        #[cfg(feature = "postgres")]
        expected.extend([
            ("db_max_connections", args.db_max_connections.to_string()),
            ("db_min_connections", args.db_min_connections.to_string()),
            (
                "db_acquire_timeout_secs",
                args.db_acquire_timeout_secs.to_string(),
            ),
            (
                "db_idle_timeout_secs",
                args.db_idle_timeout_secs.to_string(),
            ),
            (
                "db_max_lifetime_secs",
                args.db_max_lifetime_secs.to_string(),
            ),
        ]);

        for (name, value) in expected {
            assert_eq!(
                declared(name).as_deref(),
                Some(value.as_str()),
                "Args::default().{name} drifted from the clap default_value on that field"
            );
        }

        // the optional knobs carry no clap default, so `None`/`false` is the
        // only answer that matches
        assert!(args.admin_token.is_none());
        assert!(args.internal_token.is_none());
        assert!(args.internal_addr.is_none());
        assert!(args.redis_url.is_none());
        assert!(args.clickhouse_url.is_none());
        assert!(args.config.is_none());
        assert!(args.ui_otel_endpoint.is_none());
        assert!(args.ui_otel_service_name.is_none());
        assert!(!args.allow_open_mode);
        #[cfg(feature = "postgres")]
        assert!(args.database_url.is_none());
    }

    // the dashboard config view answers without a session: nothing in it may
    // be a credential, however it got there
    #[test]
    fn dashboard_config_carries_no_secrets() {
        use rolter_core::config::{
            McpOAuthSessionConfig, ProviderConfig, VirtualKeyConfig, VirtualKeyRecord,
        };
        let mut config = GatewayConfig::default();
        config.providers.push(ProviderConfig {
            name: "p".into(),
            api_key: Some("sk-live".into()),
            egress_proxy: Some("http://user:pw@proxy.internal:3128".into()),
            egress_proxies: vec!["socks5://u:p@edge:1080".into()],
            ..Default::default()
        });
        config.virtual_keys.push(VirtualKeyConfig {
            key: "sk-rolter-plaintext".into(),
            ..Default::default()
        });
        // the record and the session have no Default; parse the smallest
        // JSON that carries the secret instead of spelling every field out
        config.db_virtual_keys.push(
            serde_json::from_value::<VirtualKeyRecord>(json!({"key_hash": "deadbeef", "id": "k"}))
                .unwrap(),
        );
        config.mcp_oauth_sessions.push(
            serde_json::from_value::<McpOAuthSessionConfig>(json!({
                "id": "s", "server_id": "srv", "user_id": "u",
                "expires_at": "2030-01-01T00:00:00Z", "access_token": "ya29.secret"
            }))
            .unwrap(),
        );
        config.logging.clickhouse_url = Some("http://ch:pass@clickhouse:8123".into());

        redact_config_for_dashboard(&mut config);

        let json = serde_json::to_string(&config).unwrap();
        for secret in [
            "sk-live",
            "sk-rolter-plaintext",
            "deadbeef",
            "ya29.secret",
            "user:pw",
            "u:p@",
            "ch:pass",
        ] {
            // the message names the seed, not the serialised document: a
            // failing run must not print the very thing it is guarding
            assert!(
                !json.contains(secret),
                "a seeded secret survived redaction (seed #{})",
                secret.len()
            );
        }
        assert_eq!(
            config.providers[0].egress_proxy.as_deref(),
            Some("http://proxy.internal:3128/")
        );
        assert!(config.mcp_oauth_sessions.is_empty());
        assert_eq!(
            config.logging.clickhouse_url.as_deref(),
            Some("http://clickhouse:8123/")
        );
    }

    #[test]
    fn strip_userinfo_leaves_plain_urls_alone() {
        assert_eq!(
            strip_userinfo("http://clickhouse:8123"),
            "http://clickhouse:8123"
        );
        assert_eq!(strip_userinfo("not a url"), "not a url");
    }

    /// A scratch `ui_dir`, removed when the guard drops. No `tempfile` in this
    /// crate's dev-dependencies, and one directory is not worth adding one.
    struct ScratchUiDir(std::path::PathBuf);

    impl ScratchUiDir {
        fn with_index(name: &str, html: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("rolter-ui-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("create scratch ui dir");
            std::fs::write(dir.join("index.html"), html).expect("write index.html");
            Self(dir)
        }
    }

    impl Drop for ScratchUiDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_index_injects_the_runtime_config_into_the_built_dashboard() {
        let dir = ScratchUiDir::with_index(
            "injects",
            "<!doctype html><html><head><title>rolter</title></head><body></body></html>",
        );
        let config = ui_config::UiRuntimeConfig {
            otel_endpoint: Some("http://localhost:4318/v1/traces".to_string()),
            ..Default::default()
        };

        let html = load_index(&dir.0, &config).expect("index.html was read");

        assert!(html.contains("window.__ROLTER_CONFIG__"), "{html}");
        assert!(html.contains("http://localhost:4318/v1/traces"), "{html}");
        assert!(
            html.contains("<title>rolter</title>"),
            "original kept: {html}"
        );
    }

    /// #941 added tab icons to `ui/public`, which the build copies verbatim
    /// into `ui/dist`. Shipping them is only half the job: if the SPA fallback
    /// answered first, `/favicon.ico` would return `index.html` with a
    /// `text/html` type and the browser would render no icon at all.
    #[tokio::test]
    async fn a_static_asset_is_served_as_itself_not_as_the_spa() {
        let dir = ScratchUiDir::with_index("assets", "<html>the dashboard</html>");
        std::fs::write(dir.0.join("favicon.ico"), b"\x00\x00\x01\x00icon-bytes")
            .expect("write favicon");

        let app = Router::new().fallback_service(spa_fallback(
            &dir.0,
            Some("<html>the dashboard</html>".to_string()),
        ));
        let addr = serve(app).await;

        let response = reqwest::get(format!("http://{addr}/favicon.ico"))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            !content_type.contains("text/html"),
            "served as the SPA, not as an icon: {content_type}"
        );
        assert!(
            response
                .bytes()
                .await
                .unwrap()
                .starts_with(b"\x00\x00\x01\x00"),
            "the icon's own bytes must come back"
        );
    }

    /// The other half: a path with no file behind it still reaches the SPA, so
    /// adding assets did not break deep-link refresh.
    #[tokio::test]
    async fn a_deep_link_still_falls_through_to_the_dashboard() {
        let dir = ScratchUiDir::with_index("deeplink", "<html>the dashboard</html>");
        let app = Router::new().fallback_service(spa_fallback(
            &dir.0,
            Some("<html>the dashboard</html>".to_string()),
        ));
        let addr = serve(app).await;

        let body = reqwest::get(format!("http://{addr}/models"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("the dashboard"), "{body}");
    }

    /// #928: an unmatched path under an API prefix must answer as the API, not
    /// as the dashboard. One regex governs both halves, so this and the deep
    /// link test above are a pair — fixing either by breaking the other is the
    /// failure mode worth pinning.
    #[tokio::test]
    async fn an_unknown_api_path_is_a_json_404_not_the_dashboard() {
        let dir = ScratchUiDir::with_index("apifourohfour", "<html>the dashboard</html>");
        let app = Router::new().fallback_service(spa_fallback(
            &dir.0,
            Some("<html>the dashboard</html>".to_string()),
        ));
        let addr = serve(app).await;

        // `/api/v1/keys` is the real typo from the dogfooding pass: virtual keys
        // live under `/api/v1/projects/{id}/virtual-keys`
        for path in ["/api/v1/keys", "/api/v1", "/internal/nope", "/internal"] {
            let response = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
            assert_eq!(response.status(), 404, "{path} did not 404");
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert!(
                content_type.contains("application/json"),
                "{path} answered {content_type}"
            );
            let body: serde_json::Value = response.json().await.unwrap();
            assert!(
                body["error"]["message"].is_string(),
                "{path} body is not the api error shape: {body}"
            );
        }
    }

    /// A path that merely starts with the same letters is a dashboard route,
    /// not an API one — `/apikeys` is a plausible screen name.
    #[tokio::test]
    async fn a_route_sharing_an_api_prefixs_letters_still_reaches_the_dashboard() {
        let dir = ScratchUiDir::with_index("apiprefix", "<html>the dashboard</html>");
        let app = Router::new().fallback_service(spa_fallback(
            &dir.0,
            Some("<html>the dashboard</html>".to_string()),
        ));
        let addr = serve(app).await;

        for path in ["/apikeys", "/internals"] {
            let body = reqwest::get(format!("http://{addr}{path}"))
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(body.contains("the dashboard"), "{path}: {body}");
        }
    }

    #[test]
    fn api_paths_are_recognised_by_prefix_not_by_substring() {
        assert!(is_api_path("/api/v1/keys"));
        assert!(is_api_path("/api"));
        assert!(is_api_path("/internal/snapshot"));
        assert!(!is_api_path("/apikeys"));
        assert!(!is_api_path("/providers"));
        assert!(!is_api_path("/"));
    }

    #[test]
    fn load_index_is_none_when_the_dashboard_was_never_built() {
        let missing = std::env::temp_dir().join(format!("rolter-no-ui-{}", std::process::id()));
        // a headless control plane is a supported deployment, so this must be
        // an absent dashboard rather than a startup failure
        assert!(load_index(&missing, &ui_config::UiRuntimeConfig::default()).is_none());
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn provider_kind_str_covers_every_kind_and_matches_the_decoder() {
        use rolter_core::ProviderKind::*;
        // each variant maps to the exact string rolter_store decodes back
        for kind in [
            Openai,
            Anthropic,
            OpenaiCompatible,
            Ollama,
            OllamaCloud,
            LlamaCpp,
            Openrouter,
            Tei,
            AzureOpenai,
            Bedrock,
            Vertex,
            Gemini,
            GeminiNative,
            Mistral,
            Groq,
            Xai,
            MetaLlamaApi,
            Cohere,
            Perplexity,
            Together,
            Fireworks,
            Databricks,
            AlephAlpha,
            Nebius,
            Ovhcloud,
            Scaleway,
            Deepseek,
            Qwen,
            Zhipu,
            Kimi,
            Ernie,
            Doubao,
            Hunyuan,
            Yi,
            Minimax,
            Baichuan,
            Gigachat,
            YandexGpt,
            CloudRu,
            MtsAi,
            Naver,
            Upstage,
            Rinna,
            Rakuten,
            Sarvam,
            Krutrim,
            Falcon,
        ] {
            let s = provider_kind_str(&kind);
            assert!(!s.is_empty());
        }
        assert_eq!(provider_kind_str(&OpenaiCompatible), "openai_compatible");
        assert_eq!(provider_kind_str(&AzureOpenai), "azure_openai");
    }

    fn state_with_token(token: Option<&str>) -> ControlState {
        state_with_tokens(token, None)
    }

    fn state_with_tokens(admin: Option<&str>, internal: Option<&str>) -> ControlState {
        ControlState {
            store: Arc::new(InMemoryConfigStore::new(GatewayConfig::default())),
            config_owned: Arc::new(ConfigOwned::default()),
            egress: Arc::new(Default::default()),
            currency: Arc::new(Default::default()),
            redis: None,
            clickhouse: None,
            admin_token: admin.map(|t| Arc::new(t.to_string())),
            internal_token: internal.map(|t| Arc::new(t.to_string())),
            http: reqwest::Client::new(),
            gateway_url: Arc::new("http://localhost:4000".to_string()),
            public_url: Arc::new(DEFAULT_PUBLIC_URL.to_string()),
            cors: Arc::default(),
            metrics: Default::default(),
            login_throttle: Default::default(),
            trust_forwarded_for: false,
            update_check: update_check::UpdateChecker::disabled(),
            #[cfg(feature = "postgres")]
            pool: None,
        }
    }

    /// The public base URL is resolved once, from a value the caller supplies,
    /// so the normalization is a pure function with no environment behind it
    /// (#1418).
    #[test]
    fn the_public_url_is_normalized_once_from_the_value_it_is_given() {
        // a trailing slash would double up against the paths appended to it
        assert_eq!(
            normalize_public_url(Some("https://rolter.example.com/".to_string())),
            "https://rolter.example.com"
        );
        assert_eq!(
            normalize_public_url(Some("https://rolter.example.com".to_string())),
            "https://rolter.example.com"
        );
        // blank is a misconfiguration, not an origin: it must not build
        // `"/auth/sso/x/callback"` and call that a redirect uri
        assert_eq!(
            normalize_public_url(Some("   ".to_string())),
            DEFAULT_PUBLIC_URL
        );
        assert_eq!(normalize_public_url(None), DEFAULT_PUBLIC_URL);
    }

    /// #947: the dashboard has to state, per kind, whether `/v1` belongs in
    /// `api_base`. Serving it beats duplicating the ~38-kind list in TypeScript.
    #[tokio::test]
    async fn the_provider_kinds_endpoint_states_the_v1_rule_per_kind() {
        let addr = serve(build_app_with(state_with_token(None), false)).await;
        let body: serde_json::Value = reqwest::get(format!("http://{addr}/api/v1/provider-kinds"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let kinds = body.as_array().expect("a list of kinds");
        assert_eq!(kinds.len(), rolter_core::ProviderKind::ALL.len());

        let rule = |name: &str| -> bool {
            kinds
                .iter()
                .find(|k| k["kind"] == name)
                .unwrap_or_else(|| panic!("{name} missing from {body}"))["base_includes_v1"]
                .as_bool()
                .expect("a boolean")
        };
        // the default kind is the one the old static hint got wrong
        assert!(!rule("openai"), "openai must not carry /v1 in the base");
        assert!(!rule("openai_compatible"));
        assert!(!rule("ollama"));
        // and the kinds that genuinely need it still say so
        assert!(rule("mistral"));
        assert!(rule("openrouter"));
        assert!(rule("azure_openai"));
    }

    fn state_with_currency(currency: rolter_core::CurrencyConfig) -> ControlState {
        ControlState {
            currency: Arc::new(currency),
            ..state_with_token(None)
        }
    }

    async fn currency_settings(state: ControlState) -> serde_json::Value {
        let addr = serve(build_app_with(state, false)).await;
        reqwest::get(format!("http://{addr}/api/v1/currency"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_currency_endpoint_defaults_to_the_base_alone() {
        let body = currency_settings(state_with_token(None)).await;
        assert_eq!(body["base"], "USD");
        assert_eq!(body["codes"], serde_json::json!(["USD"]));
        // the base needs no row, but it must still be convertible
        assert_eq!(body["rates"]["USD"], 1.0);
    }

    /// #965: the dashboard hardcoded seven ISO-4217 codes, so a configured RUB
    /// was unselectable. The list has to come from the rate table.
    #[tokio::test]
    async fn a_configured_currency_is_offered_without_a_code_change() {
        let body = currency_settings(state_with_currency(rolter_core::CurrencyConfig {
            base: "USD".to_string(),
            rates: std::collections::HashMap::from([("RUB".to_string(), 0.011)]),
        }))
        .await;
        assert_eq!(body["codes"], serde_json::json!(["USD", "RUB"]));
        assert_eq!(body["rates"]["RUB"], 0.011);
    }

    /// The chooser must not offer what `require_known_currency` would reject:
    /// GBP is in the hardcoded list the dashboard used to ship, but this
    /// deployment has no rate for it.
    #[tokio::test]
    async fn a_currency_without_a_rate_is_not_offered() {
        let body = currency_settings(state_with_currency(rolter_core::CurrencyConfig {
            base: "EUR".to_string(),
            rates: std::collections::HashMap::from([("RUB".to_string(), 0.0097)]),
        }))
        .await;
        assert_eq!(body["codes"], serde_json::json!(["EUR", "RUB"]));
        assert!(body["rates"].get("GBP").is_none(), "{body}");
    }

    /// A non-USD base is the case the hardcoded list got most wrong: it always
    /// led with USD, whatever the deployment settled in.
    #[tokio::test]
    async fn the_base_leads_even_when_it_is_not_usd() {
        let body = currency_settings(state_with_currency(rolter_core::CurrencyConfig {
            base: "rub".to_string(),
            rates: std::collections::HashMap::from([
                ("USD".to_string(), 91.0),
                ("EUR".to_string(), 99.0),
            ]),
        }))
        .await;
        // normalized, base first, remainder alphabetical
        assert_eq!(body["codes"], serde_json::json!(["RUB", "EUR", "USD"]));
        assert_eq!(body["base"], "RUB");
    }

    /// the default deployment shape: /internal/* on the same listener as the
    /// public api, gated by a token
    fn build_app_with_internal(state: ControlState) -> Router {
        build_app_with(state, true)
    }

    async fn serve(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    /// #845 replaced `Json(..).into_response()` with an explicit serialize so
    /// the payload size is measurable. The wire contract must be byte-identical
    /// to what gateways already parse — a snapshot that changed shape would
    /// break config propagation fleet-wide.
    #[tokio::test]
    async fn the_snapshot_response_is_unchanged_by_the_explicit_encode() {
        let addr = serve(build_app_with_internal(state_with_token(None))).await;
        let client = reqwest::Client::new();

        let response = client
            .get(format!("http://{addr}/internal/snapshot"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        let body: Value = response.json().await.unwrap();
        assert!(body["version"].is_number(), "no version in {body}");
        assert!(body["config"].is_object(), "no config in {body}");

        // and a caller already at the current version still gets a bodyless 304
        let version = body["version"].as_i64().unwrap();
        let not_modified = client
            .get(format!("http://{addr}/internal/snapshot?version={version}"))
            .send()
            .await
            .unwrap();
        assert_eq!(not_modified.status(), 304);
        assert!(not_modified.bytes().await.unwrap().is_empty());
    }

    /// #926: one malformed provider used to withhold the entire fleet's
    /// configuration — every route, every other provider, every key — because
    /// `/internal/snapshot` validated all-or-nothing and 500'd. A provider
    /// nobody routes to must not be able to freeze a fleet.
    #[tokio::test]
    async fn one_invalid_provider_does_not_reduce_the_snapshot_to_an_error() {
        let config: GatewayConfig = serde_json::from_value(json!({
            "providers": [
                {
                    "name": "openai",
                    "kind": "openai",
                    "api_base": "https://api.openai.com/v1",
                },
                {
                    "name": "openrouter-edge",
                    "kind": "openrouter",
                    "api_base": "https://openrouter.example.com/v1",
                    "api_key_env": "OPENROUTER_API_KEY",
                },
            ],
            "routes": [
                {"model": "gpt-4o", "targets": [{"provider": "openai"}]},
            ],
        }))
        .expect("fixture config parses");
        let mut state = state_with_token(None);
        state.store = Arc::new(InMemoryConfigStore::new(config));

        let addr = serve(build_app_with_internal(state)).await;
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/internal/snapshot"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200, "the fleet must still propagate");
        let body: Value = response.json().await.unwrap();

        // the valid provider and route are served
        let providers = body["config"]["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 1, "{body}");
        assert_eq!(providers[0]["name"], "openai");
        assert_eq!(body["config"]["routes"].as_array().unwrap().len(), 1);

        // and the dropped entry rides along with its reason, so the gateway can
        // log it and the dashboard can explain it
        let problems = body["problems"].as_array().expect("problems in {body}");
        assert_eq!(problems.len(), 1, "{body}");
        let problem = problems[0].as_str().unwrap();
        assert!(problem.contains("openrouter-edge"), "{problem}");
        assert!(
            problem.contains("https://openrouter.ai/api/v1"),
            "{problem}"
        );
    }

    /// The dashboard needs the same answer the snapshot reached, or an operator
    /// has no way to see that a provider stopped being served (#926).
    #[tokio::test]
    async fn the_dashboard_can_ask_what_is_not_being_served() {
        let config: GatewayConfig = serde_json::from_value(json!({
            "providers": [
                {
                    "name": "openai",
                    "kind": "openai",
                    "api_base": "https://api.openai.com/v1",
                },
                {
                    "name": "openrouter-edge",
                    "kind": "openrouter",
                    "api_base": "https://openrouter.example.com/v1",
                    "api_key_env": "OPENROUTER_API_KEY",
                },
            ],
            "routes": [
                {"model": "gpt-4o", "targets": [{"provider": "openai"}]},
            ],
        }))
        .expect("fixture config parses");
        let mut state = state_with_token(None);
        state.store = Arc::new(InMemoryConfigStore::new(config));

        let addr = serve(build_app_with_internal(state)).await;
        let body: Value = reqwest::Client::new()
            .get(format!("http://{addr}/api/v1/config/problems"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let problems = body["problems"].as_array().unwrap();
        assert_eq!(problems.len(), 1, "{body}");
        assert!(
            problems[0].as_str().unwrap().contains("openrouter-edge"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_healthy_config_reports_no_problems_to_the_dashboard() {
        let addr = serve(build_app_with_internal(state_with_token(None))).await;
        let body: Value = reqwest::Client::new()
            .get(format!("http://{addr}/api/v1/config/problems"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["problems"].as_array().unwrap().len(), 0, "{body}");
    }

    /// The common case must not grow a field: `problems` is on the wire of
    /// every poll of every gateway in the fleet.
    #[tokio::test]
    async fn a_healthy_snapshot_carries_no_problems_key() {
        let addr = serve(build_app_with_internal(state_with_token(None))).await;
        let body: Value = reqwest::Client::new()
            .get(format!("http://{addr}/internal/snapshot"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(body.get("problems").is_none(), "{body}");
    }

    #[tokio::test]
    async fn snapshot_requires_admin_token_when_configured() {
        let addr = serve(build_app_with_internal(state_with_token(Some("sekrit")))).await;
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        let unauthenticated = client.get(&url).send().await.unwrap();
        assert_eq!(unauthenticated.status(), 401);

        let wrong = client.get(&url).bearer_auth("nope").send().await.unwrap();
        assert_eq!(wrong.status(), 401);

        let ok = client.get(&url).bearer_auth("sekrit").send().await.unwrap();
        assert_eq!(ok.status(), 200);

        // the rest of the api stays open (dashboard reads, health)
        let ping = client
            .get(format!("http://{addr}/api/v1/ping"))
            .send()
            .await
            .unwrap();
        assert_eq!(ping.status(), 200);
    }

    #[tokio::test]
    async fn a_distinct_internal_token_locks_the_admin_token_out_of_the_snapshot() {
        // the snapshot carries decrypted provider credentials (#636): once the
        // operator separates the two trust boundaries, an admin credential must
        // not open that channel any more
        let addr = serve(build_app_with_internal(state_with_tokens(
            Some("operator"),
            Some("machine"),
        )))
        .await;
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/internal/snapshot");

        let admin = client
            .get(&url)
            .bearer_auth("operator")
            .send()
            .await
            .unwrap();
        assert_eq!(
            admin.status(),
            401,
            "admin token still unlocked the snapshot"
        );

        let machine = client
            .get(&url)
            .bearer_auth("machine")
            .send()
            .await
            .unwrap();
        assert_eq!(machine.status(), 200);
    }

    #[tokio::test]
    async fn a_separate_internal_listener_removes_the_snapshot_from_the_public_api() {
        // not merely gated on the public surface — absent from it
        let state = state_with_tokens(Some("operator"), Some("machine"));
        let public = serve(build_app_with(state.clone(), false)).await;
        let internal = serve(internal_routes(&state).with_state(state)).await;
        let client = reqwest::Client::new();

        let on_public = client
            .get(format!("http://{public}/internal/snapshot"))
            .bearer_auth("machine")
            .send()
            .await
            .unwrap();
        assert_eq!(on_public.status(), 404);

        // the public api is otherwise intact
        let ping = client
            .get(format!("http://{public}/api/v1/ping"))
            .send()
            .await
            .unwrap();
        assert_eq!(ping.status(), 200);

        let on_internal = client
            .get(format!("http://{internal}/internal/snapshot"))
            .bearer_auth("machine")
            .send()
            .await
            .unwrap();
        assert_eq!(on_internal.status(), 200);
    }

    /// #929: the gateway learned where to write usage rows from its own
    /// bootstrap TOML while the dashboard read them from the control plane's
    /// `CLICKHOUSE_URL`, and the snapshot carried `"clickhouse_url": null` —
    /// so a fleet could log nowhere while the screens queried a table nobody
    /// filled.
    #[tokio::test]
    async fn the_snapshot_publishes_where_the_fleet_should_log() {
        let mut state = state_with_token(None);
        state.clickhouse = Some(analytics::ClickHouseClient::new("http://clickhouse:8123/"));
        let addr = serve(build_app_with_internal(state)).await;
        let body: Value = reqwest::Client::new()
            .get(format!("http://{addr}/internal/snapshot"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        // the trailing slash is normalised by the client, so what the fleet
        // inherits is the same string the control plane itself queries
        assert_eq!(
            body["config"]["logging"]["clickhouse_url"],
            "http://clickhouse:8123"
        );
    }

    #[tokio::test]
    async fn a_control_plane_without_analytics_publishes_no_destination() {
        // absent, not empty-string: a gateway must be able to tell "nobody told
        // me" from "told me to log to nowhere"
        let addr = serve(build_app_with_internal(state_with_token(None))).await;
        let body: Value = reqwest::Client::new()
            .get(format!("http://{addr}/internal/snapshot"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            body["config"]["logging"]["clickhouse_url"].is_null(),
            "{body}"
        );
    }

    #[tokio::test]
    async fn snapshot_open_when_no_token_configured() {
        let addr = serve(build_app_with_internal(state_with_token(None))).await;
        let resp = reqwest::Client::new()
            .get(format!("http://{addr}/internal/snapshot"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn gw_proxies_http_to_the_gateway() {
        // stand-in for the gateway data plane
        let upstream = Router::new()
            .route("/v1/ping", get(|| async { "pong-from-gw" }))
            .route(
                "/v1/echo",
                axum::routing::post(|body: String| async move { body }),
            );
        let up_addr = serve(upstream).await;

        let state = ControlState {
            gateway_url: Arc::new(format!("http://{up_addr}")),
            cors: Arc::default(),
            metrics: Default::default(),
            ..state_with_token(None)
        };
        let addr = serve(build_app_with(state, true)).await;
        let client = reqwest::Client::new();

        // GET forwards path + response body
        let got = client
            .get(format!("http://{addr}/gw/v1/ping"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(got, "pong-from-gw");

        // POST forwards the request body
        let echoed = client
            .post(format!("http://{addr}/gw/v1/echo"))
            .body("hello gateway")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(echoed, "hello gateway");
    }

    #[tokio::test]
    async fn config_endpoint_redacts_provider_keys() {
        let mut config = GatewayConfig::default();
        config.providers.push(rolter_core::ProviderConfig {
            name: "openai".to_string(),
            kind: rolter_core::ProviderKind::Openai,
            api_base: "https://api.openai.com".to_string(),
            api_key: Some("sk-super-secret".to_string()),
            api_keys: vec![rolter_core::ApiKeyConfig {
                key: Some("sk-also-secret".to_string()),
                env: None,
                weight: 1,
            }],
            ..Default::default()
        });
        let state = ControlState {
            store: Arc::new(InMemoryConfigStore::new(config)),
            ..state_with_token(None)
        };
        let addr = serve(build_app_with(state, true)).await;

        let body = reqwest::Client::new()
            .get(format!("http://{addr}/api/v1/config"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            !body.contains("sk-super-secret") && !body.contains("sk-also-secret"),
            "config endpoint must not leak provider keys: {body}"
        );
    }

    /// The #813 regression: an operator saves an origin and it must actually
    /// take effect, end to end through the real router.
    #[tokio::test]
    async fn a_configured_origin_gets_cors_headers_and_others_do_not() {
        let state = state_with_token(None);
        state.cors.store(Arc::new(cors::CorsPolicy::new(
            &["https://dash.example.com".to_string()],
            &["x-tenant".to_string()],
        )));
        let addr = serve(build_app_with(state, false)).await;
        let client = reqwest::Client::new();
        let url = format!("http://{addr}/api/v1/ping");

        let allowed = client
            .get(&url)
            .header("origin", "https://dash.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(
            allowed
                .headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("https://dash.example.com")
        );
        // Vary must be set so a shared cache cannot serve one origin's answer
        // to another
        assert!(allowed
            .headers()
            .get("vary")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().contains("origin")));

        let denied = client
            .get(&url)
            .header("origin", "https://evil.example.com")
            .send()
            .await
            .unwrap();
        assert!(denied
            .headers()
            .get("access-control-allow-origin")
            .is_none());
        // still served, just not readable cross-origin — the browser enforces it
        assert_eq!(denied.status(), 200);
    }

    #[tokio::test]
    async fn a_preflight_is_answered_with_the_configured_and_trace_headers() {
        let state = state_with_token(None);
        state.cors.store(Arc::new(cors::CorsPolicy::new(
            &["https://dash.example.com".to_string()],
            &["x-tenant".to_string()],
        )));
        let addr = serve(build_app_with(state, false)).await;

        let response = reqwest::Client::new()
            .request(
                reqwest::Method::OPTIONS,
                format!("http://{addr}/api/v1/ping"),
            )
            .header("origin", "https://dash.example.com")
            .header("access-control-request-method", "GET")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 204);
        let allow = response
            .headers()
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        // #805: without these the dashboard's spans cannot join the gateway's
        assert!(allow.contains("traceparent"), "got: {allow}");
        assert!(allow.contains("tracestate"), "got: {allow}");
        assert!(allow.contains("x-tenant"), "got: {allow}");
    }

    #[tokio::test]
    async fn the_default_deployment_is_untouched_by_cors() {
        // no origins configured is the same-origin deployment everyone runs; it
        // must behave exactly as it did before the middleware existed
        let addr = serve(build_app_with(state_with_token(None), false)).await;
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/api/v1/ping"))
            .header("origin", "https://dash.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
        assert!(response.headers().get("vary").is_none());
    }
}
