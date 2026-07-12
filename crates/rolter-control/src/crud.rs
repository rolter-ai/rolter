//! Control CRUD API: orgs, teams, projects, providers, routes/targets,
//! virtual keys, budgets, rate limits and the model pricing catalog.
//!
//! Thin Axum handlers over the `rolter_store::postgres::repo` repositories.
//! Only mounted when the control plane is started with `--database-url`
//! (see `main.rs`), since these routes need direct pool access beyond what
//! the [`rolter_store::ConfigStore`] trait exposes.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::{
    Budget, ModelPrice, Org, Project, Provider, RateLimit, Route, RouteTarget, Team, VirtualKey,
};
use rolter_store::postgres::repo::{
    BudgetRepo, ModelPriceRepo, OrgRepo, ProjectRepo, ProviderRepo, RateLimitRepo, RouteRepo,
    RouteTargetRepo, TeamRepo, VirtualKeyRepo,
};

use crate::ControlState;

pub fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/orgs", get(list_orgs).post(create_org))
        .route("/api/v1/orgs/{id}", delete(delete_org))
        .route(
            "/api/v1/orgs/{org_id}/teams",
            get(list_teams).post(create_team),
        )
        .route("/api/v1/teams/{id}", delete(delete_team))
        .route(
            "/api/v1/teams/{team_id}/projects",
            get(list_projects).post(create_project),
        )
        .route("/api/v1/projects/{id}", delete(delete_project))
        .route(
            "/api/v1/orgs/{org_id}/providers",
            get(list_providers).post(create_provider),
        )
        .route("/api/v1/providers/{id}", delete(delete_provider))
        .route(
            "/api/v1/projects/{project_id}/routes",
            get(list_routes).post(create_route),
        )
        .route(
            "/api/v1/routes/{id}",
            put(set_route_enabled).delete(delete_route),
        )
        .route("/api/v1/routes/{id}/params", put(set_route_params))
        .route(
            "/api/v1/routes/{route_id}/targets",
            get(list_route_targets).post(create_route_target),
        )
        .route("/api/v1/route-targets/{id}", delete(delete_route_target))
        .route(
            "/api/v1/projects/{project_id}/virtual-keys",
            get(list_virtual_keys).post(create_virtual_key),
        )
        .route(
            "/api/v1/virtual-keys/{id}",
            put(set_virtual_key_disabled).delete(delete_virtual_key),
        )
        .route(
            "/api/v1/virtual-keys/{id}/cache",
            put(set_virtual_key_cache),
        )
        .route("/api/v1/budgets", get(list_budgets).post(create_budget))
        .route("/api/v1/budgets/{id}", delete(delete_budget))
        .route(
            "/api/v1/rate-limits",
            get(list_rate_limits).post(create_rate_limit),
        )
        .route("/api/v1/rate-limits/{id}", delete(delete_rate_limit))
        .route(
            "/api/v1/model-prices",
            get(list_model_prices).put(upsert_model_price),
        )
        .route("/api/v1/model-prices/{model}", delete(delete_model_price))
        .route("/api/v1/models", get(list_models))
        .route("/api/v1/models/{model}", delete(delete_model))
}

fn pool(state: &ControlState) -> &PgPool {
    state
        .pool
        .as_ref()
        .expect("crud router is only mounted when a postgres pool is configured")
}

enum ApiError {
    Core(Error),
    /// mutation collides with a config-file-owned resource (409)
    Conflict(String),
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        Self::Core(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Core(err) => {
                let status = match &err {
                    Error::NotFound(_) => StatusCode::NOT_FOUND,
                    Error::Config(_) | Error::Unauthorized => StatusCode::BAD_REQUEST,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                (status, err.to_string())
            }
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
        };
        (
            status,
            Json(serde_json::json!({"error": {"message": message}})),
        )
            .into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

/// Reject a required field that's empty after trimming.
fn require_non_empty(value: &str, field: &str) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Err(ApiError::Core(Error::Config(format!(
            "{field} must not be empty"
        ))));
    }
    Ok(())
}

/// Announce a config change after a mutation that touches the effective
/// gateway config. The version bump itself is transactional with the write
/// (database triggers from migration 0003), so this only publishes the new
/// version on [`rolter_core::CONFIG_CHANNEL`] when redis is configured
/// (best-effort, off the request path) so gateways refetch immediately
/// instead of waiting for their poll interval.
async fn publish_config_change(state: &ControlState) -> ApiResult<()> {
    let Some(client) = state.redis.clone() else {
        return Ok(());
    };
    let version = rolter_store::postgres::current_version(pool(state)).await?;
    tokio::spawn(async move {
        let publish = async {
            let mut conn = client.get_multiplexed_async_connection().await?;
            redis::cmd("PUBLISH")
                .arg(rolter_core::CONFIG_CHANNEL)
                .arg(version)
                .query_async::<()>(&mut conn)
                .await
        };
        if let Err(err) = publish.await {
            tracing::warn!(error = %err, version, "failed to publish config bump to redis");
        }
    });
    Ok(())
}

/// Reject a mutation that collides with a bootstrap-config-owned resource.
fn require_not_config_owned(
    owned: &std::collections::HashSet<String>,
    name: &str,
    kind: &str,
) -> ApiResult<()> {
    if owned.contains(name) {
        return Err(ApiError::Conflict(format!(
            "{kind} '{name}' is managed by the bootstrap config and cannot be \
             modified at runtime; edit the config file and restart instead"
        )));
    }
    Ok(())
}

// --- orgs ---

async fn list_orgs(State(state): State<ControlState>) -> ApiResult<Json<Vec<Org>>> {
    Ok(Json(OrgRepo(pool(&state)).list().await?))
}

#[derive(Deserialize)]
struct CreateOrg {
    name: String,
    slug: String,
}

async fn create_org(
    State(state): State<ControlState>,
    Json(body): Json<CreateOrg>,
) -> ApiResult<Json<Org>> {
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    Ok(Json(
        OrgRepo(pool(&state)).create(&body.name, &body.slug).await?,
    ))
}

async fn delete_org(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    OrgRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- teams ---

async fn list_teams(
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Team>>> {
    Ok(Json(TeamRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
struct CreateTeam {
    name: String,
}

async fn create_team(
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateTeam>,
) -> ApiResult<Json<Team>> {
    require_non_empty(&body.name, "name")?;
    Ok(Json(
        TeamRepo(pool(&state)).create(org_id, &body.name).await?,
    ))
}

async fn delete_team(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    TeamRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- projects ---

async fn list_projects(
    State(state): State<ControlState>,
    Path(team_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Project>>> {
    Ok(Json(ProjectRepo(pool(&state)).list(team_id).await?))
}

#[derive(Deserialize)]
struct CreateProject {
    name: String,
}

async fn create_project(
    State(state): State<ControlState>,
    Path(team_id): Path<Uuid>,
    Json(body): Json<CreateProject>,
) -> ApiResult<Json<Project>> {
    require_non_empty(&body.name, "name")?;
    Ok(Json(
        ProjectRepo(pool(&state))
            .create(team_id, &body.name)
            .await?,
    ))
}

async fn delete_project(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    ProjectRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- providers ---

async fn list_providers(
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Provider>>> {
    Ok(Json(ProviderRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
struct CreateProvider {
    name: String,
    kind: String,
    api_base: String,
    api_key_env: Option<String>,
    egress_proxy: Option<String>,
}

const PROVIDER_KINDS: [&str; 5] = [
    "openai",
    "anthropic",
    "openai_compatible",
    "ollama",
    "llama_cpp",
];

async fn create_provider(
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateProvider>,
) -> ApiResult<Json<Provider>> {
    require_non_empty(&body.name, "name")?;
    require_not_config_owned(&state.config_owned.providers, &body.name, "provider")?;
    require_non_empty(&body.api_base, "api_base")?;
    if !PROVIDER_KINDS.contains(&body.kind.as_str()) {
        return Err(ApiError::Core(Error::Config(format!(
            "kind must be one of {PROVIDER_KINDS:?}"
        ))));
    }
    let row = ProviderRepo(pool(&state))
        .create(
            org_id,
            &body.name,
            &body.kind,
            &body.api_base,
            body.api_key_env.as_deref(),
            body.egress_proxy.as_deref(),
        )
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

async fn delete_provider(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    ProviderRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- routes + targets ---

async fn list_routes(
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Route>>> {
    Ok(Json(RouteRepo(pool(&state)).list(project_id).await?))
}

#[derive(Deserialize)]
struct CreateRoute {
    model: String,
    #[serde(default = "default_strategy")]
    strategy: String,
}

fn default_strategy() -> String {
    "round_robin".to_string()
}

const STRATEGIES: [&str; 7] = [
    "round_robin",
    "random",
    "power_of_two",
    "consistent_hash",
    "cache_aware",
    "weighted",
    "pipeline",
];

async fn create_route(
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateRoute>,
) -> ApiResult<Json<Route>> {
    require_non_empty(&body.model, "model")?;
    require_not_config_owned(&state.config_owned.models, &body.model, "model")?;
    if !STRATEGIES.contains(&body.strategy.as_str()) {
        return Err(ApiError::Core(Error::Config(format!(
            "strategy must be one of {STRATEGIES:?}"
        ))));
    }
    let row = RouteRepo(pool(&state))
        .create(project_id, &body.model, &body.strategy)
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct SetRouteEnabled {
    enabled: bool,
}

async fn set_route_enabled(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRouteEnabled>,
) -> ApiResult<Json<Route>> {
    let row = RouteRepo(pool(&state))
        .set_enabled(id, body.enabled)
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct SetRouteParams {
    /// admin default inference params (json object, e.g. {"temperature": 0})
    #[serde(default)]
    params: serde_json::Value,
    /// override policy {mode, allow, deny}
    #[serde(default)]
    param_policy: serde_json::Value,
}

async fn set_route_params(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRouteParams>,
) -> ApiResult<Json<Route>> {
    // both must be json objects (or null → treated as empty) so the gateway can
    // deserialize them into the param map / policy
    let params = normalize_json_object(body.params, "params")?;
    let param_policy = normalize_json_object(body.param_policy, "param_policy")?;
    let row = RouteRepo(pool(&state))
        .set_params(id, &params, &param_policy)
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

/// Coerce a json value to an object: `null` becomes `{}`, an object passes
/// through, anything else is a client error.
fn normalize_json_object(value: serde_json::Value, field: &str) -> ApiResult<serde_json::Value> {
    match value {
        serde_json::Value::Null => Ok(serde_json::json!({})),
        v @ serde_json::Value::Object(_) => Ok(v),
        _ => Err(ApiError::Core(Error::Config(format!(
            "{field} must be a json object"
        )))),
    }
}

async fn delete_route(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    RouteRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_route_targets(
    State(state): State<ControlState>,
    Path(route_id): Path<Uuid>,
) -> ApiResult<Json<Vec<RouteTarget>>> {
    Ok(Json(RouteTargetRepo(pool(&state)).list(route_id).await?))
}

#[derive(Deserialize)]
struct CreateRouteTarget {
    provider_id: Uuid,
    upstream_model: Option<String>,
    #[serde(default = "default_weight")]
    weight: i32,
}

fn default_weight() -> i32 {
    1
}

async fn create_route_target(
    State(state): State<ControlState>,
    Path(route_id): Path<Uuid>,
    Json(body): Json<CreateRouteTarget>,
) -> ApiResult<Json<RouteTarget>> {
    if body.weight <= 0 {
        return Err(ApiError::Core(Error::Config("weight must be > 0".into())));
    }
    let row = RouteTargetRepo(pool(&state))
        .create(
            route_id,
            body.provider_id,
            body.upstream_model.as_deref(),
            body.weight,
        )
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

async fn delete_route_target(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    RouteTargetRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- virtual keys ---

async fn list_virtual_keys(
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<VirtualKey>>> {
    Ok(Json(VirtualKeyRepo(pool(&state)).list(project_id).await?))
}

#[derive(Deserialize)]
struct CreateVirtualKey {
    name: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    /// per-key response-cache override; omit/null to inherit the route decision,
    /// false to bypass, true to cache even on a route that didn't opt in
    #[serde(default)]
    cache: Option<bool>,
}

#[derive(Serialize)]
struct CreatedVirtualKey {
    #[serde(flatten)]
    row: VirtualKey,
    /// the plaintext key; shown once, never persisted or returned again
    key: String,
}

/// Deployment-wide pepper shared with the gateway (`ROLTER_KEY_PEPPER`). Keys
/// are stored as `rolter_auth::hash_key(pepper, key)` so the gateway can match
/// presented keys by the same peppered digest.
fn key_pepper() -> String {
    std::env::var("ROLTER_KEY_PEPPER").unwrap_or_default()
}

fn generate_virtual_key(pepper: &str) -> (String, String, String) {
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    let key = format!("sk-rolter-{}", hex_encode(&bytes));
    let hash = rolter_auth::hash_key(pepper, &key);
    let prefix = key.chars().take(12).collect::<String>();
    (key, hash, prefix)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn create_virtual_key(
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateVirtualKey>,
) -> ApiResult<Json<CreatedVirtualKey>> {
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            body.name.as_deref(),
            &body.models,
            body.cache,
        )
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(CreatedVirtualKey { row, key }))
}

#[derive(Deserialize)]
struct SetVirtualKeyDisabled {
    disabled: bool,
}

async fn set_virtual_key_disabled(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetVirtualKeyDisabled>,
) -> ApiResult<Json<VirtualKey>> {
    let row = VirtualKeyRepo(pool(&state))
        .set_disabled(id, body.disabled)
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct SetVirtualKeyCache {
    /// null clears the override (inherit the route); false/true force it
    #[serde(default)]
    cache: Option<bool>,
}

async fn set_virtual_key_cache(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetVirtualKeyCache>,
) -> ApiResult<Json<VirtualKey>> {
    let row = VirtualKeyRepo(pool(&state))
        .set_cache(id, body.cache)
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

async fn delete_virtual_key(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    VirtualKeyRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- budgets ---

#[derive(Deserialize)]
struct ScopeQuery {
    scope_type: String,
    scope_id: Uuid,
}

const SCOPE_TYPES: [&str; 4] = ["org", "team", "project", "virtual_key"];

fn validate_scope(scope_type: &str) -> ApiResult<()> {
    if !SCOPE_TYPES.contains(&scope_type) {
        return Err(ApiError::Core(Error::Config(format!(
            "scope_type must be one of {SCOPE_TYPES:?}"
        ))));
    }
    Ok(())
}

async fn list_budgets(
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<Budget>>> {
    validate_scope(&scope.scope_type)?;
    Ok(Json(
        BudgetRepo(pool(&state))
            .list_for_scope(&scope.scope_type, scope.scope_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct CreateBudget {
    scope_type: String,
    scope_id: Uuid,
    limit_usd: String,
    #[serde(default = "default_period")]
    period: String,
}

fn default_period() -> String {
    "30d".to_string()
}

async fn create_budget(
    State(state): State<ControlState>,
    Json(body): Json<CreateBudget>,
) -> ApiResult<Json<Budget>> {
    validate_scope(&body.scope_type)?;
    if body.limit_usd.trim().parse::<f64>().is_err() {
        return Err(ApiError::Core(Error::Config(
            "limit_usd must be numeric".into(),
        )));
    }
    Ok(Json(
        BudgetRepo(pool(&state))
            .create(
                &body.scope_type,
                body.scope_id,
                &body.limit_usd,
                &body.period,
            )
            .await?,
    ))
}

async fn delete_budget(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    BudgetRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- rate limits ---

async fn list_rate_limits(
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<RateLimit>>> {
    validate_scope(&scope.scope_type)?;
    Ok(Json(
        RateLimitRepo(pool(&state))
            .list_for_scope(&scope.scope_type, scope.scope_id)
            .await?,
    ))
}

#[derive(Deserialize)]
struct CreateRateLimit {
    scope_type: String,
    scope_id: Uuid,
    rpm: Option<i32>,
    tpm: Option<i32>,
}

async fn create_rate_limit(
    State(state): State<ControlState>,
    Json(body): Json<CreateRateLimit>,
) -> ApiResult<Json<RateLimit>> {
    validate_scope(&body.scope_type)?;
    Ok(Json(
        RateLimitRepo(pool(&state))
            .create(&body.scope_type, body.scope_id, body.rpm, body.tpm)
            .await?,
    ))
}

async fn delete_rate_limit(
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    RateLimitRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- model pricing catalog ---

async fn list_model_prices(State(state): State<ControlState>) -> ApiResult<Json<Vec<ModelPrice>>> {
    Ok(Json(ModelPriceRepo(pool(&state)).list().await?))
}

#[derive(Deserialize)]
struct UpsertModelPrice {
    model: String,
    input_per_mtok: String,
    output_per_mtok: String,
    cached_input_per_mtok: Option<String>,
    #[serde(default = "default_currency")]
    currency: String,
}

fn default_currency() -> String {
    "USD".to_string()
}

fn require_numeric(value: &str, field: &str) -> ApiResult<()> {
    if value.trim().parse::<f64>().is_err() {
        return Err(ApiError::Core(Error::Config(format!(
            "{field} must be numeric"
        ))));
    }
    Ok(())
}

async fn upsert_model_price(
    State(state): State<ControlState>,
    Json(body): Json<UpsertModelPrice>,
) -> ApiResult<Json<ModelPrice>> {
    require_non_empty(&body.model, "model")?;
    require_numeric(&body.input_per_mtok, "input_per_mtok")?;
    require_numeric(&body.output_per_mtok, "output_per_mtok")?;
    if let Some(cached) = &body.cached_input_per_mtok {
        require_numeric(cached, "cached_input_per_mtok")?;
    }
    Ok(Json(
        ModelPriceRepo(pool(&state))
            .upsert(
                &body.model,
                &body.input_per_mtok,
                &body.output_per_mtok,
                body.cached_input_per_mtok.as_deref(),
                &body.currency,
            )
            .await?,
    ))
}

async fn delete_model_price(
    State(state): State<ControlState>,
    Path(model): Path<String>,
) -> ApiResult<StatusCode> {
    ModelPriceRepo(pool(&state)).delete(&model).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- effective model list (config + db, LiteLLM-style) ---

#[derive(Serialize)]
struct EffectiveModel {
    model: String,
    strategy: rolter_core::BalancingStrategy,
    targets: usize,
    /// "config" = declared in the bootstrap file, read-only at runtime;
    /// "db" = created via this API, full runtime CRUD
    source: &'static str,
}

/// The merged model list the gateway effectively serves: bootstrap-config
/// routes (read-only) plus DB routes, as exposed by the merged store.
async fn list_models(State(state): State<ControlState>) -> ApiResult<Json<Vec<EffectiveModel>>> {
    let config = state.store.load().await?;
    let models = config
        .routes
        .iter()
        .map(|r| EffectiveModel {
            model: r.model.clone(),
            strategy: r.strategy,
            targets: r.targets.len(),
            source: if state.config_owned.models.contains(&r.model) {
                "config"
            } else {
                "db"
            },
        })
        .collect();
    Ok(Json(models))
}

/// Delete a DB-defined model (all routes with that public name). Config
/// models are rejected with `409` since the file owns them.
async fn delete_model(
    State(state): State<ControlState>,
    Path(model): Path<String>,
) -> ApiResult<StatusCode> {
    require_not_config_owned(&state.config_owned.models, &model, "model")?;
    RouteRepo(pool(&state)).delete_by_model(&model).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}
