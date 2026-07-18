//! rolter-control: the control plane.
//!
//! Hosts the management API used by the dashboard and serves the built SPA as
//! static assets. The MVP exposes health, a config read endpoint backed by an
//! in-memory store, and the role catalog; CRUD, RBAC enforcement, Postgres
//! persistence and Redis change publication are added in later phases.
//!
//! The binary is a thin wrapper over [`run`]; the unified `rolter` launcher
//! reuses the same entrypoint as its `control` subcommand.

mod analytics;
#[cfg(feature = "postgres")]
mod auth;
#[cfg(feature = "postgres")]
mod crud;
mod health;
#[cfg(feature = "postgres")]
mod me;
mod proxy;
#[cfg(feature = "postgres")]
mod rbac;
#[cfg(feature = "postgres")]
pub mod seed;

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
pub struct Args {
    #[arg(long, env = "ROLTER_CONTROL_HOST", default_value = "0.0.0.0")]
    pub host: String,
    #[arg(long, env = "ROLTER_CONTROL_PORT", default_value_t = 4001)]
    pub port: u16,
    /// directory holding the built UI (index.html + assets)
    #[arg(long, env = "ROLTER_UI_DIR", default_value = "ui/dist")]
    pub ui_dir: PathBuf,
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
    /// redis connection url; when set, config-version bumps are published on
    /// the `rolter.config` channel so gateways refetch immediately instead of
    /// waiting for their poll interval
    #[arg(long, env = "ROLTER_REDIS_URL")]
    pub redis_url: Option<String>,
    /// clickhouse http url; when set, the dashboard usage/cost analytics
    /// endpoints (`/api/v1/analytics/*`) query the `request_logs` table
    #[arg(long, env = "CLICKHOUSE_URL")]
    pub clickhouse_url: Option<String>,
    /// bearer token required on the CRUD API and `/internal/snapshot`; when
    /// unset those endpoints are open (a warning is logged at startup)
    #[arg(long, env = "ROLTER_ADMIN_TOKEN")]
    pub admin_token: Option<String>,
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
    /// set when `--clickhouse-url` is configured; backs the usage/cost
    /// analytics endpoints
    clickhouse: Option<analytics::ClickHouseClient>,
    /// when set, the CRUD API and `/internal/snapshot` require
    /// `Authorization: Bearer <token>`
    admin_token: Option<Arc<String>>,
    /// shared client for the `/gw/*` reverse proxy to the gateway data plane
    http: reqwest::Client,
    /// base URL of the rolter-gateway the `/gw/*` proxy forwards to
    gateway_url: Arc<String>,
    /// set when `--database-url` is configured; backs the CRUD API, which
    /// needs direct repository access beyond what `ConfigStore` exposes
    #[cfg(feature = "postgres")]
    pool: Option<sqlx::PgPool>,
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
    if admin_token.is_none() {
        tracing::warn!(
            "ROLTER_ADMIN_TOKEN is unset: the management API and /internal/snapshot are \
             unauthenticated; set it before exposing the control plane beyond localhost"
        );
    }

    #[allow(unused_variables)]
    let (store, pool) = build_store(&args, bootstrap).await?;
    let http = reqwest::Client::new();
    let gateway_url = Arc::new(args.gateway_url.trim_end_matches('/').to_string());
    tracing::info!(gateway_url = %gateway_url, "proxying /gw/* to the gateway");
    #[cfg(feature = "postgres")]
    let state = ControlState {
        store,
        config_owned,
        redis,
        clickhouse,
        admin_token,
        http,
        gateway_url,
        pool: pool.clone(),
    };
    #[cfg(not(feature = "postgres"))]
    let state = ControlState {
        store,
        config_owned,
        redis,
        clickhouse,
        admin_token,
        http,
        gateway_url,
    };

    let app = build_app(state)
        // anything not matched by the api falls through to the built SPA
        .fallback_service(ServeDir::new(&args.ui_dir));

    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;
    tracing::info!(%addr, ui_dir = %args.ui_dir.display(), "rolter-control listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// Assemble the control-plane API router (no SPA fallback) with `state` applied.
/// The CRUD routes are only mounted when a postgres pool is present.
fn build_app(state: ControlState) -> Router {
    #[allow(unused_mut)]
    let mut api = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/api/v1/ping",
            get(|| async { Json(json!({"pong": true})) }),
        )
        .route("/api/v1/roles", get(list_roles))
        .route("/api/v1/config", get(get_config))
        .merge(analytics::router())
        .merge(health::router())
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
        api = api
            .merge(auth::router())
            .merge(crud::router())
            .merge(me::router());
    }

    // the snapshot endpoint carries decrypted provider credentials, so it stays
    // behind the shared admin token only (machine/superadmin access, no per-user
    // sessions) whenever one is configured, and open otherwise
    let snapshot = Router::new()
        .route("/internal/snapshot", get(get_snapshot))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_admin_token,
        ));

    api.merge(snapshot).with_state(state)
}

/// Reject requests lacking `Authorization: Bearer <admin token>` when a token
/// is configured; pass-through (open) when none is set.
async fn require_admin_token(
    State(state): State<ControlState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Some(expected) = state.admin_token.as_deref() else {
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
            Json(json!({"error": {"message": "missing or invalid admin token"}})),
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
    let store: Arc<dyn ConfigStore> =
        Arc::new(rolter_store::PostgresConfigStore::new(pool.clone()));
    let state = ControlState {
        store,
        config_owned: Arc::new(ConfigOwned::default()),
        redis: None,
        clickhouse: None,
        admin_token: admin_token.map(Arc::new),
        http: reqwest::Client::new(),
        gateway_url: Arc::new("http://localhost:4000".to_string()),
        pool: Some(pool),
    };
    Ok(build_app(state))
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
        if let Some(config) = bootstrap.as_ref() {
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
    // this endpoint feeds the dashboard; upstream credentials stay between the
    // store and the gateway (via the token-guarded snapshot endpoint)
    for provider in &mut config.providers {
        provider.api_key = None;
        for key in &mut provider.api_keys {
            key.key = None;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_token(token: Option<&str>) -> ControlState {
        ControlState {
            store: Arc::new(InMemoryConfigStore::new(GatewayConfig::default())),
            config_owned: Arc::new(ConfigOwned::default()),
            redis: None,
            clickhouse: None,
            admin_token: token.map(|t| Arc::new(t.to_string())),
            http: reqwest::Client::new(),
            gateway_url: Arc::new("http://localhost:4000".to_string()),
            #[cfg(feature = "postgres")]
            pool: None,
        }
    }

    async fn serve(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn snapshot_requires_admin_token_when_configured() {
        let addr = serve(build_app(state_with_token(Some("sekrit")))).await;
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
    async fn snapshot_open_when_no_token_configured() {
        let addr = serve(build_app(state_with_token(None))).await;
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
            ..state_with_token(None)
        };
        let addr = serve(build_app(state)).await;
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
            slug: None,
            kind: rolter_core::ProviderKind::Openai,
            api_base: "https://api.openai.com".to_string(),
            api_key: Some("sk-super-secret".to_string()),
            api_key_env: None,
            egress_proxy: None,
            egress_proxies: Vec::new(),
            kv_events: None,
            lmcache: None,
            ca_bundles: None,
            api_keys: vec![rolter_core::ApiKeyConfig {
                key: Some("sk-also-secret".to_string()),
                env: None,
                weight: 1,
            }],
            also_track_via_llm_call: false,
            llm_probe_model: None,
            status_page_url: None,
            role_profile: None,
            model_role_profiles: Default::default(),
        });
        let state = ControlState {
            store: Arc::new(InMemoryConfigStore::new(config)),
            ..state_with_token(None)
        };
        let addr = serve(build_app(state)).await;

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
}
