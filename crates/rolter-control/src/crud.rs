//! Control CRUD API: orgs, teams, projects, providers, routes/targets,
//! virtual keys, budgets, rate limits and the model pricing catalog.
//!
//! Thin Axum handlers over the `rolter_store::postgres::repo` repositories.
//! Only mounted when the control plane is started with `--database-url`
//! (see `main.rs`), since these routes need direct pool access beyond what
//! the [`rolter_store::ConfigStore`] trait exposes.

use axum::extract::{Path, Query, State};
use axum::http::{header::HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_auth::Role;
use rolter_core::slug::{is_valid_slug, slugify};
use rolter_core::{AdvancedModelConfig, Error};
use rolter_store::postgres::models::{
    AuditLogEntry, Budget, Membership, ModelPrice, Org, Project, Provider, ProviderGroup,
    ProviderGroupMember, RateLimit, Route, RouteTarget, Team, User, VirtualKey,
};
use rolter_store::postgres::repo::{
    AuditLogCursor, AuditLogDirection, AuditLogFilter, AuditLogRepo, BudgetRepo, MembershipRepo,
    ModelPriceRepo, OrgRepo, ProjectRepo, ProviderGroupRepo, ProviderKeyRepo, ProviderRepo,
    RateLimitRepo, RouteRepo, RouteTargetRepo, SessionRepo, TeamRepo, UserRepo, VirtualKeyRepo,
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
            "/api/v1/orgs/{org_id}/provider-groups",
            get(list_provider_groups).post(create_provider_group),
        )
        .route(
            "/api/v1/provider-groups/{id}",
            put(update_provider_group).delete(delete_provider_group),
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
            "/api/v1/routes/{id}/complexity",
            get(get_route_complexity).put(set_route_complexity),
        )
        .route("/api/v1/routes/{id}/advanced", put(set_route_advanced))
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
        .route(
            "/api/v1/virtual-keys/{id}/providers",
            put(set_virtual_key_providers),
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
        .route("/api/v1/orgs/{org_id}/audit-log", get(list_audit_log))
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

/// Record an admin/CRUD/auth action to the audit log. Best-effort: a logging
/// failure is warned about but never fails the request it's attached to.
async fn log_audit(
    state: &ControlState,
    principal: &Principal,
    org_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    detail: serde_json::Value,
) {
    let actor = match principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            org_id,
            actor,
            action,
            Some(target_type),
            Some(target_id),
            Some(detail),
        )
        .await
    {
        tracing::warn!(error = %err, action, "failed to write audit log entry");
    }
}

async fn list_audit_log(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<AuditLogQuery>,
) -> ApiResult<Json<AuditLogPageResponse>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Admin).await?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let direction = query.direction.unwrap_or_default();
    let filter = AuditLogFilter {
        actor_user_id: query.actor,
        action: normalized_filter(query.action, "action")?,
        target_type: normalized_filter(query.target_type, "target_type")?,
        start_at: query.start_at,
        end_at: query.end_at,
        cursor: query
            .cursor
            .as_deref()
            .map(parse_audit_cursor)
            .transpose()?,
        direction: direction.into(),
    };
    if filter
        .start_at
        .is_some_and(|start| filter.end_at.is_some_and(|end| start > end))
    {
        return Err(ApiError::Core(Error::Config(
            "start_at must be before or equal to end_at".to_string(),
        )));
    }
    let page = AuditLogRepo(pool(&state))
        .list_page(org_id, &filter, limit)
        .await?;
    let has_next = match direction {
        AuditLogQueryDirection::Next => page.has_more,
        AuditLogQueryDirection::Previous => !page.entries.is_empty(),
    };
    let has_previous = match direction {
        AuditLogQueryDirection::Next => filter.cursor.is_some(),
        AuditLogQueryDirection::Previous => page.has_more,
    };
    let next_cursor = has_next
        .then(|| page.entries.last().map(encode_audit_cursor))
        .flatten();
    let previous_cursor = has_previous
        .then(|| page.entries.first().map(encode_audit_cursor))
        .flatten();
    let total = if query.include_total {
        Some(AuditLogRepo(pool(&state)).count(org_id, &filter).await?)
    } else {
        None
    };
    Ok(Json(AuditLogPageResponse {
        items: page.entries,
        next_cursor,
        previous_cursor,
        has_next,
        has_previous,
        total,
    }))
}

#[derive(Deserialize)]
struct AuditLogQuery {
    limit: Option<i64>,
    #[serde(alias = "actor_user_id")]
    actor: Option<Uuid>,
    action: Option<String>,
    target_type: Option<String>,
    #[serde(alias = "from")]
    start_at: Option<DateTime<Utc>>,
    #[serde(alias = "to")]
    end_at: Option<DateTime<Utc>>,
    cursor: Option<String>,
    direction: Option<AuditLogQueryDirection>,
    #[serde(default)]
    include_total: bool,
}

#[derive(Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum AuditLogQueryDirection {
    #[default]
    Next,
    Previous,
}

impl From<AuditLogQueryDirection> for AuditLogDirection {
    fn from(direction: AuditLogQueryDirection) -> Self {
        match direction {
            AuditLogQueryDirection::Next => Self::Next,
            AuditLogQueryDirection::Previous => Self::Previous,
        }
    }
}

#[derive(Serialize)]
struct AuditLogPageResponse {
    items: Vec<AuditLogEntry>,
    next_cursor: Option<String>,
    previous_cursor: Option<String>,
    has_next: bool,
    has_previous: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<i64>,
}

fn normalized_filter(value: Option<String>, field: &str) -> ApiResult<Option<String>> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(ApiError::Core(Error::Config(format!(
                    "{field} must be 1-256 visible characters"
                ))));
            }
            Ok(value)
        })
        .transpose()
}

fn parse_audit_cursor(cursor: &str) -> ApiResult<AuditLogCursor> {
    let (at, id) = cursor.rsplit_once('|').ok_or_else(|| {
        ApiError::Core(Error::Config(
            "cursor must contain timestamp and id".to_string(),
        ))
    })?;
    let at = DateTime::parse_from_rfc3339(at)
        .map_err(|_| ApiError::Core(Error::Config("cursor has an invalid timestamp".to_string())))?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(id)
        .map_err(|_| ApiError::Core(Error::Config("cursor has an invalid id".to_string())))?;
    Ok(AuditLogCursor { at, id })
}

fn encode_audit_cursor(entry: &AuditLogEntry) -> String {
    format!(
        "{}|{}",
        entry.at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        entry.id
    )
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
/// Returns the resolved org id, for audit-log scoping.
async fn authorize_route(
    state: &ControlState,
    principal: &Principal,
    id: Uuid,
) -> ApiResult<Option<Uuid>> {
    let route = RouteRepo(pool(state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(state), route.project_id).await?;
    let org_id = chain.org;
    authorize(state, principal, chain, Role::Admin).await?;
    Ok(org_id)
}

/// Require admin on the project owning virtual key `id`. Returns the resolved
/// org id, for audit-log scoping.
async fn authorize_virtual_key(
    state: &ControlState,
    principal: &Principal,
    id: Uuid,
) -> ApiResult<Option<Uuid>> {
    let vk = VirtualKeyRepo(pool(state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(state), vk.project_id).await?;
    let org_id = chain.org;
    authorize(state, principal, chain, Role::Admin).await?;
    Ok(org_id)
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
    let org = OrgRepo(pool(&state)).create(&body.name, &body.slug).await?;
    log_audit(
        &state,
        &principal,
        Some(org.id),
        "org.create",
        "org",
        org.id,
        serde_json::json!({"name": org.name}),
    )
    .await;
    Ok(Json(org))
}

async fn delete_org(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize(&state, &principal, ScopeChain::org(id), Role::Admin).await?;
    OrgRepo(pool(&state)).delete(id).await?;
    // org_id is FK-cascaded, so deleting the org would delete a log row scoped
    // to it too; log this one unscoped so the deletion itself survives
    log_audit(
        &state,
        &principal,
        None,
        "org.delete",
        "org",
        id,
        serde_json::json!({}),
    )
    .await;
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
    let team = TeamRepo(pool(&state)).create(org_id, &body.name).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "team.create",
        "team",
        team.id,
        serde_json::json!({"name": team.name}),
    )
    .await;
    Ok(Json(team))
}

async fn delete_team(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let chain = ScopeChain::from_team(pool(&state), id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    TeamRepo(pool(&state)).delete(id).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "team.delete",
        "team",
        id,
        serde_json::json!({}),
    )
    .await;
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
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    require_non_empty(&body.name, "name")?;
    let project = ProjectRepo(pool(&state))
        .create(team_id, &body.name)
        .await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "project.create",
        "project",
        project.id,
        serde_json::json!({"name": project.name}),
    )
    .await;
    Ok(Json(project))
}

async fn delete_project(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let chain = ScopeChain::from_project(pool(&state), id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    ProjectRepo(pool(&state)).delete(id).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "project.delete",
        "project",
        id,
        serde_json::json!({}),
    )
    .await;
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
    #[serde(default)]
    egress_proxies: Vec<String>,
}

const PROVIDER_KINDS: [&str; 16] = [
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
    "gemini",
    "gemini_native",
    "mistral",
    "groq",
    "xai",
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
            &body.egress_proxies,
        )
        .await?;
    if let Some((ciphertext, nonce)) = sealed {
        ProviderKeyRepo(pool(&state))
            .set(row.id, &ciphertext, &nonce)
            .await?;
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider.create",
        "provider",
        row.id,
        serde_json::json!({"name": row.name, "slug": row.slug, "kind": row.kind}),
    )
    .await;
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
    /// omit to leave unchanged; an empty array clears
    egress_proxies: Option<Vec<String>>,
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
    let org_id = existing.org_id;
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
            body.egress_proxies.as_deref(),
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
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider.update",
        "provider",
        id,
        serde_json::json!({"slug": row.slug}),
    )
    .await;
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
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "provider.delete",
        "provider",
        id,
        serde_json::json!({"name": existing.name}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// --- provider groups (ADR-0017 addendum, ADR-0022) ---

/// A provider group with its resolved membership, as the API returns it.
#[derive(Serialize)]
struct ProviderGroupView {
    #[serde(flatten)]
    group: ProviderGroup,
    members: Vec<ProviderGroupMember>,
}

#[derive(Deserialize)]
struct GroupMemberInput {
    provider_id: Uuid,
    /// upstream model rewrite; omit for passthrough of the requested model
    #[serde(default)]
    upstream_model: Option<String>,
    #[serde(default = "default_member_weight")]
    weight: i32,
}

fn default_member_weight() -> i32 {
    1
}

fn to_member_tuples(members: &[GroupMemberInput]) -> Vec<(Uuid, Option<String>, i32)> {
    members
        .iter()
        .map(|m| {
            (
                m.provider_id,
                m.upstream_model.clone().filter(|s| !s.trim().is_empty()),
                m.weight.max(1),
            )
        })
        .collect()
}

fn validate_strategy(strategy: &str) -> ApiResult<()> {
    if !STRATEGIES.contains(&strategy) {
        return Err(ApiError::Core(Error::Config(format!(
            "strategy must be one of {STRATEGIES:?}"
        ))));
    }
    Ok(())
}

/// Reject a mutation against a readonly (config-owned) group slug (ADR-0022).
fn require_group_not_config_owned(state: &ControlState, slug: &str) -> ApiResult<()> {
    if state.config_owned.groups.contains(slug) {
        return Err(ApiError::Core(Error::Config(format!(
            "provider group '{slug}' is config-owned and cannot be modified via the API"
        ))));
    }
    Ok(())
}

async fn view_of(state: &ControlState, group: ProviderGroup) -> ApiResult<ProviderGroupView> {
    let members = ProviderGroupRepo(pool(state)).members(group.id).await?;
    Ok(ProviderGroupView { group, members })
}

async fn list_provider_groups(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ProviderGroupView>>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Viewer).await?;
    let groups = ProviderGroupRepo(pool(&state)).list(org_id).await?;
    let mut views = Vec::with_capacity(groups.len());
    for group in groups {
        views.push(view_of(&state, group).await?);
    }
    Ok(Json(views))
}

#[derive(Deserialize)]
struct CreateProviderGroup {
    name: String,
    /// stable URL-safe identity; derived from `name` when omitted
    slug: Option<String>,
    #[serde(default = "default_strategy")]
    strategy: String,
    #[serde(default)]
    members: Vec<GroupMemberInput>,
}

async fn create_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    authorize(&state, &principal, ScopeChain::org(org_id), Role::Admin).await?;
    require_non_empty(&body.name, "name")?;
    validate_strategy(&body.strategy)?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    // a readonly config group with this slug shadows any DB row — refuse early
    require_group_not_config_owned(&state, &slug)?;
    let repo = ProviderGroupRepo(pool(&state));
    let group = repo
        .create(org_id, &body.name, &slug, &body.strategy)
        .await?;
    repo.set_members(group.id, &to_member_tuples(&body.members))
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider_group.create",
        "provider_group",
        group.id,
        serde_json::json!({"name": group.name, "slug": group.slug, "strategy": group.strategy}),
    )
    .await;
    Ok(Json(view_of(&state, group).await?))
}

#[derive(Deserialize)]
struct UpdateProviderGroup {
    name: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    allow_slug_change: bool,
    strategy: Option<String>,
    /// when present, replaces the entire membership; omit to leave unchanged
    members: Option<Vec<GroupMemberInput>>,
}

async fn update_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    let repo = ProviderGroupRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        Role::Admin,
    )
    .await?;
    require_group_not_config_owned(&state, &existing.slug)?;
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    if let Some(strategy) = &body.strategy {
        validate_strategy(strategy)?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    let group = repo
        .update(
            id,
            body.name.as_deref(),
            slug_change.as_deref(),
            body.strategy.as_deref(),
        )
        .await?;
    if let Some(members) = &body.members {
        repo.set_members(id, &to_member_tuples(members)).await?;
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "provider_group.update",
        "provider_group",
        id,
        serde_json::json!({"slug": group.slug}),
    )
    .await;
    Ok(Json(view_of(&state, group).await?))
}

async fn delete_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = ProviderGroupRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        Role::Admin,
    )
    .await?;
    require_group_not_config_owned(&state, &existing.slug)?;
    repo.delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "provider_group.delete",
        "provider_group",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
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

const STRATEGIES: [&str; 11] = [
    "round_robin",
    "random",
    "power_of_two",
    "consistent_hash",
    "cache_aware",
    "weighted",
    "pipeline",
    "cheapest",
    "fastest",
    "precise_cache_aware",
    "lmcache_aware",
];

async fn create_route(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    Json(body): Json<CreateRoute>,
) -> ApiResult<Json<Route>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let org_id = chain.org;
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
    log_audit(
        &state,
        &principal,
        org_id,
        "route.create",
        "route",
        row.id,
        serde_json::json!({"model": row.model, "strategy": row.strategy}),
    )
    .await;
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
    let org_id = authorize_route(&state, &principal, id).await?;
    let row = RouteRepo(pool(&state))
        .set_enabled(id, body.enabled)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_enabled",
        "route",
        id,
        serde_json::json!({"enabled": body.enabled}),
    )
    .await;
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
    let org_id = authorize_route(&state, &principal, id).await?;
    // both must be json objects (or null → treated as empty) so the gateway can
    // deserialize them into the param map / policy
    let params = normalize_json_object(body.params, "params")?;
    let param_policy = normalize_json_object(body.param_policy, "param_policy")?;
    let row = RouteRepo(pool(&state))
        .set_params(id, &params, &param_policy)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_params",
        "route",
        id,
        serde_json::json!({"params": params, "param_policy": param_policy}),
    )
    .await;
    Ok(Json(row))
}

/// Read the policy as a first-class control-plane resource while storing it in
/// the snapshot-compatible route params JSON.
async fn get_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize_route(&state, &principal, id).await?;
    let route = RouteRepo(pool(&state)).get(id).await?;
    Ok(Json(
        route
            .params
            .get(rolter_balancer::complexity::POLICY_PARAM)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"tiers": []})),
    ))
}

/// Validate and persist one bounded policy. It travels in `params` through
/// `ConfigStore::load` and the existing atomic gateway snapshot; the gateway
/// reserves this key and removes it before any request is forwarded upstream.
async fn set_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(value): Json<serde_json::Value>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id).await?;
    let policy: rolter_balancer::complexity::ComplexityPolicy = serde_json::from_value(value)
        .map_err(|error| {
            ApiError::Core(Error::Config(format!("invalid complexity policy: {error}")))
        })?;
    policy
        .validate_shape()
        .map_err(|error| ApiError::Core(Error::Config(error)))?;
    let config = state.store.load().await?;
    let routes = config
        .routes
        .iter()
        .map(|route| route.model.clone())
        .collect();
    policy
        .validate_routes(&routes)
        .map_err(|error| ApiError::Core(Error::Config(error)))?;

    let existing = RouteRepo(pool(&state)).get(id).await?;
    let mut params = normalize_json_object(existing.params, "params")?;
    let Some(object) = params.as_object_mut() else {
        return Err(ApiError::Core(Error::Config(
            "params must be a json object".to_string(),
        )));
    };
    object.insert(
        rolter_balancer::complexity::POLICY_PARAM.to_string(),
        serde_json::to_value(&policy).map_err(|error| {
            ApiError::Core(Error::Config(format!("invalid complexity policy: {error}")))
        })?,
    );
    let row = RouteRepo(pool(&state))
        .set_params(id, &params, &existing.param_policy)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_complexity",
        "route",
        id,
        serde_json::json!({"complexity": policy}),
    )
    .await;
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

#[derive(Deserialize)]
struct SetRouteAdvanced {
    #[serde(default)]
    advanced: serde_json::Value,
}

fn validate_advanced(advanced: &AdvancedModelConfig) -> ApiResult<()> {
    if let Some(base_url) = &advanced.base_url {
        let url = reqwest::Url::parse(base_url).map_err(|_| {
            ApiError::Core(Error::Config(
                "base_url must be an absolute http(s) URL".to_string(),
            ))
        })?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(ApiError::Core(Error::Config(
                "base_url must be an absolute http(s) URL".to_string(),
            )));
        }
    }
    for (field, value) in [
        (
            "cache_write_per_mtok",
            advanced
                .pricing
                .as_ref()
                .and_then(|p| p.cache_write_per_mtok),
        ),
        (
            "image_per_unit",
            advanced.pricing.as_ref().and_then(|p| p.image_per_unit),
        ),
        (
            "audio_input_per_minute",
            advanced
                .pricing
                .as_ref()
                .and_then(|p| p.audio_input_per_minute),
        ),
        (
            "audio_output_per_minute",
            advanced
                .pricing
                .as_ref()
                .and_then(|p| p.audio_output_per_minute),
        ),
    ] {
        if value.is_some_and(|price| !price.is_finite() || price < 0.0) {
            return Err(ApiError::Core(Error::Config(format!(
                "{field} must be a finite non-negative number"
            ))));
        }
    }
    for (field, value) in [
        ("rpm", advanced.limits.rpm),
        ("tpm", advanced.limits.tpm),
        ("concurrency", advanced.limits.concurrency),
        ("timeout_secs", advanced.limits.timeout_secs),
        ("retries", advanced.limits.retries),
        ("context_window", advanced.limits.context_window),
        ("output_tokens", advanced.limits.output_tokens),
    ] {
        if value.is_some_and(|limit| limit == 0 || limit > 10_000_000) {
            return Err(ApiError::Core(Error::Config(format!(
                "{field} must be between 1 and 10000000"
            ))));
        }
    }
    for name in advanced
        .headers
        .keys()
        .chain(advanced.locked_headers.iter())
    {
        HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ApiError::Core(Error::Config(format!("invalid header name '{name}'"))))?;
    }
    for id in advanced
        .visibility
        .allowed_team_ids
        .iter()
        .chain(advanced.visibility.allowed_key_ids.iter())
        .chain(advanced.visibility.allowed_user_ids.iter())
    {
        Uuid::parse_str(id).map_err(|_| {
            ApiError::Core(Error::Config(format!(
                "invalid scoped access reference '{id}'"
            )))
        })?;
    }
    Ok(())
}

async fn set_route_advanced(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetRouteAdvanced>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id).await?;
    let advanced_value = normalize_json_object(body.advanced, "advanced")?;
    let advanced: AdvancedModelConfig =
        serde_json::from_value(advanced_value.clone()).map_err(|err| {
            ApiError::Core(Error::Config(format!(
                "invalid advanced model configuration: {err}"
            )))
        })?;
    validate_advanced(&advanced)?;
    let row = RouteRepo(pool(&state))
        .set_advanced(id, &advanced_value)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.set_advanced",
        "route",
        id,
        serde_json::json!({"advanced": advanced_value}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_route(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org_id = authorize_route(&state, &principal, id).await?;
    RouteRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route.delete",
        "route",
        id,
        serde_json::json!({}),
    )
    .await;
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
    let org_id = authorize_route(&state, &principal, route_id).await?;
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
    log_audit(
        &state,
        &principal,
        org_id,
        "route_target.create",
        "route_target",
        row.id,
        serde_json::json!({"route_id": route_id, "provider_id": body.provider_id}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_route_target(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let target = RouteTargetRepo(pool(&state)).get(id).await?;
    let org_id = authorize_route(&state, &principal, target.route_id).await?;
    RouteTargetRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "route_target.delete",
        "route_target",
        id,
        serde_json::json!({}),
    )
    .await;
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
    /// upstream providers this key may reach; an empty list permits every
    /// provider on an allowed route
    #[serde(default)]
    providers: Vec<String>,
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
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            body.name.as_deref(),
            &body.models,
            &body.providers,
            body.cache,
            None,
        )
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.create",
        "virtual_key",
        row.id,
        serde_json::json!({"name": row.name, "key_prefix": key_prefix}),
    )
    .await;
    Ok(Json(CreatedVirtualKey { row, key }))
}

#[derive(Deserialize)]
struct SetVirtualKeyProviders {
    /// empty restores the permissive default
    #[serde(default)]
    providers: Vec<String>,
}

async fn set_virtual_key_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetVirtualKeyProviders>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_providers(id, &body.providers)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_providers",
        "virtual_key",
        id,
        serde_json::json!({"providers": body.providers}),
    )
    .await;
    Ok(Json(row))
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
    let org_id = authorize_virtual_key(&state, &principal, id).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_disabled(id, body.disabled)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_disabled",
        "virtual_key",
        id,
        serde_json::json!({"disabled": body.disabled}),
    )
    .await;
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
    let org_id = authorize_virtual_key(&state, &principal, id).await?;
    let row = VirtualKeyRepo(pool(&state))
        .set_cache(id, body.cache)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_cache",
        "virtual_key",
        id,
        serde_json::json!({"cache": body.cache}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_virtual_key(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let org_id = authorize_virtual_key(&state, &principal, id).await?;
    VirtualKeyRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.delete",
        "virtual_key",
        id,
        serde_json::json!({}),
    )
    .await;
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
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    if body.limit_usd.trim().parse::<f64>().is_err() {
        return Err(ApiError::Core(Error::Config(
            "limit_usd must be numeric".into(),
        )));
    }
    let row = BudgetRepo(pool(&state))
        .create(
            &body.scope_type,
            body.scope_id,
            &body.limit_usd,
            &body.period,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.create",
        "budget",
        row.id,
        serde_json::json!({"scope_type": body.scope_type, "scope_id": body.scope_id, "limit_usd": body.limit_usd}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_budget(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let budget = BudgetRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_scope(pool(&state), &budget.scope_type, budget.scope_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    BudgetRepo(pool(&state)).delete(id).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.delete",
        "budget",
        id,
        serde_json::json!({}),
    )
    .await;
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
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    let row = RateLimitRepo(pool(&state))
        .create(&body.scope_type, body.scope_id, body.rpm, body.tpm)
        .await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "rate_limit.create",
        "rate_limit",
        row.id,
        serde_json::json!({"scope_type": body.scope_type, "scope_id": body.scope_id, "rpm": body.rpm, "tpm": body.tpm}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let limit = RateLimitRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_scope(pool(&state), &limit.scope_type, limit.scope_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    RateLimitRepo(pool(&state)).delete(id).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "rate_limit.delete",
        "rate_limit",
        id,
        serde_json::json!({}),
    )
    .await;
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
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "user.create",
        "user",
        user.id,
        serde_json::json!({"email": user.email, "role": role}),
    )
    .await;
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

    // global account edit spans orgs, so it's logged unscoped
    log_audit(
        &state,
        &principal,
        None,
        "user.update",
        "user",
        id,
        serde_json::json!({"email": user.email, "deactivated": body.deactivated}),
    )
    .await;
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
    log_audit(
        &state,
        &principal,
        None,
        "user.delete",
        "user",
        id,
        serde_json::json!({}),
    )
    .await;
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

    let membership = MembershipRepo(pool)
        .create(body.user_id, org, team, project, &body.role)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "membership.create",
        "membership",
        membership.id,
        serde_json::json!({"user_id": body.user_id, "role": body.role, "scope_type": body.scope_type, "scope_id": body.scope_id}),
    )
    .await;
    Ok(Json(membership))
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
        log_audit(
            &state,
            &principal,
            None,
            "membership.delete",
            "membership",
            id,
            serde_json::json!({}),
        )
        .await;
        return Ok(StatusCode::NO_CONTENT);
    };
    let org_id = chain.org;
    authorize(&state, &principal, chain, Role::Admin).await?;
    MembershipRepo(pool).delete(id).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "membership.delete",
        "membership",
        id,
        serde_json::json!({}),
    )
    .await;
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

    #[test]
    fn validate_strategy_accepts_known_and_rejects_unknown() {
        assert!(validate_strategy("weighted").is_ok());
        assert!(validate_strategy("round_robin").is_ok());
        assert!(is_config_err(validate_strategy("nonsense")));
    }

    #[test]
    fn member_tuples_clamp_weight_and_drop_empty_upstream() {
        let id = Uuid::nil();
        let input = vec![
            GroupMemberInput {
                provider_id: id,
                upstream_model: Some("  ".to_string()),
                weight: 0,
            },
            GroupMemberInput {
                provider_id: id,
                upstream_model: Some("qwen3".to_string()),
                weight: 5,
            },
        ];
        let tuples = to_member_tuples(&input);
        // blank upstream_model becomes passthrough (None); zero weight clamps to 1
        assert_eq!(tuples[0], (id, None, 1));
        assert_eq!(tuples[1], (id, Some("qwen3".to_string()), 5));
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
    #[test]
    fn advanced_model_validation_rejects_unsafe_values() {
        let mut advanced = AdvancedModelConfig {
            base_url: Some("ftp://models.example".to_string()),
            ..Default::default()
        };
        assert!(is_config_err(validate_advanced(&advanced)));

        advanced.base_url = Some("https://models.example/v1".to_string());
        advanced
            .headers
            .insert("bad header".to_string(), "x".to_string());
        assert!(is_config_err(validate_advanced(&advanced)));

        advanced.headers.clear();
        advanced.limits.output_tokens = Some(0);
        assert!(is_config_err(validate_advanced(&advanced)));
    }

    #[test]
    fn audit_cursor_roundtrips_and_rejects_malformed_values() {
        let entry = AuditLogEntry {
            id: Uuid::nil(),
            org_id: None,
            actor_user_id: None,
            action: "route.create".to_string(),
            target_type: Some("route".to_string()),
            target_id: None,
            detail: None,
            at: Utc::now(),
        };
        let cursor = encode_audit_cursor(&entry);
        let parsed = parse_audit_cursor(&cursor).unwrap();
        assert_eq!(parsed.id, entry.id);
        assert_eq!(parsed.at, entry.at);
        assert!(is_config_err(parse_audit_cursor("bad")));
    }

    #[test]
    fn audit_filter_rejects_control_characters() {
        assert!(is_config_err(normalized_filter(
            Some("route\ncreate".to_string()),
            "action"
        )));
    }
}

#[cfg(test)]
mod virtual_key_tests {
    use super::*;

    #[test]
    fn generates_expected_format() {
        let pepper = "test_pepper";
        let (key, hash, prefix) = generate_virtual_key(pepper);

        // Check key format
        assert!(key.starts_with("sk-rolter-"));
        // 24 bytes hex encoded = 48 chars. + 10 chars for "sk-rolter-" = 58 chars
        assert_eq!(key.len(), 58);
        assert!(key["sk-rolter-".len()..]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));

        // Check prefix
        assert_eq!(prefix, key.chars().take(12).collect::<String>());
        assert_eq!(prefix.len(), 12);

        // Check hash consistency
        assert_eq!(hash, rolter_auth::hash_key(pepper, &key));
    }

    #[test]
    fn distinct_peppers_produce_different_hashes() {
        let (key1, hash1, _) = generate_virtual_key("pepper_one");
        let hash2 = rolter_auth::hash_key("pepper_two", &key1);

        assert_ne!(hash1, hash2);
    }
}
