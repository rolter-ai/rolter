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

use rolter_auth::Role;
use rolter_core::slug::{is_valid_slug, slugify};
use rolter_core::Error;
use rolter_store::postgres::models::{
    Budget, Membership, ModelPrice, Org, Project, Provider, RateLimit, Route, RouteTarget, Team,
    User, VirtualKey,
};
use rolter_store::postgres::repo::{
    BudgetRepo, MembershipRepo, ModelPriceRepo, OrgRepo, ProjectRepo, ProviderKeyRepo,
    ProviderRepo, RateLimitRepo, RouteRepo, RouteTargetRepo, SessionRepo, TeamRepo, UserRepo,
    VirtualKeyRepo,
};

use crate::rbac::{authorize, require_superadmin, Principal, ScopeChain};
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
        .route(
            "/api/v1/providers/{id}",
            put(update_provider).delete(delete_provider),
        )
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
        .route(
            "/api/v1/orgs/{org_id}/users",
            get(list_users).post(create_user),
        )
        .route("/api/v1/users/{id}", put(update_user).delete(delete_user))
        .route(
            "/api/v1/orgs/{org_id}/memberships",
            get(list_memberships).post(create_membership),
        )
        .route("/api/v1/memberships/{id}", delete(delete_membership))
}

pub(crate) fn pool(state: &ControlState) -> &PgPool {
    state
        .pool
        .as_ref()
        .expect("crud router is only mounted when a postgres pool is configured")
}

#[derive(Debug)]
pub(crate) enum ApiError {
    Core(Error),
    /// mutation collides with a config-file-owned resource (409)
    Conflict(String),
    /// missing or invalid credentials (401)
    Unauthenticated,
    /// authenticated but lacking the required role at the scope (403)
    Forbidden,
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
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "missing or invalid credentials".to_string(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "insufficient role for this resource".to_string(),
            ),
        };
        (
            status,
            Json(serde_json::json!({"error": {"message": message}})),
        )
            .into_response()
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

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
pub(crate) async fn publish_config_change(state: &ControlState) -> ApiResult<()> {
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

/// Require admin on the project owning route `id` (walked route → project →
/// team → org). Used by the route mutations that only carry the route id.
async fn authorize_route(state: &ControlState, principal: &Principal, id: Uuid) -> ApiResult<()> {
    let route = RouteRepo(pool(state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(state), route.project_id).await?;
    authorize(state, principal, chain, Role::Admin).await
}

/// Require admin on the project owning virtual key `id`.
async fn authorize_virtual_key(
    state: &ControlState,
    principal: &Principal,
    id: Uuid,
) -> ApiResult<()> {
    let vk = VirtualKeyRepo(pool(state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(state), vk.project_id).await?;
    authorize(state, principal, chain, Role::Admin).await
}

// --- orgs ---

// global read: any authenticated principal (the extractor enforces auth when
// an admin token is configured, and is open otherwise)
async fn list_orgs(
    _principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Org>>> {
    Ok(Json(OrgRepo(pool(&state)).list().await?))
}

#[derive(Deserialize)]
struct CreateOrg {
    name: String,
    slug: String,
}

// creating a top-level org has no parent scope to be admin of, so it is
// superadmin-only
async fn create_org(
    principal: Principal,
    State(state): State<ControlState>,
    Json(body): Json<CreateOrg>,
) -> ApiResult<Json<Org>> {
    require_superadmin(&principal)?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    Ok(Json(
        OrgRepo(pool(&state)).create(&body.name, &body.slug).await?,
    ))
}

async fn delete_org(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize(&state, &principal, ScopeChain::org(id), Role::Admin).await?;
    OrgRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- teams ---

async fn list_teams(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Team>>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Viewer).await?;
    Ok(Json(TeamRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
struct CreateTeam {
    name: String,
}

async fn create_team(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateTeam>,
) -> ApiResult<Json<Team>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Admin).await?;
    require_non_empty(&body.name, "name")?;
    Ok(Json(
        TeamRepo(pool(&state)).create(org_id, &body.name).await?,
    ))
}

async fn delete_team(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let chain = ScopeChain::from_team(pool(&state), id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    TeamRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- projects ---

async fn list_projects(
    principal: Principal,
    State(state): State<ControlState>,
    Path(team_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Project>>> {
    let chain = ScopeChain::from_team(pool(&state), team_id).await?;
    authorize(&state, &principal, chain, Role::Viewer).await?;
    Ok(Json(ProjectRepo(pool(&state)).list(team_id).await?))
}

#[derive(Deserialize)]
struct CreateProject {
    name: String,
}

async fn create_project(
    principal: Principal,
    State(state): State<ControlState>,
    Path(team_id): Path<Uuid>,
    Json(body): Json<CreateProject>,
) -> ApiResult<Json<Project>> {
    let chain = ScopeChain::from_team(pool(&state), team_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    require_non_empty(&body.name, "name")?;
    Ok(Json(
        ProjectRepo(pool(&state))
            .create(team_id, &body.name)
            .await?,
    ))
}

async fn delete_project(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let chain = ScopeChain::from_project(pool(&state), id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    ProjectRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- providers ---

async fn list_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Provider>>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Viewer).await?;
    Ok(Json(ProviderRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
struct CreateProvider {
    name: String,
    /// stable URL-safe identity; when omitted it is derived from `name`
    slug: Option<String>,
    kind: String,
    api_base: String,
    /// upstream credential, sealed with AES-256-GCM (`ROLTER_KEK`) before it
    /// reaches the database; never returned by the API
    api_key: Option<String>,
    api_key_env: Option<String>,
    egress_proxy: Option<String>,
}

const PROVIDER_KINDS: [&str; 11] = [
    "openai",
    "anthropic",
    "openai_compatible",
    "ollama",
    "ollama_cloud",
    "llama_cpp",
    "openrouter",
    "tei",
    "azure_openai",
    "bedrock",
    "vertex",
];

/// Seal `api_key` with the deployment KEK for at-rest storage. An empty or
/// whitespace-only key is rejected; a missing `ROLTER_KEK` is a client-visible
/// configuration error rather than a silent plaintext fallback.
fn seal_api_key(api_key: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    use rolter_store::postgres::crypto::{Kek, KEK_ENV};
    require_non_empty(api_key, "api_key")?;
    let Some(kek) = Kek::from_env() else {
        return Err(ApiError::Core(Error::Config(format!(
            "storing provider keys requires the {KEK_ENV} environment variable on the \
             control plane (and the gateway, to decrypt snapshots)"
        ))));
    };
    Ok(kek.encrypt(api_key)?)
}

fn validate_kind(kind: &str) -> ApiResult<()> {
    if !PROVIDER_KINDS.contains(&kind) {
        return Err(ApiError::Core(Error::Config(format!(
            "kind must be one of {PROVIDER_KINDS:?}"
        ))));
    }
    Ok(())
}

/// Resolve the slug for a new provider: an explicit `slug` is validated as-is;
/// an omitted one is derived from `name`. A name with no ascii-alphanumerics
/// slugifies to the empty string, so the caller must supply an explicit slug.
fn resolve_new_slug(name: &str, slug: Option<&str>) -> ApiResult<String> {
    let candidate = match slug.map(str::trim).filter(|s| !s.is_empty()) {
        Some(explicit) => explicit.to_string(),
        None => slugify(name),
    };
    validate_slug(&candidate)?;
    Ok(candidate)
}

fn validate_slug(slug: &str) -> ApiResult<()> {
    if !is_valid_slug(slug) {
        return Err(ApiError::Core(Error::Config(
            "slug must match ^[a-z0-9][a-z0-9-]{0,62}$ (lowercase alphanumerics and \
             hyphens, 1-63 chars, not starting with a hyphen); a name with no ascii \
             letters or digits needs an explicit slug"
                .to_string(),
        )));
    }
    Ok(())
}

/// Decide whether a provider update changes the slug. `new` is the requested
/// slug from the body (`None`/empty means leave unchanged). Since the slug is a
/// stable identity, a real change requires `allow` (the `allow_slug_change`
/// opt-in); a no-op that repeats the current slug is always allowed. Returns
/// `Some(slug)` only when the row should actually change.
fn resolve_slug_change(new: Option<&str>, current: &str, allow: bool) -> ApiResult<Option<String>> {
    match new.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v == current => Ok(None),
        Some(v) => {
            if !allow {
                return Err(ApiError::Core(Error::Config(
                    "slug is immutable; pass allow_slug_change=true to rename it (this \
                     changes the provider-slug/model address)"
                        .to_string(),
                )));
            }
            validate_slug(v)?;
            Ok(Some(v.to_string()))
        }
    }
}

async fn create_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateProvider>,
) -> ApiResult<Json<Provider>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Admin).await?;
    require_non_empty(&body.name, "name")?;
    require_not_config_owned(&state.config_owned.providers, &body.name, "provider")?;
    require_non_empty(&body.api_base, "api_base")?;
    validate_kind(&body.kind)?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    // seal before touching the database so a missing KEK leaves no row behind
    let sealed = body.api_key.as_deref().map(seal_api_key).transpose()?;
    let row = ProviderRepo(pool(&state))
        .create(
            org_id,
            &body.name,
            &slug,
            &body.kind,
            &body.api_base,
            body.api_key_env.as_deref(),
            body.egress_proxy.as_deref(),
        )
        .await?;
    if let Some((ciphertext, nonce)) = sealed {
        ProviderKeyRepo(pool(&state))
            .set(row.id, &ciphertext, &nonce)
            .await?;
    }
    publish_config_change(&state).await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct UpdateProvider {
    /// new slug; rejected unless `allow_slug_change` is true, since the slug is
    /// a stable identity that addresses (`provider-slug/model`) depend on
    slug: Option<String>,
    /// explicit opt-in to rename an immutable slug
    #[serde(default)]
    allow_slug_change: bool,
    /// omit to leave unchanged
    kind: Option<String>,
    /// omit to leave unchanged
    api_base: Option<String>,
    /// omit to leave the stored credential unchanged; empty string deletes
    /// it; anything else rotates it
    api_key: Option<String>,
    /// omit to leave unchanged; empty string clears
    api_key_env: Option<String>,
    /// omit to leave unchanged; empty string clears
    egress_proxy: Option<String>,
}

/// Map an optional string field to the repo's tri-state: omitted = unchanged,
/// empty = clear, otherwise = set.
fn tri_state(field: &Option<String>) -> Option<Option<&str>> {
    field
        .as_deref()
        .map(|v| Some(v.trim()).filter(|v| !v.is_empty()))
}

async fn update_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProvider>,
) -> ApiResult<Json<Provider>> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        Role::Admin,
    )
    .await?;
    require_not_config_owned(&state.config_owned.providers, &existing.name, "provider")?;
    if let Some(kind) = &body.kind {
        validate_kind(kind)?;
    }
    if let Some(api_base) = &body.api_base {
        require_non_empty(api_base, "api_base")?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    // seal before writing anything so a missing KEK changes nothing
    let sealed = match body.api_key.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(key) => Some(Some(seal_api_key(key)?)),
    };
    let row = ProviderRepo(pool(&state))
        .update(
            id,
            slug_change.as_deref(),
            body.kind.as_deref(),
            body.api_base.as_deref(),
            tri_state(&body.api_key_env),
            tri_state(&body.egress_proxy),
        )
        .await?;
    match sealed {
        None => {}
        Some(None) => ProviderKeyRepo(pool(&state)).clear(id).await?,
        Some(Some((ciphertext, nonce))) => {
            ProviderKeyRepo(pool(&state))
                .set(id, &ciphertext, &nonce)
                .await?
        }
    }
    publish_config_change(&state).await?;
    Ok(Json(row))
}

async fn delete_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        Role::Admin,
    )
    .await?;
    ProviderRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- routes + targets ---

async fn list_routes(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Route>>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    authorize(&state, &principal, chain, Role::Viewer).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateRoute>,
) -> ApiResult<Json<Route>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRouteEnabled>,
) -> ApiResult<Json<Route>> {
    authorize_route(&state, &principal, id).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRouteParams>,
) -> ApiResult<Json<Route>> {
    authorize_route(&state, &principal, id).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_route(&state, &principal, id).await?;
    RouteRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_route_targets(
    principal: Principal,
    State(state): State<ControlState>,
    Path(route_id): Path<Uuid>,
) -> ApiResult<Json<Vec<RouteTarget>>> {
    let route = RouteRepo(pool(&state)).get(route_id).await?;
    let chain = ScopeChain::from_project(pool(&state), route.project_id).await?;
    authorize(&state, &principal, chain, Role::Viewer).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(route_id): Path<Uuid>,
    Json(body): Json<CreateRouteTarget>,
) -> ApiResult<Json<RouteTarget>> {
    authorize_route(&state, &principal, route_id).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let target = RouteTargetRepo(pool(&state)).get(id).await?;
    authorize_route(&state, &principal, target.route_id).await?;
    RouteTargetRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- virtual keys ---

async fn list_virtual_keys(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<VirtualKey>>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    authorize(&state, &principal, chain, Role::Viewer).await?;
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
pub(crate) fn key_pepper() -> String {
    std::env::var("ROLTER_KEY_PEPPER").unwrap_or_default()
}

pub(crate) fn generate_virtual_key(pepper: &str) -> (String, String, String) {
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateVirtualKey>,
) -> ApiResult<Json<CreatedVirtualKey>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            body.name.as_deref(),
            &body.models,
            body.cache,
            None,
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetVirtualKeyDisabled>,
) -> ApiResult<Json<VirtualKey>> {
    authorize_virtual_key(&state, &principal, id).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetVirtualKeyCache>,
) -> ApiResult<Json<VirtualKey>> {
    authorize_virtual_key(&state, &principal, id).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_cache(id, body.cache)
        .await?;
    publish_config_change(&state).await?;
    Ok(Json(row))
}

async fn delete_virtual_key(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_virtual_key(&state, &principal, id).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<Budget>>> {
    validate_scope(&scope.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &scope.scope_type, scope.scope_id).await?;
    authorize(&state, &principal, chain, Role::Viewer).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Json(body): Json<CreateBudget>,
) -> ApiResult<Json<Budget>> {
    validate_scope(&body.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &body.scope_type, body.scope_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let budget = BudgetRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_scope(pool(&state), &budget.scope_type, budget.scope_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    BudgetRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- rate limits ---

async fn list_rate_limits(
    principal: Principal,
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<RateLimit>>> {
    validate_scope(&scope.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &scope.scope_type, scope.scope_id).await?;
    authorize(&state, &principal, chain, Role::Viewer).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Json(body): Json<CreateRateLimit>,
) -> ApiResult<Json<RateLimit>> {
    validate_scope(&body.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &body.scope_type, body.scope_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    Ok(Json(
        RateLimitRepo(pool(&state))
            .create(&body.scope_type, body.scope_id, body.rpm, body.tpm)
            .await?,
    ))
}

async fn delete_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let limit = RateLimitRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_scope(pool(&state), &limit.scope_type, limit.scope_id).await?;
    authorize(&state, &principal, chain, Role::Admin).await?;
    RateLimitRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- model pricing catalog ---

async fn list_model_prices(
    _principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<ModelPrice>>> {
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

// the pricing catalog is a global (unscoped) resource, so its mutations are
// superadmin-only
async fn upsert_model_price(
    principal: Principal,
    State(state): State<ControlState>,
    Json(body): Json<UpsertModelPrice>,
) -> ApiResult<Json<ModelPrice>> {
    require_superadmin(&principal)?;
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
    principal: Principal,
    State(state): State<ControlState>,
    Path(model): Path<String>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
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
async fn list_models(
    _principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<EffectiveModel>>> {
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
// deleting a public model name removes routes across potentially many projects,
// so it cannot be scoped to a single org/team/project and is superadmin-only
async fn delete_model(
    principal: Principal,
    State(state): State<ControlState>,
    Path(model): Path<String>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
    require_not_config_owned(&state.config_owned.models, &model, "model")?;
    RouteRepo(pool(&state)).delete_by_model(&model).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- users + memberships (ROL-223) ---
//
// account lifecycle vs. role assignment are deliberately split by authority:
// inviting a user into an org and granting roles within it are org-admin
// operations (scoped, safe for a tenant admin), while editing or deleting the
// underlying global account — and toggling the cross-org `is_superadmin`
// bit — is superadmin-only. in open mode (no admin token) every caller is a
// superadmin, so both paths pass through unchanged for local/single-tenant use.

const MIN_PASSWORD_LEN: usize = 8;

/// hash a plaintext password with argon2id for at-rest storage; the repo layer
/// only ever sees the digest
fn hash_password(password: &str) -> ApiResult<String> {
    use argon2::password_hash::rand_core::OsRng;
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;

    if password.len() < MIN_PASSWORD_LEN {
        return Err(ApiError::Core(Error::Config(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ))));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::Core(Error::Config(format!("failed to hash password: {e}"))))
}

/// minimal email sanity check: trimmed, non-empty, with a single `@` separating
/// non-empty local and domain parts. deliberately permissive — full RFC 5322 is
/// not worth the surface here, we only guard against obvious junk
fn validate_email(email: &str) -> ApiResult<String> {
    let email = email.trim();
    let ok = match email.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@') && domain.contains('.')
        }
        None => false,
    };
    if !ok {
        return Err(ApiError::Core(Error::Config(
            "email must be a valid address".to_string(),
        )));
    }
    Ok(email.to_string())
}

fn validate_role(role: &str) -> ApiResult<()> {
    if !matches!(role, "admin" | "member" | "viewer") {
        return Err(ApiError::Core(Error::Config(
            "role must be one of admin, member, viewer".to_string(),
        )));
    }
    Ok(())
}

async fn list_users(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<User>>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Viewer).await?;
    Ok(Json(UserRepo(pool(&state)).list_in_org(org_id).await?))
}

#[derive(Deserialize)]
struct CreateUser {
    email: String,
    /// optional initial password; omit for an sso-only shell account that
    /// cannot log in locally until a password is set
    password: Option<String>,
    /// role granted at this org for the new account; defaults to `member`
    #[serde(default)]
    role: Option<String>,
}

#[derive(Serialize)]
struct CreatedUser {
    user: User,
    membership: Membership,
}

/// invite/create an account and grant it a role in this org atomically. an
/// org admin can onboard a user without superadmin: the new account carries no
/// superadmin bit and starts scoped to this org only.
async fn create_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateUser>,
) -> ApiResult<Json<CreatedUser>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Admin).await?;
    let email = validate_email(&body.email)?;
    let role = body.role.as_deref().unwrap_or("member");
    validate_role(role)?;
    let password_hash = match body.password.as_deref() {
        Some(p) => Some(hash_password(p)?),
        None => None,
    };

    let pool = pool(&state);
    if UserRepo(pool).find_by_email(&email).await?.is_some() {
        return Err(ApiError::Conflict(format!(
            "a user with email '{email}' already exists"
        )));
    }

    let user = UserRepo(pool)
        .create(&email, password_hash.as_deref(), false)
        .await?;
    let membership = MembershipRepo(pool)
        .create(user.id, Some(org_id), None, None, role)
        .await?;
    Ok(Json(CreatedUser { user, membership }))
}

#[derive(Deserialize)]
struct UpdateUser {
    email: Option<String>,
    password: Option<String>,
    is_superadmin: Option<bool>,
    /// set to `true` to deactivate (block login, revoke sessions) or `false` to
    /// re-enable the account
    deactivated: Option<bool>,
}

/// edit a global account. superadmin-only because it reaches across every org
/// the user belongs to and can grant the cross-org superadmin bit.
async fn update_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUser>,
) -> ApiResult<Json<User>> {
    require_superadmin(&principal)?;
    let pool = pool(&state);

    let email = match body.email.as_deref() {
        Some(e) => Some(validate_email(e)?),
        None => None,
    };
    let password_hash = match body.password.as_deref() {
        Some(p) => Some(hash_password(p)?),
        None => None,
    };

    // reject an email change that collides with a different account
    if let Some(ref new_email) = email {
        if let Some(existing) = UserRepo(pool).find_by_email(new_email).await? {
            if existing.id != id {
                return Err(ApiError::Conflict(format!(
                    "a user with email '{new_email}' already exists"
                )));
            }
        }
    }

    let mut user = UserRepo(pool)
        .update(
            id,
            email.as_deref(),
            password_hash.as_deref(),
            body.is_superadmin,
        )
        .await?;

    if let Some(deactivated) = body.deactivated {
        user = UserRepo(pool).set_deactivated(id, deactivated).await?;
        if deactivated {
            // cut existing access immediately, not just at token expiry
            SessionRepo(pool).delete_for_user(id).await?;
        }
    }

    Ok(Json(user))
}

async fn delete_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
    // memberships and sessions cascade on the users fk (on delete cascade)
    UserRepo(pool(&state)).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_memberships(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Membership>>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Viewer).await?;
    Ok(Json(
        MembershipRepo(pool(&state)).list_in_org(org_id).await?,
    ))
}

#[derive(Deserialize)]
struct CreateMembership {
    user_id: Uuid,
    /// one of `org` | `team` | `project`
    scope_type: String,
    scope_id: Uuid,
    role: String,
}

/// grant a role to a user at a scope within this org. requires admin at the
/// target scope, and the scope must resolve back to `org_id` so an org admin
/// cannot grant into another tenant.
async fn create_membership(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateMembership>,
) -> ApiResult<Json<Membership>> {
    validate_role(&body.role)?;
    let pool = pool(&state);

    let (chain, org, team, project) = match body.scope_type.as_str() {
        "org" => {
            let chain = ScopeChain::org(body.scope_id);
            (chain, Some(body.scope_id), None, None)
        }
        "team" => {
            let chain = ScopeChain::from_team(pool, body.scope_id).await?;
            (chain, None, Some(body.scope_id), None)
        }
        "project" => {
            let chain = ScopeChain::from_project(pool, body.scope_id).await?;
            (chain, None, None, Some(body.scope_id))
        }
        other => {
            return Err(ApiError::Core(Error::Config(format!(
                "scope_type must be one of org, team, project (got '{other}')"
            ))))
        }
    };

    if chain.org != Some(org_id) {
        return Err(ApiError::Core(Error::Config(
            "scope does not belong to this org".to_string(),
        )));
    }
    authorize(&state, &principal, chain, Role::Admin).await?;

    // the target user must exist (surfaces a 404 rather than a fk error)
    UserRepo(pool).get(body.user_id).await?;

    Ok(Json(
        MembershipRepo(pool)
            .create(body.user_id, org, team, project, &body.role)
            .await?,
    ))
}

async fn delete_membership(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let pool = pool(&state);
    let m = MembershipRepo(pool).get(id).await?;
    // authorize admin at the membership's own scope
    let chain = if let Some(project) = m.project_id {
        ScopeChain::from_project(pool, project).await?
    } else if let Some(team) = m.team_id {
        ScopeChain::from_team(pool, team).await?
    } else if let Some(org) = m.org_id {
        ScopeChain::org(org)
    } else {
        // a membership with no scope should not exist; treat as superadmin-only
        require_superadmin(&principal)?;
        MembershipRepo(pool).delete(id).await?;
        return Ok(StatusCode::NO_CONTENT);
    };
    authorize(&state, &principal, chain, Role::Admin).await?;
    MembershipRepo(pool).delete(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod slug_tests {
    use super::*;

    fn is_config_err(res: ApiResult<impl std::fmt::Debug>) -> bool {
        matches!(res, Err(ApiError::Core(Error::Config(_))))
    }

    #[test]
    fn new_slug_derives_from_name_when_omitted() {
        assert_eq!(resolve_new_slug("OpenAI MSK", None).unwrap(), "openai-msk");
    }

    #[test]
    fn new_slug_accepts_explicit_and_trims() {
        assert_eq!(
            resolve_new_slug("whatever", Some("  vllm-spb ")).unwrap(),
            "vllm-spb"
        );
    }

    #[test]
    fn new_slug_rejects_invalid_explicit() {
        assert!(is_config_err(resolve_new_slug("x", Some("Bad Slug"))));
    }

    #[test]
    fn new_slug_requires_explicit_when_name_has_no_ascii() {
        assert!(is_config_err(resolve_new_slug("非漢字", None)));
        assert_eq!(resolve_new_slug("非漢字", Some("kanji")).unwrap(), "kanji");
    }

    #[test]
    fn slug_change_is_noop_when_absent_or_unchanged() {
        assert_eq!(resolve_slug_change(None, "openai", false).unwrap(), None);
        assert_eq!(
            resolve_slug_change(Some(""), "openai", false).unwrap(),
            None
        );
        assert_eq!(
            resolve_slug_change(Some("openai"), "openai", false).unwrap(),
            None
        );
    }

    #[test]
    fn slug_change_rejected_without_opt_in() {
        assert!(is_config_err(resolve_slug_change(
            Some("renamed"),
            "openai",
            false
        )));
    }

    #[test]
    fn slug_change_allowed_with_opt_in_and_validated() {
        assert_eq!(
            resolve_slug_change(Some("renamed"), "openai", true).unwrap(),
            Some("renamed".to_string())
        );
        assert!(is_config_err(resolve_slug_change(
            Some("Bad"),
            "openai",
            true
        )));
    }
}

#[cfg(test)]
mod user_tests {
    use super::*;

    fn is_config_err<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(res, Err(ApiError::Core(Error::Config(_))))
    }

    #[test]
    fn email_accepts_plain_addresses_and_trims() {
        assert_eq!(validate_email("  a@b.com ").unwrap(), "a@b.com");
        assert_eq!(
            validate_email("user.name@sub.example.io").unwrap(),
            "user.name@sub.example.io"
        );
    }

    #[test]
    fn email_rejects_junk() {
        assert!(is_config_err(validate_email("")));
        assert!(is_config_err(validate_email("nope")));
        assert!(is_config_err(validate_email("@b.com")));
        assert!(is_config_err(validate_email("a@")));
        assert!(is_config_err(validate_email("a@b"))); // no dot in domain
        assert!(is_config_err(validate_email("a@@b.com")));
    }

    #[test]
    fn role_accepts_known_and_rejects_others() {
        assert!(validate_role("admin").is_ok());
        assert!(validate_role("member").is_ok());
        assert!(validate_role("viewer").is_ok());
        assert!(is_config_err(validate_role("superadmin")));
        assert!(is_config_err(validate_role("")));
    }

    #[test]
    fn password_hash_enforces_min_length_and_verifies() {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        use argon2::Argon2;

        assert!(is_config_err(hash_password("short")));
        let hash = hash_password("longenough").unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default()
            .verify_password(b"longenough", &parsed)
            .is_ok());
    }
}
