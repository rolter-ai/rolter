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
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::slug::{is_valid_slug, slugify};
use rolter_core::{AdvancedModelConfig, Error};
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::models::{
    AuditLogEntry, Budget, BusinessUnit, Customer, Membership, ModelPrice, Org, Project,
    PromptTemplate, PromptTemplateScope, PromptTemplateVersion, Provider, ProviderGroup,
    ProviderGroupMember, RateLimit, Route, RouteTarget, Skill, SkillVersion, Team, User,
    VirtualKey,
};
use rolter_store::postgres::repo::{
    AuditLogCursor, AuditLogDirection, AuditLogFilter, AuditLogRepo, BudgetRepo, BusinessUnitRepo,
    CustomerRepo, MembershipRepo, ModelPriceRepo, OrgRepo, ProjectRepo, PromptTemplateRepo,
    ProviderGroupRepo, ProviderKeyRepo, ProviderRepo, RateLimitRepo, RouteRepo, RouteTargetRepo,
    SessionRepo, SkillRepo, TeamRepo, UserRepo, VirtualKeyRepo,
};

use crate::access_control::caller_policy;
use crate::rbac::{authorize, authorize_superadmin, policy_allows, Principal, ScopeChain};
use crate::rbac_matrix::{cap, superadmin_cap, Requirement};
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
            "/api/v1/orgs/{org_id}/business-units",
            get(list_business_units).post(create_business_unit),
        )
        .route(
            "/api/v1/business-units/{id}",
            put(update_business_unit).delete(delete_business_unit),
        )
        .route(
            "/api/v1/orgs/{org_id}/customers",
            get(list_customers).post(create_customer),
        )
        .route(
            "/api/v1/customers/{id}",
            put(update_customer).delete(delete_customer),
        )
        .route(
            "/api/v1/orgs/{org_id}/prompt-templates",
            get(list_prompt_templates).post(create_prompt_template),
        )
        .route(
            "/api/v1/prompt-templates/{id}",
            put(update_prompt_template).delete(delete_prompt_template),
        )
        .route(
            "/api/v1/prompt-templates/{id}/versions",
            get(list_prompt_template_versions).post(create_prompt_template_version),
        )
        .route(
            "/api/v1/prompt-templates/{id}/publish",
            put(publish_prompt_template_version),
        )
        .route(
            "/api/v1/prompt-templates/{id}/rollback",
            put(rollback_prompt_template_version),
        )
        .route(
            "/api/v1/prompt-templates/{id}/versions/{version}/scopes",
            get(list_prompt_template_scopes).put(set_prompt_template_scopes),
        )
        .route(
            "/api/v1/orgs/{org_id}/skills",
            get(list_skills).post(create_skill),
        )
        .route(
            "/api/v1/orgs/{org_id}/skills/resolve/{slug}",
            get(resolve_published_skill),
        )
        .route(
            "/api/v1/skills/{id}",
            put(update_skill).delete(delete_skill),
        )
        .route(
            "/api/v1/skills/{id}/versions",
            get(list_skill_versions).post(create_skill_version),
        )
        .route("/api/v1/skills/{id}/publish", put(publish_skill_version))
        .route("/api/v1/skills/{id}/rollback", put(rollback_skill_version))
        .route(
            "/api/v1/orgs/{org_id}/providers",
            get(list_providers).post(create_provider),
        )
        .route(
            "/api/v1/providers/{id}",
            put(update_provider).delete(delete_provider),
        )
        .route("/api/v1/providers/{id}/test", post(test_provider))
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
        .route(
            "/api/v1/virtual-keys/{id}/attribution",
            put(set_virtual_key_attribution),
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
    /// the client has spent its budget of rejected attempts on a token
    /// endpoint and is locked for a while (429, #1079). Carries the remaining
    /// lock, which is rendered as `Retry-After`
    TooManyAttempts(std::time::Duration),
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        Self::Core(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // read before the match consumes `self`
        let retry_after = match &self {
            Self::TooManyAttempts(remaining) => Some(*remaining),
            _ => None,
        };
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
            Self::TooManyAttempts(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many rejected attempts; try again later".to_string(),
            ),
        };
        let mut response = (
            status,
            Json(serde_json::json!({"error": {"message": message}})),
        )
            .into_response();
        // say what the lock reads, so a client waits rather than polling. rounded
        // up, so a sub-second remainder never renders as `0`
        if let Some(retry_after) = retry_after {
            let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
            if let Ok(value) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
        }
        response
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

/// Reject control characters anywhere in a CRUD body's strings.
///
/// Postgres `text` columns cannot hold a NUL byte, so a field carrying one
/// fails deep inside the store and surfaces as an unhandled 500 rather than
/// input validation. The rest of the C0/C1 range is equally meaningless in a
/// name, slug, model or URL and is a log-injection vector, so the whole range
/// is rejected at the API boundary. Tab, newline and carriage return stay
/// allowed: multi-line values are legitimate (a PEM CA bundle, for one).
///
/// `path` names the offending field in the 400 so the caller can find it in a
/// nested body; the empty string is the request root.
fn reject_control_chars(value: &serde_json::Value, path: &str) -> ApiResult<()> {
    match value {
        serde_json::Value::String(text) => {
            if let Some(bad) = text
                .chars()
                .find(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
            {
                let field = if path.is_empty() { "body" } else { path };
                return Err(ApiError::Core(Error::Config(format!(
                    "{field} must not contain control characters (found U+{:04X})",
                    bad as u32
                ))));
            }
            Ok(())
        }
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                // the key itself lands in jsonb columns (advanced config, metadata)
                reject_control_chars(&serde_json::Value::String(key.clone()), path)?;
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                reject_control_chars(child, &child_path)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                reject_control_chars(child, &format!("{path}[{idx}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// `Json`, but every string in the body is screened for control characters
/// first (see [`reject_control_chars`]) and every failure — malformed JSON
/// included — comes back in the same OpenAI-style error envelope the rest of
/// the API uses. Every CRUD handler takes this instead of [`Json`] so the
/// guarantee holds for fields nobody validates individually.
pub(crate) struct SafeJson<T>(pub(crate) T);

impl<S, T> axum::extract::FromRequest<S> for SafeJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<serde_json::Value>::from_request(req, state)
            .await
            .map_err(|rejection| ApiError::Core(Error::Config(rejection.body_text())))?;
        reject_control_chars(&value, "")?;
        let parsed = serde_json::from_value(value)
            .map_err(|err| ApiError::Core(Error::Config(format!("invalid request body: {err}"))))?;
        Ok(Self(parsed))
    }
}

/// `Option<SafeJson<T>>`: absent when the request carries no JSON body at all,
/// so a POST whose every field is optional can be sent with no body. A body
/// that *is* present still goes through the same validation — being optional
/// is not a way to skip [`reject_control_chars`].
impl<S, T> axum::extract::OptionalFromRequest<S> for SafeJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(
        req: axum::extract::Request,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        let Some(Json(value)) = <Json<serde_json::Value> as axum::extract::OptionalFromRequest<
            S,
        >>::from_request(req, state)
        .await
        .map_err(|rejection| ApiError::Core(Error::Config(rejection.body_text())))?
        else {
            return Ok(None);
        };
        reject_control_chars(&value, "")?;
        let parsed = serde_json::from_value(value)
            .map_err(|err| ApiError::Core(Error::Config(format!("invalid request body: {err}"))))?;
        Ok(Some(Self(parsed)))
    }
}

/// Reject a required field that's empty after trimming.
pub(crate) fn require_non_empty(value: &str, field: &str) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Err(ApiError::Core(Error::Config(format!(
            "{field} must not be empty"
        ))));
    }
    Ok(())
}

/// Reject an upstream URL the deployment's egress policy denies, so an SSRF
/// target (cloud instance metadata, by default) never reaches the database.
/// The gateway re-checks the same policy when it validates a snapshot, but
/// failing at write time gives the operator a 400 at the point of the mistake
/// instead of a rejected snapshot later.
pub(crate) fn require_allowed_egress(
    state: &ControlState,
    url: &str,
    field: &str,
) -> ApiResult<()> {
    state
        .egress
        .check_url(url, field)
        .map_err(|problem| ApiError::Core(Error::Config(problem)))
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
pub(crate) async fn log_audit(
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
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("audit_log", Read),
    )
    .await?;
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

/// Check `requirement` on the project owning route `id` (walked route →
/// project → team → org). Used by the route handlers that only carry the route
/// id. Returns the resolved org id, for audit-log scoping.
async fn authorize_route(
    state: &ControlState,
    principal: &Principal,
    id: Uuid,
    requirement: Requirement,
) -> ApiResult<Option<Uuid>> {
    let route = RouteRepo(pool(state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(state), route.project_id).await?;
    let org_id = chain.org;
    authorize(state, principal, chain, requirement).await?;
    Ok(org_id)
}

/// Check `requirement` on the project owning virtual key `id`. Returns the
/// resolved org id, for audit-log scoping.
async fn authorize_virtual_key(
    state: &ControlState,
    principal: &Principal,
    id: Uuid,
    requirement: Requirement,
) -> ApiResult<Option<Uuid>> {
    let vk = VirtualKeyRepo(pool(state)).get(id).await?;
    let chain = ScopeChain::from_project(pool(state), vk.project_id).await?;
    let org_id = chain.org;
    authorize(state, principal, chain, requirement).await?;
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
#[serde(deny_unknown_fields)]
struct CreateOrg {
    name: String,
    slug: String,
}

// creating a top-level org has no parent scope to be admin of, so it is
// superadmin-only
async fn create_org(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateOrg>,
) -> ApiResult<Json<Org>> {
    authorize_superadmin(&principal, superadmin_cap!("org", Create))?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    validate_slug(&body.slug)?;
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
    authorize(&state, &principal, ScopeChain::org(id), cap!("org", Delete)).await?;
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
// --- business units ---

async fn list_business_units(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<BusinessUnit>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("business_unit", Read),
    )
    .await?;
    Ok(Json(BusinessUnitRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBusinessUnit {
    name: String,
    /// Stable URL-safe identity; derived from `name` when omitted.
    slug: Option<String>,
}

async fn create_business_unit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateBusinessUnit>,
) -> ApiResult<Json<BusinessUnit>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("business_unit", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let unit = BusinessUnitRepo(pool(&state))
        .create(org_id, &body.name, &slug)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "business_unit.create",
        "business_unit",
        unit.id,
        serde_json::json!({"name": unit.name, "slug": unit.slug}),
    )
    .await;
    Ok(Json(unit))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBusinessUnit {
    name: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    allow_slug_change: bool,
    retired: Option<bool>,
}

async fn update_business_unit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateBusinessUnit>,
) -> ApiResult<Json<BusinessUnit>> {
    let repo = BusinessUnitRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("business_unit", Update),
    )
    .await?;
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    let unit = repo
        .update(
            id,
            body.name.as_deref(),
            slug_change.as_deref(),
            body.retired,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "business_unit.update",
        "business_unit",
        id,
        serde_json::json!({"slug": unit.slug, "retired": unit.retired_at.is_some()}),
    )
    .await;
    Ok(Json(unit))
}

async fn delete_business_unit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = BusinessUnitRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("business_unit", Delete),
    )
    .await?;
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "business_unit.delete",
        "business_unit",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// --- customers ---

async fn list_customers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Customer>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("customer", Read),
    )
    .await?;
    Ok(Json(CustomerRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCustomer {
    name: String,
    /// Stable URL-safe identity; derived from `name` when omitted.
    slug: Option<String>,
    business_unit_id: Option<Uuid>,
}

async fn create_customer(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateCustomer>,
) -> ApiResult<Json<Customer>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("customer", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    if let Some(business_unit_id) = body.business_unit_id {
        let unit = BusinessUnitRepo(pool(&state)).get(business_unit_id).await?;
        if unit.org_id != org_id {
            return Err(ApiError::Core(Error::Config(
                "business_unit_id must belong to the same org".to_string(),
            )));
        }
    }
    let customer = CustomerRepo(pool(&state))
        .create(org_id, body.business_unit_id, &body.name, &slug)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "customer.create",
        "customer",
        customer.id,
        serde_json::json!({"name": customer.name, "slug": customer.slug, "business_unit_id": customer.business_unit_id}),
    )
    .await;
    Ok(Json(customer))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateCustomer {
    name: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    allow_slug_change: bool,
    /// Omit to leave unchanged, null to clear, UUID to set.
    #[serde(default)]
    business_unit_id: NullableUuid,
    retired: Option<bool>,
}

#[derive(Default)]
enum NullableUuid {
    #[default]
    Missing,
    Null,
    Value(Uuid),
}

impl<'de> Deserialize<'de> for NullableUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match Option::<Uuid>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

async fn update_customer(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateCustomer>,
) -> ApiResult<Json<Customer>> {
    let repo = CustomerRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("customer", Update),
    )
    .await?;
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    if let NullableUuid::Value(business_unit_id) = &body.business_unit_id {
        let unit = BusinessUnitRepo(pool(&state))
            .get(*business_unit_id)
            .await?;
        if unit.org_id != existing.org_id {
            return Err(ApiError::Core(Error::Config(
                "business_unit_id must belong to the same org".to_string(),
            )));
        }
    }
    let business_unit_id = match body.business_unit_id {
        NullableUuid::Missing => None,
        NullableUuid::Null => Some(None),
        NullableUuid::Value(id) => Some(Some(id)),
    };
    let customer = repo
        .update(
            id,
            business_unit_id,
            body.name.as_deref(),
            slug_change.as_deref(),
            body.retired,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "customer.update",
        "customer",
        id,
        serde_json::json!({"slug": customer.slug, "retired": customer.retired_at.is_some(), "business_unit_id": customer.business_unit_id}),
    )
    .await;
    Ok(Json(customer))
}

async fn delete_customer(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = CustomerRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("customer", Delete),
    )
    .await?;
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "customer.delete",
        "customer",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// --- prompt templates ---

async fn list_prompt_templates(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<PromptTemplate>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("prompt_template", Read),
    )
    .await?;
    Ok(Json(
        PromptTemplateRepo(pool(&state))
            .list_templates(org_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePromptTemplate {
    name: String,
    /// Stable URL-safe identity; derived from `name` when omitted.
    slug: Option<String>,
    description: Option<String>,
}

async fn create_prompt_template(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreatePromptTemplate>,
) -> ApiResult<Json<PromptTemplate>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("prompt_template", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let description = body.description.as_deref().map(str::trim);
    let template = PromptTemplateRepo(pool(&state))
        .create_template(org_id, &body.name, &slug, description)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "prompt_template.create",
        "prompt_template",
        template.id,
        serde_json::json!({"name": template.name, "slug": template.slug}),
    )
    .await;
    Ok(Json(template))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdatePromptTemplate {
    name: Option<String>,
    /// Omit to leave unchanged; otherwise set to the trimmed value.
    description: Option<String>,
}

async fn update_prompt_template(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdatePromptTemplate>,
) -> ApiResult<Json<PromptTemplate>> {
    let repo = PromptTemplateRepo(pool(&state));
    let existing = repo.get_template(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("prompt_template", Update),
    )
    .await?;
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    let template = repo
        .update_template(
            id,
            body.name.as_deref(),
            body.description.as_deref().map(str::trim),
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "prompt_template.update",
        "prompt_template",
        id,
        serde_json::json!({"name": template.name}),
    )
    .await;
    Ok(Json(template))
}

async fn delete_prompt_template(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = PromptTemplateRepo(pool(&state));
    let existing = repo.get_template(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("prompt_template", Delete),
    )
    .await?;
    repo.delete_template(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "prompt_template.delete",
        "prompt_template",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_prompt_template_versions(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<PromptTemplateVersion>>> {
    let repo = PromptTemplateRepo(pool(&state));
    let template = repo.get_template(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(template.org_id),
        cap!("prompt_template", Read),
    )
    .await?;
    Ok(Json(repo.list_versions(id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePromptTemplateVersion {
    #[serde(default)]
    variables: serde_json::Value,
    #[serde(default)]
    decorators: serde_json::Value,
}

async fn create_prompt_template_version(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<CreatePromptTemplateVersion>,
) -> ApiResult<Json<PromptTemplateVersion>> {
    if !body.variables.is_array() {
        return Err(ApiError::Core(Error::Config(
            "variables must be a JSON array".to_string(),
        )));
    }
    if !body.decorators.is_array() {
        return Err(ApiError::Core(Error::Config(
            "decorators must be a JSON array".to_string(),
        )));
    }
    let repo = PromptTemplateRepo(pool(&state));
    let template = repo.get_template(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(template.org_id),
        cap!("prompt_template", Update),
    )
    .await?;
    let version = repo
        .create_version(id, &body.variables, &body.decorators)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(template.org_id),
        "prompt_template.version.create",
        "prompt_template",
        id,
        serde_json::json!({"version": version.version}),
    )
    .await;
    Ok(Json(version))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishPromptTemplateVersion {
    version: i32,
}

async fn publish_prompt_template_version(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PublishPromptTemplateVersion>,
) -> ApiResult<Json<PromptTemplate>> {
    set_prompt_template_version(&principal, &state, id, body.version, "publish").await
}

async fn rollback_prompt_template_version(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PublishPromptTemplateVersion>,
) -> ApiResult<Json<PromptTemplate>> {
    set_prompt_template_version(&principal, &state, id, body.version, "rollback").await
}

async fn set_prompt_template_version(
    principal: &Principal,
    state: &ControlState,
    id: Uuid,
    version: i32,
    action: &str,
) -> ApiResult<Json<PromptTemplate>> {
    if version <= 0 {
        return Err(ApiError::Core(Error::Config(
            "version must be greater than zero".to_string(),
        )));
    }
    let repo = PromptTemplateRepo(pool(state));
    let existing = repo.get_template(id).await?;
    authorize(
        state,
        principal,
        ScopeChain::org(existing.org_id),
        cap!("prompt_template", Update),
    )
    .await?;
    let template = repo.publish_version(id, version).await?;
    log_audit(
        state,
        principal,
        Some(existing.org_id),
        &format!("prompt_template.{action}"),
        "prompt_template",
        id,
        serde_json::json!({"version": version}),
    )
    .await;
    Ok(Json(template))
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PromptTemplateScopeKind {
    Org,
    Project,
    Route,
    VirtualKey,
}

impl PromptTemplateScopeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Project => "project",
            Self::Route => "route",
            Self::VirtualKey => "virtual_key",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptTemplateScopeInput {
    scope_type: PromptTemplateScopeKind,
    scope_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetPromptTemplateScopes {
    scopes: Vec<PromptTemplateScopeInput>,
}

async fn ensure_scope_belongs_to_org(
    state: &ControlState,
    scope_type: PromptTemplateScopeKind,
    scope_id: Uuid,
    org_id: Uuid,
) -> ApiResult<()> {
    match scope_type {
        PromptTemplateScopeKind::Org => {
            if scope_id != org_id {
                return Err(ApiError::Core(Error::Config(
                    "org scope_id must match the template org".to_string(),
                )));
            }
        }
        PromptTemplateScopeKind::Project => {
            let chain = ScopeChain::from_project(pool(state), scope_id).await?;
            if chain.org != Some(org_id) {
                return Err(ApiError::Core(Error::Config(
                    "project scope_id must belong to the template org".to_string(),
                )));
            }
        }
        PromptTemplateScopeKind::Route => {
            let route = RouteRepo(pool(state)).get(scope_id).await?;
            let chain = ScopeChain::from_project(pool(state), route.project_id).await?;
            if chain.org != Some(org_id) {
                return Err(ApiError::Core(Error::Config(
                    "route scope_id must belong to the template org".to_string(),
                )));
            }
        }
        PromptTemplateScopeKind::VirtualKey => {
            let key = VirtualKeyRepo(pool(state)).get(scope_id).await?;
            let chain = ScopeChain::from_project(pool(state), key.project_id).await?;
            if chain.org != Some(org_id) {
                return Err(ApiError::Core(Error::Config(
                    "virtual_key scope_id must belong to the template org".to_string(),
                )));
            }
        }
    }
    Ok(())
}

async fn list_prompt_template_scopes(
    principal: Principal,
    State(state): State<ControlState>,
    Path((id, version)): Path<(Uuid, i32)>,
) -> ApiResult<Json<Vec<PromptTemplateScope>>> {
    let repo = PromptTemplateRepo(pool(&state));
    let template = repo.get_template(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(template.org_id),
        cap!("prompt_template", Read),
    )
    .await?;
    Ok(Json(repo.list_scopes(id, version).await?))
}

async fn set_prompt_template_scopes(
    principal: Principal,
    State(state): State<ControlState>,
    Path((id, version)): Path<(Uuid, i32)>,
    SafeJson(body): SafeJson<SetPromptTemplateScopes>,
) -> ApiResult<StatusCode> {
    if version <= 0 {
        return Err(ApiError::Core(Error::Config(
            "version must be greater than zero".to_string(),
        )));
    }
    let repo = PromptTemplateRepo(pool(&state));
    let template = repo.get_template(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(template.org_id),
        cap!("prompt_template", Update),
    )
    .await?;
    if template.published_version == Some(version) {
        return Err(ApiError::Core(Error::Config(
            "published prompt template scopes are immutable; create a new version".to_string(),
        )));
    }
    for scope in &body.scopes {
        ensure_scope_belongs_to_org(&state, scope.scope_type, scope.scope_id, template.org_id)
            .await?;
    }
    let scopes: Vec<(String, Uuid)> = body
        .scopes
        .into_iter()
        .map(|scope| (scope.scope_type.as_str().to_string(), scope.scope_id))
        .collect();
    repo.set_scopes(id, version, &scopes).await?;
    log_audit(
        &state,
        &principal,
        Some(template.org_id),
        "prompt_template.scopes.update",
        "prompt_template",
        id,
        serde_json::json!({"version": version, "scope_count": scopes.len()}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// --- skills ---

async fn list_skills(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Skill>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("skill", Read),
    )
    .await?;
    let mut visible = Vec::new();
    for skill in SkillRepo(pool(&state)).list_skills(org_id).await? {
        if policy_allows(
            &state,
            &principal,
            org_id,
            &skill.allowed_team_ids,
            &skill.minimum_role,
        )
        .await?
        {
            visible.push(skill);
        }
    }
    Ok(Json(visible))
}

async fn resolve_published_skill(
    principal: Principal,
    State(state): State<ControlState>,
    Path((org_id, slug)): Path<(Uuid, String)>,
) -> ApiResult<Json<SkillVersion>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("skill", Read),
    )
    .await?;
    let repo = SkillRepo(pool(&state));
    let skill = repo.get_by_slug(org_id, &slug).await?;
    if !policy_allows(
        &state,
        &principal,
        org_id,
        &skill.allowed_team_ids,
        &skill.minimum_role,
    )
    .await?
    {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(repo.resolve_published(skill.id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSkill {
    name: String,
    /// Stable URL-safe identity; derived from `name` when omitted.
    slug: Option<String>,
    description: Option<String>,
    #[serde(default)]
    allowed_team_ids: Vec<Uuid>,
    #[serde(default = "default_viewer_role")]
    minimum_role: String,
}

fn default_viewer_role() -> String {
    "viewer".to_string()
}

fn validate_access_role(role: &str) -> ApiResult<()> {
    match role {
        "viewer" | "member" | "admin" => Ok(()),
        _ => Err(ApiError::Core(Error::Config(
            "minimum_role must be one of viewer, member, admin".to_string(),
        ))),
    }
}

async fn ensure_skill_teams_belong_to_org(
    state: &ControlState,
    org_id: Uuid,
    team_ids: &[Uuid],
) -> ApiResult<()> {
    for team_id in team_ids {
        let team = TeamRepo(pool(state)).get(*team_id).await?;
        if team.org_id != org_id {
            return Err(ApiError::Core(Error::Config(
                "allowed_team_ids must belong to the skill org".to_string(),
            )));
        }
    }
    Ok(())
}

async fn create_skill(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateSkill>,
) -> ApiResult<Json<Skill>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("skill", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    validate_access_role(&body.minimum_role)?;
    ensure_skill_teams_belong_to_org(&state, org_id, &body.allowed_team_ids).await?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let skill = SkillRepo(pool(&state))
        .create_skill(
            org_id,
            &body.name,
            &slug,
            body.description.as_deref().map(str::trim),
            &body.allowed_team_ids,
            &body.minimum_role,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "skill.create",
        "skill",
        skill.id,
        serde_json::json!({"name": skill.name, "slug": skill.slug}),
    )
    .await;
    Ok(Json(skill))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateSkill {
    name: Option<String>,
    /// Omit to leave unchanged; otherwise set to the trimmed value.
    description: Option<String>,
    retired: Option<bool>,
    allowed_team_ids: Option<Vec<Uuid>>,
    minimum_role: Option<String>,
}

async fn update_skill(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateSkill>,
) -> ApiResult<Json<Skill>> {
    let repo = SkillRepo(pool(&state));
    let existing = repo.get_skill(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("skill", Update),
    )
    .await?;
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    if let Some(role) = &body.minimum_role {
        validate_access_role(role)?;
    }
    if let Some(team_ids) = &body.allowed_team_ids {
        ensure_skill_teams_belong_to_org(&state, existing.org_id, team_ids).await?;
    }
    let skill = repo
        .update_skill(
            id,
            body.name.as_deref(),
            body.description.as_deref().map(str::trim),
            body.retired,
            body.allowed_team_ids.as_deref(),
            body.minimum_role.as_deref(),
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "skill.update",
        "skill",
        id,
        serde_json::json!({"name": skill.name, "retired": skill.retired_at.is_some()}),
    )
    .await;
    Ok(Json(skill))
}

async fn delete_skill(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = SkillRepo(pool(&state));
    let existing = repo.get_skill(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("skill", Delete),
    )
    .await?;
    repo.delete_skill(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "skill.delete",
        "skill",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_skill_versions(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<SkillVersion>>> {
    let repo = SkillRepo(pool(&state));
    let existing = repo.get_skill(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("skill", Read),
    )
    .await?;
    if !policy_allows(
        &state,
        &principal,
        existing.org_id,
        &existing.allowed_team_ids,
        &existing.minimum_role,
    )
    .await?
    {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(repo.list_versions(id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateSkillVersion {
    content: Option<String>,
    content_ref: Option<String>,
    #[serde(default = "empty_json_object")]
    metadata: serde_json::Value,
}

fn empty_json_object() -> serde_json::Value {
    serde_json::json!({})
}

fn metadata_contains_sensitive_key(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => fields.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
            normalized.contains("secret")
                || normalized.contains("password")
                || normalized.contains("token")
                || normalized.contains("apikey")
                || metadata_contains_sensitive_key(value)
        }),
        serde_json::Value::Array(items) => items.iter().any(metadata_contains_sensitive_key),
        _ => false,
    }
}

fn validate_skill_reference(reference: &str) -> ApiResult<()> {
    let reference = reference.trim();
    let allowed_scheme = ["https://", "oci://", "s3://", "git+https://"]
        .iter()
        .any(|scheme| reference.starts_with(scheme));
    let authority_has_credentials = reference
        .split_once("://")
        .map(|(_, rest)| rest.split('/').next().unwrap_or_default().contains('@'))
        .unwrap_or(true);
    if !allowed_scheme || authority_has_credentials {
        return Err(ApiError::Core(Error::Config(
            "content_ref must use https, git+https, oci, or s3 without embedded credentials"
                .to_string(),
        )));
    }
    Ok(())
}

async fn create_skill_version(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateSkillVersion>,
) -> ApiResult<Json<SkillVersion>> {
    if !body.metadata.is_object() {
        return Err(ApiError::Core(Error::Config(
            "metadata must be a JSON object".to_string(),
        )));
    }
    if metadata_contains_sensitive_key(&body.metadata) {
        return Err(ApiError::Core(Error::Config(
            "metadata must not contain secret-bearing fields".to_string(),
        )));
    }
    match (&body.content, &body.content_ref) {
        (Some(content), None) => require_non_empty(content, "content")?,
        (None, Some(reference)) => validate_skill_reference(reference)?,
        _ => {
            return Err(ApiError::Core(Error::Config(
                "exactly one of content or content_ref is required".to_string(),
            )))
        }
    }
    let repo = SkillRepo(pool(&state));
    let existing = repo.get_skill(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("skill", Update),
    )
    .await?;
    let version = repo
        .create_version(
            id,
            body.content.as_deref().map(str::trim),
            body.content_ref.as_deref().map(str::trim),
            &body.metadata,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "skill.version.create",
        "skill",
        id,
        serde_json::json!({"version": version.version}),
    )
    .await;
    Ok(Json(version))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishSkillVersion {
    version: i32,
}

async fn publish_skill_version(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PublishSkillVersion>,
) -> ApiResult<Json<Skill>> {
    set_skill_version(&principal, &state, id, body.version, "publish").await
}

async fn rollback_skill_version(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PublishSkillVersion>,
) -> ApiResult<Json<Skill>> {
    set_skill_version(&principal, &state, id, body.version, "rollback").await
}

async fn set_skill_version(
    principal: &Principal,
    state: &ControlState,
    id: Uuid,
    version: i32,
    action: &str,
) -> ApiResult<Json<Skill>> {
    if version <= 0 {
        return Err(ApiError::Core(Error::Config(
            "version must be greater than zero".to_string(),
        )));
    }
    let repo = SkillRepo(pool(state));
    let existing = repo.get_skill(id).await?;
    authorize(
        state,
        principal,
        ScopeChain::org(existing.org_id),
        cap!("skill", Update),
    )
    .await?;
    let skill = repo.publish_version(id, version).await?;
    log_audit(
        state,
        principal,
        Some(existing.org_id),
        &format!("skill.{action}"),
        "skill",
        id,
        serde_json::json!({"version": version}),
    )
    .await;
    Ok(Json(skill))
}

// --- teams ---

async fn list_teams(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Team>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("team", Read),
    )
    .await?;
    Ok(Json(TeamRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTeam {
    name: String,
}

async fn create_team(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateTeam>,
) -> ApiResult<Json<Team>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("team", Create),
    )
    .await?;
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
    authorize(&state, &principal, chain, cap!("team", Delete)).await?;
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
    authorize(&state, &principal, chain, cap!("project", Read)).await?;
    Ok(Json(ProjectRepo(pool(&state)).list(team_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProject {
    name: String,
}

async fn create_project(
    principal: Principal,
    State(state): State<ControlState>,
    Path(team_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateProject>,
) -> ApiResult<Json<Project>> {
    let chain = ScopeChain::from_team(pool(&state), team_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("project", Create)).await?;
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
    authorize(&state, &principal, chain, cap!("project", Delete)).await?;
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

/// What a "Test connection" attempt found.
#[derive(Serialize)]
struct ProviderTestResult {
    /// whether the probe demonstrated a working provider API — for kinds whose
    /// probe is a catalogue that means a parseable model list, not merely a
    /// 2xx. `false` with a `status` in the 2xx range is the "answered, but not
    /// with a model list" case, and `error` says so (#980)
    reachable: bool,
    /// the URL actually probed, so an operator can see what was tried rather
    /// than guessing how `api_base` was turned into an endpoint
    probed_url: String,
    /// HTTP status the upstream returned; absent when the request never
    /// completed (DNS, TLS, connection refused, timeout)
    status: Option<u16>,
    latency_ms: u64,
    /// how the credential was resolved, so "401" and "no key configured" are
    /// distinguishable without reading the provider row
    credential: &'static str,
    /// how many models the upstream listed, when it answered with a catalogue
    models_found: Option<usize>,
    error: Option<String>,
}

/// What a probe response amounts to, once the body has been read.
struct ProbeVerdict {
    reachable: bool,
    models_found: Option<usize>,
    error: Option<String>,
}

/// Decide what a probe proved.
///
/// `reachable` used to be `status.is_success()` alone, so any endpoint
/// answering 2xx at the probed URL — a catch-all route, an SPA index, a reverse
/// proxy, an auth portal — reported green for a provider that could not serve a
/// single completion (#980). It is what made the doubled base in #947 look
/// healthy: the upstream answered 200 at `/v1/v1/models` with something that
/// was not a catalogue.
///
/// So for kinds whose probe *is* a model catalogue, the catalogue is the
/// evidence and the status is only the envelope. Kinds whose probe is a
/// liveness endpoint (Tei's `/health`) keep 2xx as the criterion — explicitly,
/// via [`rolter_core::probe_expectation`], rather than incidentally.
///
/// The three outcomes stay distinguishable: success, a flat failure, and
/// "answered, but not with a model list" — which is neither, and is the one an
/// operator most needs named, because the host is up and it is the URL or the
/// service behind it that is wrong.
fn judge_probe(
    kind: rolter_core::ProviderKind,
    status: u16,
    body: Option<&serde_json::Value>,
    url: &str,
    credential: &str,
) -> ProbeVerdict {
    // a provider that answers 200 with zero models is reachable but not yet
    // useful, and the operator should be able to see that difference
    let models_found = body.and_then(rolter_core::count_catalogue);
    let answered = (200..300).contains(&status);
    let proved = answered
        && match rolter_core::probe_expectation(kind, "/") {
            rolter_core::ProbeExpectation::Catalogue => models_found.is_some(),
            rolter_core::ProbeExpectation::Liveness => true,
        };
    ProbeVerdict {
        reachable: proved,
        models_found: proved.then_some(models_found).flatten(),
        error: (!proved).then(|| {
            if answered {
                format!(
                    "{status}: {url} answered, but not with a model list. Something other than \
                     the provider's API is serving that URL — check api_base for a duplicated \
                     path segment, a catch-all route or a login portal."
                )
            } else {
                match status {
                    401 | 403 => format!(
                        "{status}: the upstream rejected the credential (resolved from: \
                         {credential})"
                    ),
                    404 => format!("{status}: reached the host, but {url} is not served there"),
                    other => format!("{other}: the upstream refused the probe"),
                }
            }
        }),
    }
}

/// Probe a stored provider and report whether it actually answers.
///
/// Configuring a provider is otherwise a write with no feedback: the row saves,
/// and the first sign that the base URL is wrong or the key is stale is a failed
/// request through the gateway, attributed to whatever route happened to select
/// it. This closes that loop at the moment the operator is looking at the form.
///
/// Probes the same free, non-inference endpoint the gateway's health sweep uses
/// (`rolter_core::probe`), so a green test means the sweep will agree. Costs
/// nothing to run: it is a model-list call, not a completion.
async fn test_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProviderTestResult>> {
    // reveals whether a credential works, so it is gated like a mutation even
    // though it writes nothing
    let row: (Uuid, String, String, String, Option<String>) = sqlx::query_as(
        "select org_id, name, kind, api_base, api_key_env from providers where id=$1",
    )
    .bind(id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| Error::Store(e.to_string()))?
    .ok_or_else(|| Error::NotFound(format!("provider {id}")))?;
    let (org_id, name, kind, api_base, api_key_env) = row;

    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("provider", Update),
    )
    .await?;

    // the base was checked when it was stored, but the egress policy may have
    // been tightened since; a stored row is not a standing permission to egress
    require_allowed_egress(&state, &api_base, "api_base")?;

    let parsed_kind: rolter_core::ProviderKind =
        serde_json::from_value(serde_json::Value::String(kind.clone()))
            .map_err(|_| Error::Store(format!("unknown provider kind '{kind}'")))?;

    // same precedence the snapshot uses: a sealed key wins over the env var
    let sealed: Option<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("select ciphertext, nonce from provider_keys where provider_id=$1")
            .bind(id)
            .fetch_optional(pool(&state))
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
    let (secret, credential) = match sealed {
        Some((ciphertext, nonce)) => match Kek::from_env() {
            Some(kek) => match kek.decrypt(&ciphertext, &nonce) {
                Ok(plaintext) => (Some(plaintext), "stored"),
                // the operator needs to know this is a KEK problem, not a
                // provider problem — the upstream is never even contacted
                Err(_) => (None, "stored (undecryptable)"),
            },
            None => (None, "stored (KEK unset)"),
        },
        None => match api_key_env.as_deref().map(std::env::var) {
            Some(Ok(value)) => (Some(value), "env"),
            Some(Err(_)) => (None, "env (unset)"),
            None => (None, "none"),
        },
    };

    if credential == "stored (undecryptable)" || credential == "stored (KEK unset)" {
        return Ok(Json(ProviderTestResult {
            reachable: false,
            probed_url: String::new(),
            status: None,
            latency_ms: 0,
            credential,
            models_found: None,
            error: Some(format!(
                "the stored credential for '{name}' could not be read: {KEK_ENV} is unset or \
                 does not match the key it was sealed with. The upstream was not contacted."
            )),
        }));
    }

    let (url, headers) = rolter_core::probe_request(parsed_kind, &api_base, "/");
    let mut req = state
        .http
        .get(&url)
        .timeout(std::time::Duration::from_secs(10));
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(secret) = &secret {
        req = req.bearer_auth(secret);
    }

    let started = std::time::Instant::now();
    let outcome = req.send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    let result = match outcome {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.json::<serde_json::Value>().await.ok();
            let verdict = judge_probe(parsed_kind, status, body.as_ref(), &url, credential);
            ProviderTestResult {
                reachable: verdict.reachable,
                probed_url: url.clone(),
                status: Some(status),
                latency_ms,
                credential,
                models_found: verdict.models_found,
                error: verdict.error,
            }
        }
        // never surface the raw reqwest error: it can carry the full URL
        // including a query-string credential for kinds that authenticate that way
        Err(e) => ProviderTestResult {
            reachable: false,
            probed_url: url,
            status: None,
            latency_ms,
            credential,
            models_found: None,
            error: Some(if e.is_timeout() {
                "no response within 10s".to_string()
            } else if e.is_connect() {
                "could not connect — check the host, port and TLS".to_string()
            } else {
                "the request could not be completed".to_string()
            }),
        },
    };

    Ok(Json(result))
}

async fn list_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Provider>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("provider", Read),
    )
    .await?;
    Ok(Json(ProviderRepo(pool(&state)).list(org_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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

const PROVIDER_KINDS: [&str; 48] = [
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
    "gemini_interactions",
    "mistral",
    "groq",
    "xai",
    "meta_llama_api",
    "cohere",
    "perplexity",
    "together",
    "fireworks",
    "databricks",
    "aleph_alpha",
    "nebius",
    "ovhcloud",
    "scaleway",
    "deepseek",
    "qwen",
    "zhipu",
    "kimi",
    "ernie",
    "doubao",
    "hunyuan",
    "yi",
    "minimax",
    "baichuan",
    "gigachat",
    "yandex_gpt",
    "cloud_ru",
    "mts_ai",
    "naver",
    "upstage",
    "rinna",
    "rakuten",
    "sarvam",
    "krutrim",
    "falcon",
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

#[cfg(test)]
mod probe_verdict_tests {
    use super::*;
    use rolter_core::ProviderKind;
    use serde_json::json;

    fn judge(kind: ProviderKind, status: u16, body: Option<serde_json::Value>) -> ProbeVerdict {
        judge_probe(
            kind,
            status,
            body.as_ref(),
            "https://x.test/v1/models",
            "stored",
        )
    }

    /// The case from #980: an upstream answering 200 with an HTML page at the
    /// probed URL must not read as Reachable. A test that passes where
    /// inference fails is worse than no test.
    #[test]
    fn a_2xx_carrying_a_non_catalogue_body_is_not_reachable() {
        // reqwest fails to parse html as json, so the body arrives as None
        let verdict = judge(ProviderKind::Openai, 200, None);
        assert!(!verdict.reachable, "html answered as a green result");
        assert_eq!(verdict.models_found, None);
        let error = verdict.error.expect("the operator must be told why");
        assert!(error.contains("not with a model list"), "{error}");
        // distinct from a flat failure: it must not claim the host refused us
        assert!(!error.contains("refused the probe"), "{error}");
    }

    /// Valid JSON that simply is not a catalogue — an auth portal's `{"error":…}`
    /// or a proxy's status document.
    #[test]
    fn a_2xx_json_body_with_no_model_array_is_not_reachable() {
        let verdict = judge(ProviderKind::Openai, 200, Some(json!({"status": "ok"})));
        assert!(!verdict.reachable);
        assert!(verdict
            .error
            .is_some_and(|e| e.contains("not with a model list")));
    }

    #[test]
    fn a_real_catalogue_is_reachable_and_counted() {
        let verdict = judge(
            ProviderKind::Openai,
            200,
            Some(json!({"data": [{"id": "gpt-4o"}, {"id": "gpt-4o-mini"}]})),
        );
        assert!(verdict.reachable);
        assert_eq!(verdict.models_found, Some(2));
        assert!(verdict.error.is_none());
    }

    /// Reachable but empty is a real state, and a different one from "not a
    /// catalogue" — the operator can see it in `models_found`.
    #[test]
    fn an_empty_catalogue_is_still_reachable() {
        let verdict = judge(ProviderKind::Openai, 200, Some(json!({"data": []})));
        assert!(verdict.reachable);
        assert_eq!(verdict.models_found, Some(0));
    }

    /// Tei probes `/health`, which has no `data` array. A blanket
    /// "require a catalogue" would report a healthy embedder as down.
    #[test]
    fn a_liveness_probe_keeps_2xx_as_the_criterion() {
        assert!(judge(ProviderKind::Tei, 200, None).reachable);
        assert!(judge(ProviderKind::Tei, 200, Some(json!("OK"))).reachable);
    }

    /// Bedrock and Vertex answer their own catalogue shapes rather than `data`.
    #[test]
    fn the_control_plane_catalogue_shapes_are_recognised() {
        let bedrock = judge(
            ProviderKind::Bedrock,
            200,
            Some(json!({"modelSummaries": [{"modelId": "anthropic.claude"}]})),
        );
        assert!(bedrock.reachable);
        assert_eq!(bedrock.models_found, Some(1));

        let vertex = judge(
            ProviderKind::Vertex,
            200,
            Some(json!({"publisherModels": [{"name": "gemini"}]})),
        );
        assert!(vertex.reachable);
    }

    /// Non-2xx keeps the messages it always had.
    #[test]
    fn a_failing_status_still_explains_itself() {
        let unauthorized = judge(ProviderKind::Openai, 401, None);
        assert!(!unauthorized.reachable);
        assert!(unauthorized
            .error
            .is_some_and(|e| e.contains("rejected the credential")));

        let missing = judge(ProviderKind::Openai, 404, None);
        assert!(missing
            .error
            .is_some_and(|e| e.contains("not served there")));

        let broken = judge(ProviderKind::Openai, 503, None);
        assert!(broken
            .error
            .is_some_and(|e| e.contains("refused the probe")));
    }
}

#[cfg(test)]
mod api_base_tests {
    use super::*;

    #[test]
    fn a_doubled_v1_is_refused_for_openai_shaped_kinds() {
        // the exact base from #947, hit against a real GPUStack instance
        for kind in ["openai", "openai_compatible", "ollama", "llama_cpp"] {
            let err = require_wellformed_api_base(kind, "https://gpustack.localhost/v1")
                .expect_err("{kind} must refuse a doubled base");
            let message = format!("{err:?}");
            assert!(message.contains("/v1/v1/chat/completions"), "{message}");
        }
    }

    #[test]
    fn a_v1_base_is_required_for_stripping_kinds() {
        // the same spelling is correct here, so it must pass untouched
        for (kind, base) in [
            ("mistral", "https://api.mistral.ai/v1"),
            ("groq", "https://api.groq.com/openai/v1"),
            ("xai", "https://api.x.ai/v1"),
        ] {
            assert!(require_wellformed_api_base(kind, base).is_ok(), "{kind}");
        }
    }

    #[test]
    fn a_wellformed_openai_base_passes() {
        assert!(require_wellformed_api_base("openai", "https://api.openai.com").is_ok());
        assert!(require_wellformed_api_base("ollama", "http://localhost:11434").is_ok());
    }

    #[test]
    fn an_unknown_kind_defers_to_validate_kind() {
        // reporting "your base is malformed" for a kind that does not exist
        // would bury the real error
        assert!(require_wellformed_api_base("not_a_kind", "https://host/v1").is_ok());
    }

    /// Every kind the CRUD API accepts must be a kind core can parse, or
    /// `require_wellformed_api_base` silently skips it and the guard is a no-op.
    #[test]
    fn every_accepted_kind_is_known_to_core() {
        for kind in PROVIDER_KINDS {
            let parsed = serde_json::from_value::<rolter_core::ProviderKind>(
                serde_json::Value::String(kind.to_string()),
            );
            assert!(parsed.is_ok(), "core cannot parse accepted kind '{kind}'");
        }
        assert_eq!(PROVIDER_KINDS.len(), rolter_core::ProviderKind::ALL.len());
    }
}

fn validate_kind(kind: &str) -> ApiResult<()> {
    if !PROVIDER_KINDS.contains(&kind) {
        return Err(ApiError::Core(Error::Config(format!(
            "kind must be one of {PROVIDER_KINDS:?}"
        ))));
    }
    Ok(())
}

/// Reject an `api_base` that would double the version prefix.
///
/// For openai-shaped kinds the gateway appends `/v1/chat/completions` itself, so
/// a base that already ends in `/v1` resolves to `/v1/v1/chat/completions` and
/// every request 404s. The dashboard used to actively teach this mistake, its
/// base-URL placeholder being `https://api.openai.com/v1` for all kinds (#947).
///
/// Refused rather than silently trimmed: the operator pasted that URL from
/// somewhere, and quietly rewriting it hides the fact that this provider kind
/// wants a different base than the one they were reading about.
fn require_wellformed_api_base(kind: &str, api_base: &str) -> ApiResult<()> {
    let Ok(parsed) = serde_json::from_value::<rolter_core::ProviderKind>(
        serde_json::Value::String(kind.to_string()),
    ) else {
        // an unknown kind is validate_kind's error to report, not this one's
        return Ok(());
    };
    match parsed.api_base_problem(api_base) {
        Some(problem) => Err(ApiError::Core(Error::Config(format!(
            "api_base: {problem}"
        )))),
        None => Ok(()),
    }
}

/// Resolve the slug for a new provider: an explicit `slug` is validated as-is;
/// an omitted one is derived from `name`. A name with no ascii-alphanumerics
/// slugifies to the empty string, so the caller must supply an explicit slug.
pub(crate) fn resolve_new_slug(name: &str, slug: Option<&str>) -> ApiResult<String> {
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
    SafeJson(body): SafeJson<CreateProvider>,
) -> ApiResult<Json<Provider>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("provider", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    require_not_config_owned(&state.config_owned.providers, &body.name, "provider")?;
    require_non_empty(&body.api_base, "api_base")?;
    require_allowed_egress(&state, &body.api_base, "api_base")?;
    validate_kind(&body.kind)?;
    require_wellformed_api_base(&body.kind, &body.api_base)?;
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
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<UpdateProvider>,
) -> ApiResult<Json<Provider>> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    let org_id = existing.org_id;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("provider", Update),
    )
    .await?;
    require_not_config_owned(&state.config_owned.providers, &existing.name, "provider")?;
    if let Some(kind) = &body.kind {
        validate_kind(kind)?;
    }
    if let Some(api_base) = &body.api_base {
        require_non_empty(api_base, "api_base")?;
        require_allowed_egress(&state, api_base, "api_base")?;
        // an edit may change either half of the pair, so check the base
        // against the kind this provider will have once the update lands
        require_wellformed_api_base(body.kind.as_deref().unwrap_or(&existing.kind), api_base)?;
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
        cap!("provider", Delete),
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
#[serde(deny_unknown_fields)]
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
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("provider_group", Read),
    )
    .await?;
    let groups = ProviderGroupRepo(pool(&state)).list(org_id).await?;
    let mut views = Vec::with_capacity(groups.len());
    for group in groups {
        views.push(view_of(&state, group).await?);
    }
    Ok(Json(views))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<CreateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("provider_group", Create),
    )
    .await?;
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
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<UpdateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    let repo = ProviderGroupRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("provider_group", Update),
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
        cap!("provider_group", Delete),
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
    authorize(&state, &principal, chain, cap!("route", Read)).await?;
    let routes = RouteRepo(pool(&state)).list(project_id).await?;
    // an access profile may narrow which routes the caller sees (#534). With no
    // profile carrying a policy the filter is the identity, which is every
    // deployment that defines none
    let policy = caller_policy(&state, &principal).await?;
    Ok(Json(if policy.is_unrestricted() {
        routes
    } else {
        routes
            .into_iter()
            .filter(|route| policy.permits_route(&route.model))
            .collect()
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRoute {
    model: String,
    #[serde(default = "default_strategy")]
    strategy: String,
}

fn default_strategy() -> String {
    "round_robin".to_string()
}

const STRATEGIES: [&str; 14] = [
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
    "adaptive",
    "lora_aware",
    "predicted_latency",
];

async fn create_route(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateRoute>,
) -> ApiResult<Json<Route>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("route", Create)).await?;
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
#[serde(deny_unknown_fields)]
struct SetRouteEnabled {
    enabled: bool,
}

async fn set_route_enabled(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetRouteEnabled>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
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
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<SetRouteParams>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
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
// this GET has always been held to the route *mutation* bar (admin), not
// `route:read`; the capability it names records that rather than quietly
// widening it to a viewer's (#704)
async fn get_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize_route(&state, &principal, id, cap!("route", Update)).await?;
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
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
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
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<SetRouteAdvanced>,
) -> ApiResult<Json<Route>> {
    let org_id = authorize_route(&state, &principal, id, cap!("route", Update)).await?;
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
    let org_id = authorize_route(&state, &principal, id, cap!("route", Delete)).await?;
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
    authorize(&state, &principal, chain, cap!("route", Read)).await?;
    Ok(Json(RouteTargetRepo(pool(&state)).list(route_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<CreateRouteTarget>,
) -> ApiResult<Json<RouteTarget>> {
    let org_id = authorize_route(&state, &principal, route_id, cap!("route", Update)).await?;
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
    let org_id =
        authorize_route(&state, &principal, target.route_id, cap!("route", Update)).await?;
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
    authorize(&state, &principal, chain, cap!("virtual_key", Read)).await?;
    Ok(Json(VirtualKeyRepo(pool(&state)).list(project_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateVirtualKey {
    /// required, and the same rule the self-service mint path applies: the
    /// plaintext is shown once, so an unnamed key is unattributable forever
    /// after (#945)
    name: String,
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
    /// key lifetime in days; `None` mints a key that never expires, which the
    /// dashboard only sends for an explicit choice
    #[serde(default)]
    expires_in_days: Option<u32>,
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
    rolter_auth::hex::encode(bytes)
}

async fn create_virtual_key(
    principal: Principal,
    State(state): State<ControlState>,
    Path(project_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateVirtualKey>,
) -> ApiResult<Json<CreatedVirtualKey>> {
    let chain = ScopeChain::from_project(pool(&state), project_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("virtual_key", Create)).await?;
    let (name, expires_at) =
        crate::me::validated_name_and_expiry(&body.name, body.expires_in_days)?;
    let (key, key_hash, key_prefix) = generate_virtual_key(&key_pepper());
    let row = VirtualKeyRepo(pool(&state))
        .create(
            project_id,
            &key_hash,
            &key_prefix,
            Some(name.as_str()),
            &body.models,
            &body.providers,
            body.cache,
            None,
            expires_at,
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
#[serde(deny_unknown_fields)]
struct SetVirtualKeyProviders {
    /// empty restores the permissive default
    #[serde(default)]
    providers: Vec<String>,
}

async fn set_virtual_key_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyProviders>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
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
#[serde(deny_unknown_fields)]
struct SetVirtualKeyAttribution {
    /// Omit to leave unchanged, null to clear, UUID to set.
    #[serde(default)]
    business_unit_id: NullableUuid,
    /// Omit to leave unchanged, null to clear, UUID to set.
    #[serde(default)]
    customer_id: NullableUuid,
}

/// Point a key's spend at a business unit and/or customer. Both dimensions must
/// live in the key's own org, and a customer already owned by a business unit
/// may not be paired with a different one.
async fn set_virtual_key_attribution(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyAttribution>,
) -> ApiResult<Json<VirtualKey>> {
    let repo = VirtualKeyRepo(pool(&state));
    let existing = repo.get(id).await?;
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
    let business_unit_id = match body.business_unit_id {
        NullableUuid::Missing => existing.business_unit_id,
        NullableUuid::Null => None,
        NullableUuid::Value(unit_id) => {
            let unit = BusinessUnitRepo(pool(&state)).get(unit_id).await?;
            if Some(unit.org_id) != org_id {
                return Err(ApiError::Core(Error::Config(
                    "business_unit_id must belong to the same org".to_string(),
                )));
            }
            Some(unit_id)
        }
    };
    let customer_id = match body.customer_id {
        NullableUuid::Missing => existing.customer_id,
        NullableUuid::Null => None,
        NullableUuid::Value(customer_id) => {
            let customer = CustomerRepo(pool(&state)).get(customer_id).await?;
            if Some(customer.org_id) != org_id {
                return Err(ApiError::Core(Error::Config(
                    "customer_id must belong to the same org".to_string(),
                )));
            }
            Some(customer_id)
        }
    };
    if let (Some(unit_id), Some(customer_id)) = (business_unit_id, customer_id) {
        let customer = CustomerRepo(pool(&state)).get(customer_id).await?;
        if customer
            .business_unit_id
            .is_some_and(|owner| owner != unit_id)
        {
            return Err(ApiError::Core(Error::Config(
                "customer_id belongs to a different business unit".to_string(),
            )));
        }
    }
    let row = repo
        .set_attribution(id, business_unit_id, customer_id)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "virtual_key.set_attribution",
        "virtual_key",
        id,
        serde_json::json!({
            "business_unit_id": row.business_unit_id,
            "customer_id": row.customer_id,
        }),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetVirtualKeyDisabled {
    disabled: bool,
}

async fn set_virtual_key_disabled(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyDisabled>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
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
#[serde(deny_unknown_fields)]
struct SetVirtualKeyCache {
    /// null clears the override (inherit the route); false/true force it
    #[serde(default)]
    cache: Option<bool>,
}

async fn set_virtual_key_cache(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetVirtualKeyCache>,
) -> ApiResult<Json<VirtualKey>> {
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Update)).await?;
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
    let org_id = authorize_virtual_key(&state, &principal, id, cap!("virtual_key", Delete)).await?;
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

const SCOPE_TYPES: [&str; 6] = [
    "org",
    "team",
    "project",
    "virtual_key",
    "business_unit",
    "customer",
];

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
    authorize(&state, &principal, chain, cap!("budget", Read)).await?;
    Ok(Json(
        BudgetRepo(pool(&state))
            .list_for_scope(&scope.scope_type, scope.scope_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBudget {
    scope_type: String,
    scope_id: Uuid,
    limit_usd: String,
    #[serde(default = "default_period")]
    period: String,
    /// this budget's own answer to unpriced traffic, overriding the
    /// deployment-wide setting; omitted or null inherits it (#996)
    #[serde(default)]
    unpriced_policy: Option<String>,
}

fn default_period() -> String {
    "30d".to_string()
}

const UNPRICED_POLICIES: [&str; 3] = ["ignore", "warn", "block"];

/// Validate the optional per-budget unpriced-policy override.
///
/// The column carries the same check constraint, but a database error surfaces
/// as a 500; a caller who sends `blocked` deserves a 400 that names the three
/// values. Absent means "inherit the deployment-wide setting" and is always
/// allowed.
fn validate_unpriced_policy(value: Option<&str>) -> ApiResult<()> {
    match value {
        None => Ok(()),
        Some(policy) if UNPRICED_POLICIES.contains(&policy) => Ok(()),
        Some(_) => Err(ApiError::Core(Error::Config(format!(
            "unpriced_policy must be one of {UNPRICED_POLICIES:?}"
        )))),
    }
}

async fn create_budget(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateBudget>,
) -> ApiResult<Json<Budget>> {
    validate_scope(&body.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &body.scope_type, body.scope_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("budget", Create)).await?;
    if body.limit_usd.trim().parse::<f64>().is_err() {
        return Err(ApiError::Core(Error::Config(
            "limit_usd must be numeric".into(),
        )));
    }
    validate_unpriced_policy(body.unpriced_policy.as_deref())?;
    let row = BudgetRepo(pool(&state))
        .create(
            &body.scope_type,
            body.scope_id,
            &body.limit_usd,
            &body.period,
            body.unpriced_policy.as_deref(),
        )
        .await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.create",
        "budget",
        row.id,
        serde_json::json!({
            "scope_type": body.scope_type,
            "scope_id": body.scope_id,
            "limit_usd": body.limit_usd,
            // an override changes what the gateway will serve, so the audit
            // row has to say when one was set (#996)
            "unpriced_policy": body.unpriced_policy,
        }),
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
    authorize(&state, &principal, chain, cap!("budget", Delete)).await?;
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
    authorize(&state, &principal, chain, cap!("rate_limit", Read)).await?;
    Ok(Json(
        RateLimitRepo(pool(&state))
            .list_for_scope(&scope.scope_type, scope.scope_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRateLimit {
    scope_type: String,
    scope_id: Uuid,
    rpm: Option<i32>,
    tpm: Option<i32>,
}

async fn create_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateRateLimit>,
) -> ApiResult<Json<RateLimit>> {
    validate_scope(&body.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &body.scope_type, body.scope_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("rate_limit", Create)).await?;
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
    authorize(&state, &principal, chain, cap!("rate_limit", Delete)).await?;
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
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<ModelPrice>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_price", Read),
    )
    .await?;
    Ok(Json(ModelPriceRepo(pool(&state)).list().await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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

/// Reject a model price in a currency the deployment cannot convert.
///
/// The dashboard has always let an operator pick a currency, but until #650 it
/// was a display label the cost engine ignored — a EUR price was charged as if
/// it were USD. Now it is real, which means a code with no rate has to be
/// refused here: the alternative is storing a price nothing can convert and
/// discovering it as wrong spend later.
fn require_known_currency(state: &ControlState, currency: &str) -> ApiResult<()> {
    require_non_empty(currency, "currency")?;
    if state.currency.rate(currency).is_some() {
        return Ok(());
    }
    Err(ApiError::Core(Error::Config(format!(
        "currency '{}' has no conversion rate configured (base is '{}'); \
         add it to [currency.rates] in the bootstrap config, or price in the base currency",
        currency.trim(),
        state.currency.base
    ))))
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
    SafeJson(body): SafeJson<UpsertModelPrice>,
) -> ApiResult<Json<ModelPrice>> {
    authorize_superadmin(&principal, superadmin_cap!("model_price", Update))?;
    require_non_empty(&body.model, "model")?;
    require_numeric(&body.input_per_mtok, "input_per_mtok")?;
    require_numeric(&body.output_per_mtok, "output_per_mtok")?;
    if let Some(cached) = &body.cached_input_per_mtok {
        require_numeric(cached, "cached_input_per_mtok")?;
    }
    require_known_currency(&state, &body.currency)?;
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
    authorize_superadmin(&principal, superadmin_cap!("model_price", Delete))?;
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
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<EffectiveModel>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model", Read),
    )
    .await?;
    let config = state.store.load().await?;
    // model visibility carried by the caller's access profiles (#534);
    // unrestricted for a superadmin and for anyone holding no policy
    let policy = caller_policy(&state, &principal).await?;
    let models = config
        .routes
        .iter()
        .filter(|r| policy.permits_model(&r.model))
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
    authorize_superadmin(&principal, superadmin_cap!("model", Delete))?;
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
pub(crate) fn hash_password(password: &str) -> ApiResult<String> {
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
pub(crate) fn validate_email(email: &str) -> ApiResult<String> {
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

pub(crate) fn validate_role(role: &str) -> ApiResult<()> {
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
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("user", Read),
    )
    .await?;
    Ok(Json(UserRepo(pool(&state)).list_in_org(org_id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<CreateUser>,
) -> ApiResult<Json<CreatedUser>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("user", Create),
    )
    .await?;
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
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<UpdateUser>,
) -> ApiResult<Json<User>> {
    authorize_superadmin(&principal, superadmin_cap!("user", Update))?;
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
    authorize_superadmin(&principal, superadmin_cap!("user", Delete))?;
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
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("membership", Read),
    )
    .await?;
    Ok(Json(
        MembershipRepo(pool(&state)).list_in_org(org_id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
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
    SafeJson(body): SafeJson<CreateMembership>,
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
    authorize(&state, &principal, chain, cap!("membership", Create)).await?;

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
        // a membership with no scope should not exist — the schema forbids it.
        // this is a data-integrity fallback rather than a route capability, so
        // it names no `(resource, action)`: nothing but a superadmin may clean
        // such a row up
        authorize_superadmin(&principal, Requirement::unscoped_superadmin())?;
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
    authorize(&state, &principal, chain, cap!("membership", Delete)).await?;
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
mod control_char_tests {
    use super::*;
    use serde_json::json;

    fn check(value: serde_json::Value) -> ApiResult<()> {
        reject_control_chars(&value, "")
    }

    fn message(value: serde_json::Value) -> String {
        match check(value) {
            Err(ApiError::Core(Error::Config(problem))) => problem,
            other => panic!("expected a config error, got {other:?}"),
        }
    }

    #[test]
    fn nul_in_a_string_field_is_rejected_with_the_field_name() {
        // the #618 e2e payload: a route model carrying an embedded NUL, which
        // postgres text cannot store and which used to surface as a 500
        let problem = message(json!({"model": "gpt-4o\u{0}null"}));
        assert!(problem.contains("model"), "{problem}");
        assert!(problem.contains("U+0000"), "{problem}");
    }

    #[test]
    fn control_chars_are_rejected_anywhere_in_the_body() {
        assert!(message(json!({"a": {"b": "x\u{1}"}})).contains("a.b"));
        assert!(
            message(json!({"targets": [{"provider": "p\u{7}"}]})).contains("targets[0].provider")
        );
        assert!(message(json!(["ok", "bad\u{1b}"])).contains("[1]"));
        // an object key lands in jsonb columns too
        assert!(check(json!({"k\u{0}": "fine"})).is_err());
        // c1 and delete are controls as well
        assert!(check(json!({"a": "x\u{7f}"})).is_err());
        assert!(check(json!({"a": "x\u{9f}"})).is_err());
    }

    #[test]
    fn ordinary_and_multiline_values_pass() {
        // a PEM ca bundle is a legitimate multi-line field, so tab/newline/cr stay allowed
        assert!(
            check(json!({"ca_bundle": "-----BEGIN CERT-----\nabc\r\n\t-----END-----"})).is_ok()
        );
        assert!(check(json!({
            "name": "OpenAI MSK",
            "api_base": "https://api.openai.com/v1",
            "weight": 3,
            "enabled": true,
            "unset": null,
            "emoji": "проверка 🚀",
        }))
        .is_ok());
    }
}

/// #940: a CRUD body carrying a field the API does not know used to be
/// accepted and the field dropped. `POST /routes` with a `targets` array
/// returned `200 OK` and created a route with zero targets — an object that
/// cannot serve a single request, reported as a success.
#[cfg(test)]
mod unknown_field_tests {
    use super::*;
    use serde_json::json;

    /// The path SafeJson takes: `serde_json::Value` in, the request type out.
    /// Only the error is returned — the request types are deliberately not
    /// `Debug`, and what is under test is the rejection, not the value.
    fn parse<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<(), String> {
        serde_json::from_value::<T>(value)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn rejection<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> String {
        match parse::<T>(value) {
            Err(problem) => problem,
            Ok(()) => panic!("an unknown field was accepted and silently dropped"),
        }
    }

    #[test]
    fn a_route_payload_with_targets_is_rejected_rather_than_silently_dropped() {
        let problem = rejection::<CreateRoute>(json!({
            "model": "gpt-4o",
            "targets": [{"provider": "openai", "model": "gpt-4o"}],
        }));
        // the 400 has to name the offending key, or the caller is left
        // guessing which of their fields went nowhere
        assert!(problem.contains("targets"), "{problem}");
        assert!(problem.contains("unknown field"), "{problem}");
    }

    #[test]
    fn the_same_payload_without_the_bogus_field_still_parses() {
        assert!(
            parse::<CreateRoute>(json!({"model": "gpt-4o"})).is_ok(),
            "a valid body must keep working"
        );
    }

    #[test]
    fn the_rule_covers_providers_groups_and_keys_too() {
        assert!(parse::<CreateProvider>(json!({
            "name": "openai",
            "kind": "openai",
            "api_base": "https://api.openai.com",
            "api_kye": "sk-typo",
        }))
        .is_err());
        assert!(parse::<CreateProviderGroup>(json!({
            "name": "pool",
            "menbers": [],
        }))
        .is_err());
        assert!(parse::<CreateVirtualKey>(json!({
            "name": "ci",
            "moduls": ["gpt-4o"],
        }))
        .is_err());
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
