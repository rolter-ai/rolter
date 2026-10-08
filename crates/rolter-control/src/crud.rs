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
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use rolter_core::slug::{is_valid_slug, slugify};
use rolter_core::{AdvancedModelConfig, BudgetPeriod, Error};
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::models::{
    AuditLogEntry, Budget, BusinessUnit, BusinessUnitListing, Customer, CustomerListing,
    Membership, ModelPrice, Org, OrgProject, Project, PromptTemplate, PromptTemplateScope,
    PromptTemplateVersion, Provider, ProviderGroup, ProviderGroupMember, RateLimit, Route,
    RouteTarget, Skill, SkillVersion, Team, User, VirtualKey,
};
use rolter_store::postgres::repo::{
    AuditLogCursor, AuditLogDirection, AuditLogFilter, AuditLogPage, AuditLogRepo, BudgetRepo,
    BusinessUnitRepo, CustomerRepo, LockoutGuard, MembershipRepo, MfaRepo, ModelPriceRepo, OrgRepo,
    ProjectRepo, PromptTemplateRepo, ProviderDeletion, ProviderGroupRepo, ProviderKeyRepo,
    ProviderRepo, RateLimitRepo, RouteRepo, RouteTargetRepo, SessionRepo, SkillRepo, TeamRepo,
    UserRepo, VirtualKeyRepo,
};

use crate::access_control::caller_policy;
use crate::rbac::{
    authorize, authorize_superadmin, policy_allows, reaches_org, Principal, ScopeChain, ScopeFilter,
};
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
        .route("/api/v1/orgs/{org_id}/projects", get(list_org_projects))
        .route("/api/v1/projects/{id}", delete(delete_project))
        .route(
            "/api/v1/projects/{id}/settings",
            get(get_project_settings).put(update_project_settings),
        )
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
        .route("/api/v1/providers/{id}/models", get(list_provider_models))
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
        .route(
            "/api/v1/budgets/{id}",
            patch(update_budget).delete(delete_budget),
        )
        .route(
            "/api/v1/rate-limits",
            get(list_rate_limits).post(create_rate_limit),
        )
        .route(
            "/api/v1/rate-limits/{id}",
            patch(update_rate_limit).delete(delete_rate_limit),
        )
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
        .route("/api/v1/audit-log", get(list_deployment_audit_log))
}

pub(crate) fn pool(state: &ControlState) -> &PgPool {
    state
        .pool
        .as_ref()
        .expect("crud router is only mounted when a postgres pool is configured")
}

#[derive(Debug)]
pub(crate) enum ApiError {
    /// An error from the store, the core or a dependency. A `4xx` renders its
    /// own message, which is validation written for the caller. A `500` renders
    /// only [`INTERNAL_ERROR`] and logs the rest: `Error::Store` in particular
    /// carries raw driver text from every `e.to_string()` call site, which can
    /// name hosts, ports, schemas or query fragments (#2268).
    Core(Error),
    /// A server-side failure (500) whose message was written for the caller on
    /// purpose, such as a store that is not configured or a write it refused.
    /// Rendered verbatim, so it must never carry anything a driver said.
    Curated(String),
    /// mutation collides with a config-file-owned resource (409)
    Conflict(String),
    /// a 409 a client can branch on: `code` is part of the API and never
    /// renamed, `message` is for the person reading it
    CodedConflict { code: &'static str, message: String },
    /// a 400 for one field of the request, rendered with the stable code
    /// [`INVALID_FIELD`] and the field's path, so a client can translate the
    /// refusal and point at the input it came from (#2567)
    InvalidField { field: String, message: String },
    /// missing or invalid credentials (401)
    Unauthenticated,
    /// authenticated but lacking the required role at the scope (403)
    Forbidden,
    /// a 403 that is about policy rather than the caller's role, with a `code`
    /// a client can branch on. Same contract as [`ApiError::CodedConflict`]:
    /// `code` is part of the API and never renamed
    CodedForbidden { code: &'static str, message: String },
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

/// What a `500` says when its cause is not one of the [`ApiError::Curated`]
/// messages. The cause itself goes to the log, not the response.
pub(crate) const INTERNAL_ERROR: &str = "internal server error";

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // read before the match consumes `self`
        let retry_after = match &self {
            Self::TooManyAttempts(remaining) => Some(*remaining),
            _ => None,
        };
        let code = match &self {
            Self::CodedConflict { code, .. } | Self::CodedForbidden { code, .. } => Some(*code),
            Self::InvalidField { .. } => Some(INVALID_FIELD),
            Self::Core(Error::AlreadyExists(_)) => Some(NAME_TAKEN),
            // the same code the sign-in answers a lock with, so a client reads
            // one name for it wherever a password is being guessed
            Self::TooManyAttempts(_) => Some("too_many_attempts"),
            _ => None,
        };
        let field = match &self {
            Self::InvalidField { field, .. } => Some(field.clone()),
            _ => None,
        };
        let (status, message) = match self {
            Self::Core(err) => match &err {
                Error::NotFound(_) => (StatusCode::NOT_FOUND, err.to_string()),
                Error::Config(_) | Error::Unauthorized => {
                    (StatusCode::BAD_REQUEST, err.to_string())
                }
                // the store's text names the constraint, not the field; the
                // caller only needs to know the value is taken
                Error::AlreadyExists(_) => {
                    tracing::debug!(error = %err, "control-plane write hit a unique constraint");
                    (StatusCode::CONFLICT, ALREADY_EXISTS_MESSAGE.to_string())
                }
                _ => {
                    tracing::error!(error = %err, "control-plane request failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        INTERNAL_ERROR.to_string(),
                    )
                }
            },
            Self::Curated(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
            Self::InvalidField { message, .. } => (StatusCode::BAD_REQUEST, message),
            Self::Conflict(message) | Self::CodedConflict { message, .. } => {
                (StatusCode::CONFLICT, message)
            }
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "missing or invalid credentials".to_string(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "insufficient role for this resource".to_string(),
            ),
            Self::CodedForbidden { message, .. } => (StatusCode::FORBIDDEN, message),
            Self::TooManyAttempts(_) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many rejected attempts; try again later".to_string(),
            ),
        };
        let mut error = serde_json::json!({"message": message});
        if let Some(code) = code {
            error["code"] = code.into();
        }
        if let Some(field) = field {
            error["field"] = field.into();
        }
        let mut response = (status, Json(serde_json::json!({ "error": error }))).into_response();
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

/// stable code of a 400 refusing one field of the request (#2567). The body
/// also carries `field`, the field's path (`name`, `metadata.team`, `tags[2]`)
pub(crate) const INVALID_FIELD: &str = "invalid_field";

/// stable code of the 409 for a name, slug, email or key that another record
/// already holds (#2567). Raised by the explicit checks and, as the backstop for
/// a concurrent write or an unchecked constraint, by every unique violation the
/// store reports
pub(crate) const NAME_TAKEN: &str = "name_taken";

/// stable code of the 409 for a delete refused because other records still
/// depend on the one being removed (#2567)
pub(crate) const REFERENCED: &str = "referenced";

/// stable code of the 409 for a write that would use a resource from outside
/// the scope it is confined to, such as a project-scoped provider from another
/// project (#2567)
pub(crate) const SCOPE_MISMATCH: &str = "scope_mismatch";

/// what a unique violation from the store says. the driver's own text names a
/// constraint and a table, which is schema detail the caller has no use for
const ALREADY_EXISTS_MESSAGE: &str =
    "a record with that name or identifier already exists; choose another";

/// the 400 for one field of the request, with the stable [`INVALID_FIELD`] code
pub(crate) fn invalid_field(field: impl Into<String>, message: impl Into<String>) -> ApiError {
    ApiError::InvalidField {
        field: field.into(),
        message: message.into(),
    }
}

/// a 409 with the stable [`NAME_TAKEN`] code
pub(crate) fn name_taken(message: impl Into<String>) -> ApiError {
    ApiError::CodedConflict {
        code: NAME_TAKEN,
        message: message.into(),
    }
}

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
                return Err(invalid_field(
                    field,
                    format!(
                        "{field} must not contain control characters (found U+{:04X})",
                        bad as u32
                    ),
                ));
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

/// Deserialize a present-but-null field as `Some(None)` rather than `None`.
///
/// serde collapses both "absent" and "null" to `None` for an `Option<Option<T>>`,
/// which for a PATCH silently turns "clear this override" into "leave it
/// alone". The function only runs when the key is present, so wrapping in
/// `Some` here is what makes the two distinguishable. Pair it with
/// `#[serde(default)]` so an absent key still reads as `None`.
pub(crate) fn explicit_null<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// Reject a required field that's empty after trimming.
pub(crate) fn require_non_empty(value: &str, field: &str) -> ApiResult<()> {
    if value.trim().is_empty() {
        return Err(invalid_field(field, format!("{field} must not be empty")));
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
    let (filter, limit) = audit_log_filter(&query)?;
    let repo = AuditLogRepo(pool(&state));
    let page = repo.list_page(org_id, &filter, limit).await?;
    let total = if query.include_total {
        Some(repo.count(org_id, &filter).await?)
    } else {
        None
    };
    Ok(Json(audit_log_response(page, &filter, total)))
}

/// Every audit row in the deployment, org-less account events included. Those
/// are the ones no org read returns: a superadmin's own sign-ins, attempts
/// against an unregistered address and the events of someone removed from
/// every org (#1858).
// a security-auditor role (#1834) would be admitted here alongside superadmin
async fn list_deployment_audit_log(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<AuditLogQuery>,
) -> ApiResult<Json<AuditLogPageResponse>> {
    authorize_superadmin(&principal, superadmin_cap!("deployment_audit_log", Read))?;
    let (filter, limit) = audit_log_filter(&query)?;
    let repo = AuditLogRepo(pool(&state));
    let page = repo.list_page_all(&filter, limit).await?;
    let total = if query.include_total {
        Some(repo.count_all(&filter).await?)
    } else {
        None
    };
    Ok(Json(audit_log_response(page, &filter, total)))
}

/// Parse the audit-log query string into a store filter and a clamped page
/// size; shared by the per-org and the deployment-wide read.
fn audit_log_filter(query: &AuditLogQuery) -> ApiResult<(AuditLogFilter, i64)> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let filter = AuditLogFilter {
        actor_user_id: query.actor,
        action: normalized_filter(query.action.clone(), "action")?,
        target_type: normalized_filter(query.target_type.clone(), "target_type")?,
        start_at: query.start_at,
        end_at: query.end_at,
        cursor: query
            .cursor
            .as_deref()
            .map(parse_audit_cursor)
            .transpose()?,
        direction: query.direction.unwrap_or_default().into(),
    };
    if filter
        .start_at
        .is_some_and(|start| filter.end_at.is_some_and(|end| start > end))
    {
        return Err(ApiError::Core(Error::Config(
            "start_at must be before or equal to end_at".to_string(),
        )));
    }
    Ok((filter, limit))
}

fn audit_log_response(
    page: AuditLogPage,
    filter: &AuditLogFilter,
    total: Option<i64>,
) -> AuditLogPageResponse {
    let previous = matches!(filter.direction, AuditLogDirection::Previous);
    let has_next = if previous {
        !page.entries.is_empty()
    } else {
        page.has_more
    };
    let has_previous = if previous {
        page.has_more
    } else {
        filter.cursor.is_some()
    };
    let next_cursor = has_next
        .then(|| page.entries.last().map(encode_audit_cursor))
        .flatten();
    let previous_cursor = has_previous
        .then(|| page.entries.first().map(encode_audit_cursor))
        .flatten();
    AuditLogPageResponse {
        items: page.entries,
        next_cursor,
        previous_cursor,
        has_next,
        has_previous,
        total,
    }
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
/// Every org for a superadmin; otherwise the orgs the caller holds a role
/// anywhere inside: at the org, one of its teams or one of its projects. A
/// plain signed-in account must not learn the names of tenants it does not
/// belong to, and the dashboard defaults to the first org it is given (#1846).
async fn list_orgs(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Org>>> {
    let orgs = OrgRepo(pool(&state)).list().await?;
    let filter = ScopeFilter::load(&state, &principal, cap!("org", Read)).await?;
    if filter.is_superadmin() {
        return Ok(Json(orgs));
    }
    let reach = filter.reach(pool(&state)).await?;
    Ok(Json(
        orgs.into_iter()
            .filter(|org| reaches_org(&reach, org.id))
            .collect(),
    ))
}

/// Each project's team within `org_id`, so a listing can build every row's
/// scope chain without a query per row.
async fn project_teams(state: &ControlState, org_id: Uuid) -> ApiResult<HashMap<Uuid, Uuid>> {
    Ok(ProjectRepo(pool(state))
        .list_for_org(org_id)
        .await?
        .into_iter()
        .map(|project| (project.id, project.team_id))
        .collect())
}

/// The chain of a row that lives at an org, a team or a project of `org_id`.
fn row_chain(
    org_id: Uuid,
    team: Option<Uuid>,
    project: Option<Uuid>,
    teams_of: &HashMap<Uuid, Uuid>,
) -> ScopeChain {
    ScopeChain {
        org: Some(org_id),
        team: team.or_else(|| project.and_then(|project| teams_of.get(&project).copied())),
        project,
    }
}

/// Refuse a caller who holds no role anywhere inside `org_id`; a caller who
/// holds one below the org gets the rows they may read, which can be none.
async fn require_reach(state: &ControlState, filter: &ScopeFilter, org_id: Uuid) -> ApiResult<()> {
    if reaches_org(&filter.reach(pool(state)).await?, org_id) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
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
) -> ApiResult<Json<Vec<BusinessUnitListing>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("business_unit", Read),
    )
    .await?;
    // the live key count rides along so a zero-spend card can say whether any
    // key is attributed to the unit at all (#2581)
    Ok(Json(
        BusinessUnitRepo(pool(&state))
            .list_with_key_counts(org_id)
            .await?,
    ))
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
) -> ApiResult<Json<Vec<CustomerListing>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("customer", Read),
    )
    .await?;
    // see list_business_units for why the count is part of the listing
    Ok(Json(
        CustomerRepo(pool(&state))
            .list_with_key_counts(org_id)
            .await?,
    ))
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

/// Refuse version content `PromptTemplatesConfig::validate` would reject.
///
/// The same check the snapshot's config validation runs, so a version the
/// gateway could never be served is refused here instead of stored (#2279).
/// The placeholder id only has to be non-empty; the problems name the content.
fn check_prompt_template_version(
    id: Uuid,
    variables: &serde_json::Value,
    decorators: &serde_json::Value,
) -> ApiResult<()> {
    let malformed = |what: &str, e: serde_json::Error| {
        ApiError::Core(Error::Config(format!("{what} are malformed: {e}")))
    };
    let candidate = rolter_core::prompt_templates::PromptTemplate {
        id: id.to_string(),
        version: 1,
        routes: Vec::new(),
        scopes: Vec::new(),
        variables: serde_json::from_value(variables.clone())
            .map_err(|e| malformed("variables", e))?,
        decorators: serde_json::from_value(decorators.clone())
            .map_err(|e| malformed("decorators", e))?,
    };
    let problems =
        rolter_core::prompt_templates::PromptTemplatesConfig::template_problems(&candidate);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Core(Error::Config(format!(
            "invalid prompt template version: {}",
            problems.join("; ")
        ))))
    }
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
    check_prompt_template_version(id, &body.variables, &body.decorators)?;
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
    // a version stored before create-time validation may still be malformed;
    // publishing it would only get it pruned from the snapshot
    if let Some(stored) = repo
        .list_versions(id)
        .await?
        .into_iter()
        .find(|v| v.version == version)
    {
        check_prompt_template_version(id, &stored.variables, &stored.decorators)?;
    }
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
    let teams = TeamRepo(pool(&state)).list(org_id).await?;
    let filter = ScopeFilter::load(&state, &principal, cap!("team", Read)).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(teams));
    }
    // below the org: the teams the caller holds a role in, or holds one inside,
    // so a project member can still navigate to their project (#1846)
    let reach = filter.reach(pool(&state)).await?;
    let visible: Vec<Team> = teams
        .into_iter()
        .filter(|team| reach.iter().any(|chain| chain.team == Some(team.id)))
        .collect();
    if visible.is_empty() {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(visible))
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

/// Refuse deleting a project, or a team holding it, while a provider or group
/// is scoped to it (#1919). The database refuses too, with a foreign-key error;
/// this says what to move first.
async fn require_no_scoped_resources(
    state: &ControlState,
    project_ids: &[Uuid],
    what: &str,
) -> ApiResult<()> {
    if project_ids.is_empty() {
        return Ok(());
    }
    let held = ProjectRepo(pool(state))
        .scoped_resources(project_ids)
        .await?;
    if held.is_empty() {
        return Ok(());
    }
    Err(ApiError::CodedConflict {
        code: REFERENCED,
        message: format!(
            "this {what} still owns {}; delete them or make them org-wide first, since \
             deleting the project would otherwise widen their access or destroy them",
            held.join(", ")
        ),
    })
}

async fn delete_team(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let chain = ScopeChain::from_team(pool(&state), id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("team", Delete)).await?;
    let project_ids: Vec<Uuid> = ProjectRepo(pool(&state))
        .list(id)
        .await?
        .into_iter()
        .map(|project| project.id)
        .collect();
    require_no_scoped_resources(&state, &project_ids, "team").await?;
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
    let projects = ProjectRepo(pool(&state)).list(team_id).await?;
    let filter = ScopeFilter::load(&state, &principal, cap!("project", Read)).await?;
    if filter.allows(chain) {
        return Ok(Json(projects));
    }
    // below the team: the projects the caller holds a role in (#1846)
    let visible: Vec<Project> = projects
        .into_iter()
        .filter(|project| {
            filter.allows(ScopeChain {
                project: Some(project.id),
                ..chain
            })
        })
        .collect();
    if visible.is_empty() {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(visible))
}

/// `GET /api/v1/orgs/{org_id}/projects` — every project in the org.
///
/// The per-team route above answers one team at a time, so a caller that needs
/// to name any project in the org — an SCIM or SSO group mapping, which may
/// grant anywhere inside its own org — had to fan out one request per team
/// (#1357). Each row carries its `team_id` and the team's name so the caller
/// can still group by team without that fan-out.
///
/// Guarded at org scope with the same `project:read` capability the per-team
/// route takes: the answer spans every team, so the authority has to be the
/// org's rather than any one team's.
async fn list_org_projects(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<OrgProject>>> {
    let projects = ProjectRepo(pool(&state)).list_for_org(org_id).await?;
    let filter = ScopeFilter::load(&state, &principal, cap!("project", Read)).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(projects));
    }
    // below the org: the projects the caller reads through a team or project
    // role (#1846)
    let visible: Vec<OrgProject> = projects
        .into_iter()
        .filter(|project| {
            filter.allows(row_chain(
                org_id,
                Some(project.team_id),
                Some(project.id),
                &HashMap::new(),
            ))
        })
        .collect();
    if visible.is_empty() {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(visible))
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
    require_no_scoped_resources(&state, &[id], "project").await?;
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

/// A project's own settings (#1820).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectSettings {
    /// the lowest built-in role that may read the request and response bodies
    /// payload capture stored for this project's traffic: `member` (the
    /// default) or `viewer`. An admin always may
    payload_min_role: String,
}

/// The roles `payload_min_role` may take. `admin` is not one: an admin always
/// reads what a member reads, so it would change nothing.
const PAYLOAD_MIN_ROLES: [&str; 2] = ["member", "viewer"];

/// `GET /api/v1/projects/{id}/settings`.
async fn get_project_settings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProjectSettings>> {
    let chain = ScopeChain::from_project(pool(&state), id).await?;
    authorize(&state, &principal, chain, cap!("project_settings", Read)).await?;
    Ok(Json(ProjectSettings {
        payload_min_role: ProjectRepo(pool(&state)).payload_min_role(id).await?,
    }))
}

/// `PUT /api/v1/projects/{id}/settings` — a project admin's.
///
/// Lowering `payload_min_role` to `viewer` lets every viewer of the project
/// read its captured prompts and completions in the request log, so the change
/// is audited with the value it was set to.
async fn update_project_settings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<ProjectSettings>,
) -> ApiResult<Json<ProjectSettings>> {
    let chain = ScopeChain::from_project(pool(&state), id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("project_settings", Update)).await?;
    let role = body.payload_min_role.trim();
    if !PAYLOAD_MIN_ROLES.contains(&role) {
        return Err(ApiError::Core(Error::Config(format!(
            "payload_min_role must be one of {}",
            PAYLOAD_MIN_ROLES.join(", ")
        ))));
    }
    ProjectRepo(pool(&state))
        .set_payload_min_role(id, role)
        .await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "project.settings.update",
        "project",
        id,
        serde_json::json!({"payload_min_role": role}),
    )
    .await;
    Ok(Json(ProjectSettings {
        payload_min_role: role.to_string(),
    }))
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

/// The `credential` a test reports when `api_key_env` names a variable this
/// process does not have.
///
/// The control plane resolves the variable from its own environment, while the
/// gateway resolves it from the gateway's, so this is not proof that the
/// gateway lacks it, only that the test ran without a key (#2811).
const ENV_UNSET: &str = "env (unset)";

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
    api_key_env: Option<&str>,
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
                    401 | 403 => match (credential, api_key_env) {
                        (ENV_UNSET, Some(var)) => format!(
                            "{status}: the upstream rejected the request, and the key's \
                             environment variable {var} is not set in the control plane's \
                             environment (resolved from: {credential}). This test runs in the \
                             control plane, so set {var} there; the gateway reads it from its \
                             own environment, so set it there too."
                        ),
                        _ => format!(
                            "{status}: the upstream rejected the credential (resolved from: \
                             {credential})"
                        ),
                    },
                    404 => format!("{status}: reached the host, but {url} is not served there"),
                    other => format!("{other}: the upstream refused the probe"),
                }
            }
        }),
    }
}

/// How a stored provider's credential is resolved for a probe, and the secret
/// itself when there is one.
///
/// The same precedence the snapshot uses: a sealed key wins over the env var.
/// The second half names where the credential came from, so a refused probe is
/// distinguishable from a missing key without reading the provider row.
async fn resolve_probe_credential(
    state: &ControlState,
    id: Uuid,
    api_key_env: Option<&str>,
) -> ApiResult<(Option<String>, &'static str)> {
    let sealed: Option<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("select ciphertext, nonce from provider_keys where provider_id=$1")
            .bind(id)
            .fetch_optional(pool(state))
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
    Ok(match sealed {
        Some((ciphertext, nonce)) => match Kek::from_env() {
            Some(kek) => match kek.decrypt(&ciphertext, &nonce) {
                Ok(plaintext) => (Some(plaintext), "stored"),
                // the operator needs to know this is a KEK problem, not a
                // provider problem — the upstream is never even contacted
                Err(_) => (None, "stored (undecryptable)"),
            },
            None => (None, "stored (KEK unset)"),
        },
        None => match api_key_env.map(std::env::var) {
            Some(Ok(value)) => (Some(value), "env"),
            Some(Err(_)) => (None, ENV_UNSET),
            None => (None, "none"),
        },
    })
}

/// The model ids a stored provider's upstream lists.
#[derive(Serialize)]
struct ProviderModelList {
    /// sorted and without duplicates; empty when the upstream is down, answered
    /// with something other than a catalogue, or has no catalogue to read
    models: Vec<String>,
}

/// Most ids one listing returns. An aggregator can list thousands, and the
/// caller offers these as suggestions, not as the catalogue.
const MAX_LISTED_MODELS: usize = 2000;

/// Largest probe body read for a listing, the cap the gateway's own catalogue
/// read holds, so an upstream answering with something enormous costs a bounded
/// amount of memory.
const MAX_LISTING_BYTES: usize = 1 << 20;

/// Sorted, de-duplicated and capped ids, the shape a listing is returned in.
fn tidy_model_ids(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids.dedup();
    ids.truncate(MAX_LISTED_MODELS);
    ids
}

/// Ask a stored provider's upstream which models it serves.
///
/// The connection test counts the same catalogue and drops the ids, so the
/// route sheet had nothing to suggest for a target's upstream model and the id
/// had to be retyped to the letter (#2810). The ids are what the gateway lists
/// under `provider-slug/model` (`rolter_core::probe::catalogue_ids`).
///
/// Gated like the test, not like the provider listing: it sends the provider's
/// own credential to the upstream, authenticated the way the test does and
/// through the same egress policy, so a role that may not spend the credential
/// on a test may not spend it here either. That is `provider:update`, in the
/// provider's own scope. A provider the caller cannot see at all is answered as
/// one that does not exist. A failure at the upstream is an empty list — the
/// list only ever suggests, and "Test connection" is the call that says why a
/// provider does not answer.
async fn list_provider_models(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProviderModelList>> {
    let provider = ProviderRepo(pool(&state)).get(id).await?;
    let org_id = provider.org_id;
    // the listing's own rule: an org-wide provider is visible to anyone with a
    // role in the org, a project-scoped one to those holding a role there. one
    // that is not visible is answered as one that does not exist
    let visible = visible_in_scope(
        &state,
        &principal,
        cap!("provider", Read),
        org_id,
        vec![provider.clone()],
        |row| row.project_id,
    )
    .await?;
    if visible.is_empty() {
        return Err(Error::NotFound(format!("provider {id}")).into());
    }
    // seeing it is not enough: the credential is spent on the call. a provider
    // scoped to a project is its project admin's to change, an org-wide one the
    // org admin's, as for an edit of the row (#1919)
    let chain = match provider.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(org_id),
    };
    authorize(&state, &principal, chain, cap!("provider", Update)).await?;

    // the base was checked when it was stored, but the egress policy may have
    // been tightened since; a stored row is not a standing permission to egress
    require_allowed_egress(&state, &provider.api_base, "api_base")?;
    let kind: rolter_core::ProviderKind =
        serde_json::from_value(serde_json::Value::String(provider.kind.clone()))
            .map_err(|_| ApiError::Curated(format!("unknown provider kind '{}'", provider.kind)))?;
    let (secret, credential) =
        resolve_probe_credential(&state, id, provider.api_key_env.as_deref()).await?;
    if credential == "stored (undecryptable)" || credential == "stored (KEK unset)" {
        return Ok(Json(ProviderModelList { models: Vec::new() }));
    }

    let (url, headers) = rolter_core::probe_request(kind, &provider.api_base, "/");
    let models =
        match send_provider_probe(&state.egress, kind, &url, headers, secret.as_deref()).await {
            Ok(resp) if resp.status().is_success() => read_listing(resp).await,
            _ => Vec::new(),
        };
    Ok(Json(ProviderModelList {
        models: tidy_model_ids(models),
    }))
}

/// The model ids in a probe response, chunk by chunk against
/// [`MAX_LISTING_BYTES`]; none when it is not a catalogue or is too large.
async fn read_listing(mut resp: reqwest::Response) -> Vec<String> {
    let mut body: Vec<u8> = Vec::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if body.len() + chunk.len() > MAX_LISTING_BYTES {
            return Vec::new();
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|parsed| rolter_core::probe::catalogue_ids(&parsed))
        .unwrap_or_default()
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
            .map_err(|_| ApiError::Curated(format!("unknown provider kind '{kind}'")))?;

    let (secret, credential) = resolve_probe_credential(&state, id, api_key_env.as_deref()).await?;

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
    let started = std::time::Instant::now();
    let outcome =
        send_provider_probe(&state.egress, parsed_kind, &url, headers, secret.as_deref()).await;
    let latency_ms = started.elapsed().as_millis() as u64;

    let result = match outcome {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.json::<serde_json::Value>().await.ok();
            let verdict = judge_probe(
                parsed_kind,
                status,
                body.as_ref(),
                &url,
                credential,
                api_key_env.as_deref(),
            );
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

/// Send the probe request through the connect-time egress client.
///
/// Not `ControlState::http`: that client classifies only IP literals and
/// follows redirects, so a name that resolves to link-local, or an upstream
/// answering `302 Location: http://169.254.169.254/`, would reach a denied
/// address. The probe has never used the provider's `egress_proxy`, and still
/// does not, so there is no proxy path to keep.
///
/// The credential goes out the way `kind`'s API reads it (`x-api-key` for
/// Anthropic, `api-key` for Azure, bearer for most), through the same function
/// the proxy authenticates with. A bearer token to an API that ignores it reads
/// as a rejected key, and every Anthropic provider failed its first test that
/// way (#2806).
async fn send_provider_probe(
    egress: &std::sync::Arc<rolter_core::EgressPolicy>,
    kind: rolter_core::ProviderKind,
    url: &str,
    headers: impl IntoIterator<Item = (String, String)>,
    secret: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    let client = crate::egress_client::builder(egress)
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut req = client.get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(secret) = secret {
        let (name, value) = kind.auth_header(secret);
        req = req.header(name, value.as_ref());
    }
    req.send().await
}

/// A provider as the API returns it: the row plus whether a sealed key is
/// stored for it.
///
/// The flag is derived from `provider_keys` and says nothing else about the
/// credential; the key, its ciphertext and its nonce never leave the store.
#[derive(Serialize)]
struct ProviderView {
    #[serde(flatten)]
    provider: Provider,
    /// a row exists in `provider_keys`; an `api_key_env` is not a stored key
    has_stored_key: bool,
}

async fn list_providers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ProviderView>>> {
    let rows = ProviderRepo(pool(&state)).list(org_id).await?;
    let visible = visible_in_scope(
        &state,
        &principal,
        cap!("provider", Read),
        org_id,
        rows,
        |row| row.project_id,
    )
    .await?;
    // after the scope filter so only rows the caller may see are looked up
    let ids: Vec<Uuid> = visible.iter().map(|row| row.id).collect();
    let stored = ProviderKeyRepo(pool(&state)).stored_among(&ids).await?;
    let views = visible
        .into_iter()
        .map(|provider| ProviderView {
            has_stored_key: stored.contains(&provider.id),
            provider,
        })
        .collect();
    Ok(Json(views))
}

/// The rows of an org listing the caller may see, where a row may be scoped to
/// one project (#1919).
///
/// Holding the read role at the org shows every row. Below it, a caller who
/// holds a role somewhere in the org sees the org-wide rows, which every
/// project may use, plus the rows scoped to a project they hold the role in;
/// a caller with no role in the org at all is refused, as the listing always
/// did.
async fn visible_in_scope<T>(
    state: &ControlState,
    principal: &Principal,
    requirement: Requirement,
    org_id: Uuid,
    rows: Vec<T>,
    project_of: impl Fn(&T) -> Option<Uuid>,
) -> ApiResult<Vec<T>> {
    let filter = ScopeFilter::load(state, principal, requirement).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(rows);
    }
    let reach = filter.reach(pool(state)).await?;
    if !reaches_org(&reach, org_id) {
        return Err(ApiError::Forbidden);
    }
    let mut chains: HashMap<Uuid, ScopeChain> = HashMap::new();
    let mut visible = Vec::new();
    for row in rows {
        let Some(project) = project_of(&row) else {
            visible.push(row);
            continue;
        };
        let chain = match chains.get(&project) {
            Some(chain) => *chain,
            None => {
                let chain = ScopeChain::from_project(pool(state), project).await?;
                chains.insert(project, chain);
                chain
            }
        };
        if filter.allows(chain) {
            visible.push(row);
        }
    }
    Ok(visible)
}

/// The project a provider or group is being scoped to, checked against the org
/// it lives in: a project of another org is refused, and so is an unknown one,
/// with the same message so neither confirms what exists elsewhere (#1919).
async fn require_scope_project(
    state: &ControlState,
    org_id: Uuid,
    project_id: Uuid,
) -> ApiResult<()> {
    if ProjectRepo(pool(state)).in_org(project_id, org_id).await? {
        return Ok(());
    }
    Err(invalid_field(
        "project_id",
        "project_id must name a project of this organization",
    ))
}

/// Refuse a route or group owned by `owner` (a route's project, a group's
/// scope, `None` for an org-wide group) using a provider scoped to a different
/// project, since the provider would then be reachable from outside its
/// project through it (#1919).
async fn require_providers_usable_from(
    state: &ControlState,
    provider_ids: &[Uuid],
    owner: Option<Uuid>,
    what: &str,
) -> ApiResult<()> {
    let outside = ProviderRepo(pool(state))
        .scoped_outside(provider_ids, owner)
        .await?;
    if outside.is_empty() {
        return Ok(());
    }
    let owner_note = if owner.is_some() {
        "a different project"
    } else {
        "a project, and an org-wide group would expose it to every project"
    };
    Err(ApiError::CodedConflict {
        code: SCOPE_MISMATCH,
        message: format!(
            "{what} cannot use provider{} {}: scoped to {owner_note}; use org-wide providers \
             or ones scoped to the same project",
            if outside.len() == 1 { "" } else { "s" },
            outside
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
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
    /// scope the provider to one project of the org: only keys minted in it may
    /// reach the provider, through a route or `provider-slug/model`. Omit for an
    /// org-wide provider, usable by every project
    #[serde(default)]
    project_id: Option<Uuid>,
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
            // only the control plane holds the KEK: it unseals provider keys
            // when it builds the snapshot, and gateways receive them over the
            // token-guarded /internal/snapshot, so naming the gateway here
            // would send operators to copy the KEK where it is not needed
            "storing provider keys requires the {KEK_ENV} environment variable on the \
             control plane to seal them at rest"
        ))));
    };
    Ok(kek.encrypt(api_key)?)
}

#[cfg(test)]
mod provider_model_listing_tests {
    use super::*;

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    /// A catalogue arrives in the upstream's order and may repeat an id; the
    /// route sheet offers them as suggestions, so each shows once and in a
    /// stable place (#2810).
    #[test]
    fn a_listing_is_sorted_and_without_duplicates() {
        let tidy = tidy_model_ids(ids(&["llama-3.1-8b", "gemma-2-9b", "llama-3.1-8b"]));
        assert_eq!(tidy, ids(&["gemma-2-9b", "llama-3.1-8b"]));
    }

    /// An aggregator lists thousands of models, and a suggestion list that long
    /// is not one: the cap is a bound on the response, not a ranking.
    #[test]
    fn a_listing_is_capped() {
        let many: Vec<String> = (0..MAX_LISTED_MODELS + 500)
            .map(|n| format!("model-{n:05}"))
            .collect();
        let tidy = tidy_model_ids(many);
        assert_eq!(tidy.len(), MAX_LISTED_MODELS);
        assert_eq!(tidy.first().map(String::as_str), Some("model-00000"));
    }

    #[test]
    fn an_empty_catalogue_is_an_empty_listing() {
        assert!(tidy_model_ids(Vec::new()).is_empty());
    }
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
            None,
        )
    }

    /// #2811: `401 … (resolved from: env (unset))` said a key was missing and
    /// not whose environment it was missing from. The test runs in the control
    /// plane, the gateway has its own environment, and the operator has to fix
    /// the right one.
    #[test]
    fn an_unset_env_var_names_the_process_it_is_missing_from() {
        let verdict = judge_probe(
            ProviderKind::Openai,
            401,
            None,
            "https://x.test/v1/models",
            ENV_UNSET,
            Some("OPENAI_API_KEY"),
        );
        let error = verdict.error.expect("a rejection explains itself");
        assert!(
            error.contains("OPENAI_API_KEY"),
            "the message names the variable"
        );
        assert!(
            error.contains("control plane's environment"),
            "the message says the control plane is missing it"
        );
        assert!(
            error.contains("gateway"),
            "the message says the gateway needs it too"
        );
        assert!(
            error.contains("resolved from: env (unset)"),
            "the message keeps naming the credential source"
        );
    }

    /// The extra explanation is for the one credential source it is true of.
    #[test]
    fn a_rejected_stored_or_set_key_keeps_the_plain_message() {
        for credential in ["stored", "env", "none"] {
            let verdict = judge_probe(
                ProviderKind::Openai,
                401,
                None,
                "https://x.test/v1/models",
                credential,
                Some("OPENAI_API_KEY"),
            );
            let error = verdict.error.expect("a rejection explains itself");
            assert!(
                error.contains("rejected the credential"),
                "the plain message is kept for {credential}"
            );
            assert!(
                !error.contains("control plane's environment"),
                "the process explanation is only for an unset variable"
            );
        }
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
        return Err(invalid_field(
            "slug",
            "slug must match ^[a-z0-9][a-z0-9-]{0,62}$ (lowercase alphanumerics and \
             hyphens, 1-63 chars, not starting with a hyphen); a name with no ascii \
             letters or digits needs an explicit slug",
        ));
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
) -> ApiResult<Json<ProviderView>> {
    // a project-scoped provider is authorised at its project, so a project admin
    // may create one for their own project. Naming an environment variable is
    // a read of the control plane's environment, which is an org admin's call
    // whatever the scope, so that half is checked at the org
    if let Some(project_id) = body.project_id {
        require_scope_project(&state, org_id, project_id).await?;
        let chain = ScopeChain::from_project(pool(&state), project_id).await?;
        authorize(&state, &principal, chain, cap!("provider", Create)).await?;
    }
    if body.project_id.is_none() || has_env_name(&body.api_key_env) {
        authorize(
            &state,
            &principal,
            ScopeChain::org(org_id),
            cap!("provider", Create),
        )
        .await?;
    }
    require_non_empty(&body.name, "name")?;
    require_not_config_owned(&state.config_owned.providers, &body.name, "provider")?;
    require_non_empty(&body.api_base, "api_base")?;
    require_allowed_egress(&state, &body.api_base, "api_base")?;
    validate_kind(&body.kind)?;
    require_wellformed_api_base(&body.kind, &body.api_base)?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let providers = ProviderRepo(pool(&state));
    if providers.name_in_use(&body.name).await? {
        return Err(taken_in_deployment("provider name", &body.name));
    }
    require_address_slug_free(&state, "provider slug", &slug, None).await?;
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
            body.project_id,
        )
        .await?;
    let has_stored_key = sealed.is_some();
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
        serde_json::json!({
            "name": row.name, "slug": row.slug, "kind": row.kind, "project_id": row.project_id
        }),
    )
    .await;
    Ok(Json(ProviderView {
        provider: row,
        has_stored_key,
    }))
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
    /// omit to leave the scope unchanged; a project id scopes the provider to
    /// that project; `null` makes it org-wide again
    #[serde(default, deserialize_with = "explicit_null")]
    project_id: Option<Option<Uuid>>,
}

/// Whether a request names an environment variable to read a credential from.
fn has_env_name(field: &Option<String>) -> bool {
    field.as_deref().is_some_and(|name| !name.trim().is_empty())
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
) -> ApiResult<Json<ProviderView>> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    let org_id = existing.org_id;
    // an org-wide provider is an org admin's, and so is any change of scope or
    // of the environment variable a credential is read from. Only edits inside
    // a provider already scoped to a project are the project admin's (#1919)
    match existing.project_id {
        Some(project_id) if body.project_id.is_none() && !has_env_name(&body.api_key_env) => {
            let chain = ScopeChain::from_project(pool(&state), project_id).await?;
            authorize(&state, &principal, chain, cap!("provider", Update)).await?;
        }
        _ => {
            authorize(
                &state,
                &principal,
                ScopeChain::org(existing.org_id),
                cap!("provider", Update),
            )
            .await?;
        }
    }
    require_not_config_owned(&state.config_owned.providers, &existing.name, "provider")?;
    if let Some(Some(project_id)) = body.project_id {
        require_scope_project(&state, org_id, project_id).await?;
    }
    if let Some(scope) = body.project_id {
        let dependents = ProviderRepo(pool(&state))
            .dependents_outside(id, scope)
            .await?;
        if !dependents.is_empty() {
            return Err(ApiError::CodedConflict {
                code: SCOPE_MISMATCH,
                message: format!(
                    "provider '{}' is used by {}, which belong to other projects; remove it \
                     from them before scoping it to one project",
                    existing.name,
                    dependents.join(", ")
                ),
            });
        }
    }
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
    if let Some(slug) = slug_change.as_deref() {
        require_address_slug_free(&state, "provider slug", slug, Some(id)).await?;
    }
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
            body.project_id,
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
    // read back after the write so an omitted `api_key` reports what is stored
    let has_stored_key = ProviderKeyRepo(pool(&state)).exists(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "provider.update",
        "provider",
        id,
        serde_json::json!({"slug": row.slug, "project_id": row.project_id}),
    )
    .await;
    Ok(Json(ProviderView {
        provider: row,
        has_stored_key,
    }))
}

async fn delete_provider(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let existing = ProviderRepo(pool(&state)).get(id).await?;
    let chain = match existing.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(existing.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider", Delete)).await?;
    if let ProviderDeletion::InUse(dependents) = ProviderRepo(pool(&state)).delete(id).await? {
        return Err(ApiError::Conflict(format!(
            "provider '{}' is used by {}; remove it from them before deleting it",
            existing.name,
            dependents.join(", ")
        )));
    }
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

/// The 409 for a name the gateway holds in one namespace across every org.
///
/// It says only that the name is taken, never where: an org may not learn
/// what another one calls its routes or providers from the refusal (#1845).
fn taken_in_deployment(what: &str, name: &str) -> ApiError {
    name_taken(format!(
        "{what} '{name}' is already in use in this deployment; choose another"
    ))
}

/// Refuse a provider or group slug that already answers an address.
///
/// Providers and provider groups share one slug namespace at the gateway
/// (`provider-slug/model`, `group-slug/model`), across every org and the
/// bootstrap file, and a second holder silently takes the address from the
/// first: a group whose slug a provider holds is dropped from routing
/// entirely. `except` is the row being renamed, so it does not collide with
/// itself.
async fn require_address_slug_free(
    state: &ControlState,
    what: &str,
    slug: &str,
    except: Option<Uuid>,
) -> ApiResult<()> {
    if state.config_owned.holds_slug(slug)
        || ProviderRepo(pool(state))
            .address_slug_in_use(slug, except)
            .await?
    {
        return Err(taken_in_deployment(what, slug));
    }
    Ok(())
}

/// Refuse a route name the gateway already answers for someone else.
///
/// A name is taken when another route has it, when it is the builtin
/// `fake-llm` every org can call, or when it has the `slug/model` shape and
/// the slug belongs to a provider or group of another org or of the bootstrap
/// file. The gateway already treats a route outside the caller's org as
/// absent, so such a route could only shadow the address for the operator's
/// own keys, which carry no org and see every row; refusing it keeps those
/// keys on the address they meant. A slug the route's own org holds is that
/// org's choice. A name with `/` is otherwise fine: `Qwen/Qwen2.5-7B` is not
/// a slug address, since a slug is lower-case. Neither refusal says who holds
/// the name (#1845).
///
/// Only the API refuses `fake-llm`. A tenant must not take the builtin away,
/// but an operator may replace it: a route of that name in the bootstrap file,
/// a `[[models.default]]` or a `rolter-seed --import` still shadows it.
async fn require_route_name_free(
    state: &ControlState,
    org_id: Option<Uuid>,
    model: &str,
) -> ApiResult<()> {
    if model == rolter_core::FAKE_LLM_MODEL {
        return Err(name_taken(format!(
            "'{model}' is the gateway's built-in model and cannot be a route name; choose another"
        )));
    }
    let routes = RouteRepo(pool(state));
    if routes.model_in_use(model).await? {
        return Err(taken_in_deployment("route name", model));
    }
    if let Some(slug) = rolter_core::slug::address_slug(model) {
        if state.config_owned.holds_slug(slug)
            || routes.name_takes_address_outside_org(model, org_id).await?
        {
            return Err(name_taken(format!(
                "route name '{model}' is a provider or provider group address in this \
                 deployment ('{slug}/…'); choose another"
            )));
        }
    }
    Ok(())
}

/// Refuse any provider that is not in `org_id`.
///
/// A route target or group member on another org's provider would spend that
/// org's credential (#1844). The refusal is the 404 an unknown id gets, so it
/// does not confirm that the provider exists somewhere else.
async fn require_providers_in_org(
    state: &ControlState,
    org_id: Uuid,
    provider_ids: &[Uuid],
) -> ApiResult<()> {
    let repo = ProviderRepo(pool(state));
    for &id in provider_ids {
        if repo.get(id).await?.org_id != org_id {
            return Err(ApiError::Core(Error::NotFound(format!("provider {id}"))));
        }
    }
    Ok(())
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
    let groups = ProviderGroupRepo(pool(&state)).list(org_id).await?;
    let groups = visible_in_scope(
        &state,
        &principal,
        cap!("provider_group", Read),
        org_id,
        groups,
        |group| group.project_id,
    )
    .await?;
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
    /// scope the group to one project of the org, as for a provider. A scoped
    /// group may hold that project's providers and org-wide ones; an org-wide
    /// group may hold only org-wide providers. Omit for an org-wide group
    #[serde(default)]
    project_id: Option<Uuid>,
}

async fn create_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    let chain = match body.project_id {
        Some(project_id) => {
            require_scope_project(&state, org_id, project_id).await?;
            ScopeChain::from_project(pool(&state), project_id).await?
        }
        None => ScopeChain::org(org_id),
    };
    authorize(&state, &principal, chain, cap!("provider_group", Create)).await?;
    require_non_empty(&body.name, "name")?;
    validate_strategy(&body.strategy)?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    // a readonly config group with this slug shadows any DB row — refuse early
    require_group_not_config_owned(&state, &slug)?;
    let repo = ProviderGroupRepo(pool(&state));
    require_address_slug_free(&state, "provider group slug", &slug, None).await?;
    let member_providers: Vec<Uuid> = body.members.iter().map(|m| m.provider_id).collect();
    require_providers_in_org(&state, org_id, &member_providers).await?;
    require_providers_usable_from(&state, &member_providers, body.project_id, "this group").await?;
    let group = repo
        .create(org_id, &body.name, &slug, &body.strategy, body.project_id)
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
        serde_json::json!({
            "name": group.name, "slug": group.slug, "strategy": group.strategy,
            "project_id": group.project_id
        }),
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
    /// omit to leave the scope unchanged; a project id scopes the group to that
    /// project; `null` makes it org-wide again
    #[serde(default, deserialize_with = "explicit_null")]
    project_id: Option<Option<Uuid>>,
}

async fn update_provider_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateProviderGroup>,
) -> ApiResult<Json<ProviderGroupView>> {
    let repo = ProviderGroupRepo(pool(&state));
    let existing = repo.get(id).await?;
    // changing the scope is an org admin's call; edits inside a scoped group
    // are its project admin's
    let chain = match existing.project_id {
        Some(project_id) if body.project_id.is_none() => {
            ScopeChain::from_project(pool(&state), project_id).await?
        }
        _ => ScopeChain::org(existing.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider_group", Update)).await?;
    require_group_not_config_owned(&state, &existing.slug)?;
    if let Some(Some(project_id)) = body.project_id {
        require_scope_project(&state, existing.org_id, project_id).await?;
    }
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    if let Some(strategy) = &body.strategy {
        validate_strategy(strategy)?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    if let Some(slug) = slug_change.as_deref() {
        require_address_slug_free(&state, "provider group slug", slug, Some(id)).await?;
    }
    if let Some(members) = &body.members {
        let member_providers: Vec<Uuid> = members.iter().map(|m| m.provider_id).collect();
        require_providers_in_org(&state, existing.org_id, &member_providers).await?;
    }
    // the rule is checked against the group as it will be: its new scope, or
    // the old one, over its new members, or the ones it already has
    if body.members.is_some() || body.project_id.is_some() {
        let scope = body.project_id.unwrap_or(existing.project_id);
        let member_providers: Vec<Uuid> = match &body.members {
            Some(members) => members.iter().map(|m| m.provider_id).collect(),
            None => repo
                .members(id)
                .await?
                .into_iter()
                .map(|m| m.provider_id)
                .collect(),
        };
        require_providers_usable_from(&state, &member_providers, scope, "this group").await?;
    }
    let group = repo
        .update(
            id,
            body.name.as_deref(),
            slug_change.as_deref(),
            body.strategy.as_deref(),
            body.project_id,
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
        serde_json::json!({"slug": group.slug, "project_id": group.project_id}),
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
    let chain = match existing.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(existing.org_id),
    };
    authorize(&state, &principal, chain, cap!("provider_group", Delete)).await?;
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
    require_route_name_free(&state, org_id, &body.model).await?;
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
// this read takes `route:read`, the same bar as every other attribute of the
// route it belongs to. #704 held it to the *mutation* bar instead, which made
// the policy the one part of a route a viewer could list but never see, so the
// dashboard's Complexity Router answered a reader with a permission error
// (#1666). It is configuration rather than a secret — it already travels to the
// gateway inside the route's `params` — and the PUT beside it still takes
// `route:update`, so a viewer reads it and writes nothing
async fn get_route_complexity(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize_route(&state, &principal, id, cap!("route", Read)).await?;
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
    SafeJson(value): SafeJson<serde_json::Value>,
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
    if let Some(org_id) = org_id {
        require_providers_in_org(&state, org_id, &[body.provider_id]).await?;
    }
    // a route may use its own project's providers and org-wide ones (#1919)
    let route = RouteRepo(pool(&state)).get(route_id).await?;
    require_providers_usable_from(
        &state,
        &[body.provider_id],
        Some(route.project_id),
        "this route",
    )
    .await?;
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
            // an operator minted this deliberately; `purpose` marks the keys
            // the dashboard mints for itself (#1640)
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
                return Err(invalid_field(
                    "customer_id",
                    "customer_id must belong to the same org",
                ));
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
            return Err(invalid_field(
                "customer_id",
                "customer_id belongs to a different business unit",
            ));
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

/// Authorize reading the caps set at one scope.
///
/// A cap applies to everything beneath the scope it is set on, so the caller
/// may read the rows at their own scope and at the team and org above it, which
/// is what throttles their keys (#2527). Holding the role at the scope itself
/// passes as always. Otherwise the scope must be the team or org of a place the
/// caller holds the role at; another project's rows, a sibling team's, a
/// customer's or a business unit's stay refused, and writes never come through
/// here.
async fn authorize_scope_read(
    state: &ControlState,
    principal: &Principal,
    scope: &ScopeQuery,
    chain: ScopeChain,
    requirement: Requirement,
) -> ApiResult<()> {
    match authorize(state, principal, chain, requirement).await {
        Err(ApiError::Forbidden) if matches!(scope.scope_type.as_str(), "org" | "team") => {}
        other => return other,
    }
    let filter = ScopeFilter::load(state, principal, requirement).await?;
    for held in filter.reach(pool(state)).await? {
        let above = match scope.scope_type.as_str() {
            "org" => held.org == Some(scope.scope_id),
            _ => held.team == Some(scope.scope_id),
        };
        if above && filter.allows(held) {
            return Ok(());
        }
    }
    Err(ApiError::Forbidden)
}

async fn list_budgets(
    principal: Principal,
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<Budget>>> {
    validate_scope(&scope.scope_type)?;
    let chain = ScopeChain::from_scope(pool(&state), &scope.scope_type, scope.scope_id).await?;
    authorize_scope_read(&state, &principal, &scope, chain, cap!("budget", Read)).await?;
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
    // the same checks an edit gets (#1903): a cap the gateway would read as
    // no cap, or as already spent, is refused here rather than stored
    validate_limit_usd(&body.limit_usd)?;
    validate_period(&body.period)?;
    validate_unpriced_policy(body.unpriced_policy.as_deref())?;
    let row = BudgetRepo(pool(&state))
        .create(
            &body.scope_type,
            body.scope_id,
            body.limit_usd.trim(),
            body.period.trim(),
            body.unpriced_policy.as_deref(),
        )
        .await?;
    publish_config_change(&state).await?;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBudget {
    /// omit to leave the cap as it is
    #[serde(default)]
    limit_usd: Option<String>,
    /// omit to leave the window as it is
    #[serde(default)]
    period: Option<String>,
    // doubly optional so a PATCH can tell "leave the override alone" (absent)
    // from "drop it and inherit the deployment setting again" (null)
    #[serde(default, deserialize_with = "explicit_null")]
    unpriced_policy: Option<Option<String>>,
}

/// The smallest `limit_usd` the `numeric(12,4)` column cannot hold.
///
/// The column tops out at 99999999.9999, and Postgres rounds a value to four
/// places before it checks the precision, so everything from 99999999.99995
/// up overflows. Parsing text to `f64` is monotonic, so refusing any value
/// that parses to this float or above refuses every one of those. The only
/// cost is a sliver just below the boundary that parses to the same float.
const LIMIT_USD_CEILING: f64 = 99_999_999.999_95;

/// Validate a budget cap, on create and on update.
///
/// Stricter than a bare `f64` parse: `NaN` parses as a float and is a valid
/// `numeric`, but the gateway reads it back as no cap at all, and a negative
/// cap refuses every request as already exhausted. A cap
/// too large for the column would pass a parse and then fail in the store as
/// a 500 carrying the database's own message, so it is refused here as a 400
/// naming the range. A cap of zero stays legal, since it is how a scope is
/// frozen without deleting anything.
fn validate_limit_usd(value: &str) -> ApiResult<()> {
    match value.trim().parse::<f64>() {
        // a range check refuses NaN and both infinities along with the rest
        Ok(limit) if (0.0..LIMIT_USD_CEILING).contains(&limit) => Ok(()),
        _ => Err(ApiError::Core(Error::Config(
            "limit_usd must be a finite number from 0 to 99999999.9999".into(),
        ))),
    }
}

/// Validate a budget period, on create and on update (#1902).
///
/// The column is free text, and the snapshot loader reads anything it does not
/// recognise as monthly, so a `7d` budget, which the dashboard itself used to
/// suggest, was enforced as a calendar-month cap without a word. Refusing the
/// value here, with every spelling the gateway does recognise, is what stops a
/// new row from being misread. Rows stored before this check are reported by
/// `GET /api/v1/config/problems` instead of being rewritten.
fn validate_period(value: &str) -> ApiResult<()> {
    if BudgetPeriod::parse(value).is_some() {
        return Ok(());
    }
    let accepted: Vec<&str> = BudgetPeriod::SPELLINGS
        .iter()
        .map(|(spelling, _)| *spelling)
        .collect();
    Err(ApiError::Core(Error::Config(format!(
        "period must be one of {accepted:?}; there are no rolling windows such as 7d"
    ))))
}

/// What an edit actually moved, field by field, for the audit row.
///
/// Read off the row before and the row the update returned rather than off
/// the request, so a field sent with the value it already had is not reported
/// as a change and `limit_usd` compares in the column's own `numeric(12,4)`
/// spelling instead of whatever the caller typed.
fn budget_changes(before: &Budget, after: &Budget) -> serde_json::Value {
    let mut changes = serde_json::Map::new();
    if before.limit_usd != after.limit_usd {
        changes.insert(
            "limit_usd".into(),
            serde_json::json!({"from": before.limit_usd, "to": after.limit_usd}),
        );
    }
    if before.period != after.period {
        changes.insert(
            "period".into(),
            serde_json::json!({"from": before.period, "to": after.period}),
        );
    }
    if before.unpriced_policy != after.unpriced_policy {
        changes.insert(
            "unpriced_policy".into(),
            serde_json::json!({"from": before.unpriced_policy, "to": after.unpriced_policy}),
        );
    }
    serde_json::Value::Object(changes)
}

/// Change a budget in place (#1285).
///
/// Deleting and recreating a budget to change it left the scope with no cap
/// between the two calls, and each call bumped `config_version`, so a polling
/// gateway could load exactly that gap. One update is one statement and one
/// bump. The scope is not editable: a budget moved to another scope is a
/// different budget, and the authorization below is checked against the scope
/// it already has.
async fn update_budget(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateBudget>,
) -> ApiResult<Json<Budget>> {
    let existing = BudgetRepo(pool(&state)).get(id).await?;
    let chain =
        ScopeChain::from_scope(pool(&state), &existing.scope_type, existing.scope_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("budget", Update)).await?;
    if let Some(limit) = &body.limit_usd {
        validate_limit_usd(limit)?;
    }
    if let Some(period) = &body.period {
        validate_period(period)?;
    }
    validate_unpriced_policy(body.unpriced_policy.as_ref().and_then(|p| p.as_deref()))?;
    if body.limit_usd.is_none() && body.period.is_none() && body.unpriced_policy.is_none() {
        // an empty patch changes nothing, so it costs no write, no config bump
        // and no audit row
        return Ok(Json(existing));
    }
    let edit = BudgetRepo(pool(&state))
        .update(
            id,
            body.limit_usd.as_deref().map(str::trim),
            body.period.as_deref().map(str::trim),
            body.unpriced_policy.as_ref().map(|p| p.as_deref()),
        )
        .await?;
    if !edit.changed {
        // every field sent already held that value, so the store wrote
        // nothing and there is no bump to announce or edit to audit
        return Ok(Json(edit.after));
    }
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "budget.update",
        "budget",
        id,
        serde_json::json!({
            "scope_type": edit.after.scope_type,
            "scope_id": edit.after.scope_id,
            "changes": budget_changes(&edit.before, &edit.after),
        }),
    )
    .await;
    Ok(Json(edit.after))
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
    publish_config_change(&state).await?;
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
    authorize_scope_read(&state, &principal, &scope, chain, cap!("rate_limit", Read)).await?;
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
    // each cap sent has to limit something, and the store refuses a limit
    // left with none, as it does for an edit (#1903)
    validate_rate_limit_caps(body.rpm, body.tpm)?;
    let row = RateLimitRepo(pool(&state))
        .create(&body.scope_type, body.scope_id, body.rpm, body.tpm)
        .await?;
    publish_config_change(&state).await?;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRateLimit {
    // each cap is doubly optional: absent leaves it, null lifts it, a number
    // sets it. a plain Option would make "lift the rpm cap" unsayable
    #[serde(default, deserialize_with = "explicit_null")]
    rpm: Option<Option<i32>>,
    #[serde(default, deserialize_with = "explicit_null")]
    tpm: Option<Option<i32>>,
}

/// Check each cap a rate-limit create or edit sets.
///
/// `None` is a cap that is not being set: absent or null on a create, absent or
/// lifted on an edit. A cap that is set has to be at least 1, since the
/// snapshot loader reads zero and below as "no cap", so storing one would look
/// like a limit while admitting everything. Whether any cap is left at all is
/// the store's to check: on an edit that depends on the row as it stands when
/// the edit lands, so [`RateLimitRepo::update`] checks it under its row lock,
/// and [`RateLimitRepo::create`] applies the same rule to a new row.
fn validate_rate_limit_caps(rpm: Option<i32>, tpm: Option<i32>) -> ApiResult<()> {
    for (field, value) in [("rpm", rpm), ("tpm", tpm)] {
        if matches!(value, Some(cap) if cap < 1) {
            return Err(ApiError::Core(Error::Config(format!(
                "{field} must be at least 1, or null for no cap"
            ))));
        }
    }
    Ok(())
}

/// Change a rate limit's caps in place (#1285), for the reason
/// [`update_budget`] gives: delete-and-recreate opened a window with no limit
/// at all.
async fn update_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateRateLimit>,
) -> ApiResult<Json<RateLimit>> {
    let existing = RateLimitRepo(pool(&state)).get(id).await?;
    let chain =
        ScopeChain::from_scope(pool(&state), &existing.scope_type, existing.scope_id).await?;
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("rate_limit", Update)).await?;
    if body.rpm.is_none() && body.tpm.is_none() {
        // nothing asked for, so nothing written, bumped or audited
        return Ok(Json(existing));
    }
    validate_rate_limit_caps(body.rpm.flatten(), body.tpm.flatten())?;
    let edit = RateLimitRepo(pool(&state))
        .update(id, body.rpm, body.tpm)
        .await?;
    if !edit.changed {
        // both caps already read as sent: nothing written, bumped or audited
        return Ok(Json(edit.after));
    }
    publish_config_change(&state).await?;
    let (before, row) = (edit.before, edit.after);
    let mut changes = serde_json::Map::new();
    if before.rpm != row.rpm {
        changes.insert(
            "rpm".into(),
            serde_json::json!({"from": before.rpm, "to": row.rpm}),
        );
    }
    if before.tpm != row.tpm {
        changes.insert(
            "tpm".into(),
            serde_json::json!({"from": before.tpm, "to": row.tpm}),
        );
    }
    log_audit(
        &state,
        &principal,
        org_id,
        "rate_limit.update",
        "rate_limit",
        id,
        serde_json::json!({
            "scope_type": row.scope_type,
            "scope_id": row.scope_id,
            "changes": changes,
        }),
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
    publish_config_change(&state).await?;
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
        return Err(invalid_field(field, format!("{field} must be numeric")));
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

/// what a password shorter than [`MIN_PASSWORD_LEN`] is refused with, or `None`
/// when it is long enough. One rule for every path that sets a password, so the
/// admin reset, an invitation and the account's own change cannot disagree
pub(crate) fn password_length_problem(password: &str) -> Option<String> {
    (password.len() < MIN_PASSWORD_LEN)
        .then(|| format!("password must be at least {MIN_PASSWORD_LEN} characters"))
}

/// hash a plaintext password with argon2id for at-rest storage; the repo layer
/// only ever sees the digest
pub(crate) fn hash_password(password: &str) -> ApiResult<String> {
    use argon2::password_hash::PasswordHasher;
    use argon2::Argon2;

    if let Some(problem) = password_length_problem(password) {
        return Err(ApiError::Core(Error::Config(problem)));
    }
    // hash_password generates its own 16-byte salt from getrandom
    Argon2::default()
        .hash_password(password.as_bytes())
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
        return Err(invalid_field("email", "email must be a valid address"));
    }
    Ok(email.to_string())
}

pub(crate) fn validate_role(role: &str) -> ApiResult<()> {
    if !matches!(role, "admin" | "member" | "viewer") {
        return Err(invalid_field(
            "role",
            "role must be one of admin, member, viewer",
        ));
    }
    Ok(())
}

/// what `GET /api/v1/orgs/{org_id}/users` can be narrowed or widened by
#[derive(Deserialize, Default)]
struct ListUsersQuery {
    /// also list the accounts that hold no membership anywhere, such as the
    /// superadmin `rolter-seed --admin-email` creates. Only a superadmin can
    /// edit such an account, so only a superadmin is shown it: for anyone else
    /// the flag changes nothing, rather than failing a list the caller may
    /// read (#2804)
    #[serde(default)]
    include_unassigned: bool,
}

async fn list_users(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<ListUsersQuery>,
) -> ApiResult<Json<Vec<User>>> {
    let repo = UserRepo(pool(&state));
    let users = if query.include_unassigned && matches!(principal, Principal::Superadmin) {
        repo.list_in_org_with_unassigned(org_id).await?
    } else {
        repo.list_in_org(org_id).await?
    };
    let filter = ScopeFilter::load(&state, &principal, cap!("user", Read)).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(users));
    }
    // below the org: the people holding a role inside the teams and projects
    // the caller reads, so a team admin sees their own team (#1850)
    require_reach(&state, &filter, org_id).await?;
    let teams_of = project_teams(&state, org_id).await?;
    let visible: HashSet<Uuid> = MembershipRepo(pool(&state))
        .list_in_org(org_id)
        .await?
        .into_iter()
        .filter(|m| filter.allows(row_chain(org_id, m.team_id, m.project_id, &teams_of)))
        .map(|m| m.user_id)
        .collect();
    Ok(Json(
        users
            .into_iter()
            .filter(|user| visible.contains(&user.id))
            .collect(),
    ))
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
        return Err(name_taken(format!(
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

/// stable code of the 409 for a write that would leave the deployment with no
/// active superadmin account (#2344)
pub(crate) const LAST_SUPERADMIN: &str = "last_superadmin";

/// the refusal for a write that would leave no active superadmin. the admin
/// token is not an account, so it never counts as the one that remains
pub(crate) fn last_superadmin() -> ApiError {
    ApiError::CodedConflict {
        code: LAST_SUPERADMIN,
        message: "this is the last active superadmin; make another account superadmin first"
            .to_string(),
    }
}

/// stable code of the 409 for a write that would leave an org with no active
/// org-scoped admin (#2311)
pub(crate) const LAST_ORG_ADMIN: &str = "last_org_admin";

/// the refusal for revoking an org's last admin grant. a superadmin is exempt
pub(crate) fn last_org_admin() -> ApiError {
    ApiError::CodedConflict {
        code: LAST_ORG_ADMIN,
        message: "this is the organization's last admin grant; grant admin to another person \
                  first, or ask a superadmin"
            .to_string(),
    }
}

/// Revoke a grant an identity provider produced and no longer implies (SSO
/// group reconciliation on login, SCIM group sync), unless it is the org's last
/// active admin grant (#2558).
///
/// The revoke goes through `MembershipRepo::delete_guarded`, so the count and
/// the delete share one transaction under the org's admin lock. A refused
/// revoke is not an error: the sign-in or the sync still succeeds, the grant
/// stays, and a `membership.last_admin_kept` audit row plus a warning name the
/// org. Both callers derive the wanted set from the IdP every time, so the
/// next login or sync revokes the grant once the org has another admin.
/// Returns whether the grant was kept.
///
/// `protect` is false only when a superadmin's operator action triggered the
/// reconciliation (deleting a SCIM group mapping), matching
/// `delete_membership`, where a superadmin may revoke an org's last admin
/// grant. Login and IdP sync always pass true (#2673).
pub(crate) async fn revoke_idp_grant(
    state: &ControlState,
    stale: &Membership,
    protect: bool,
) -> ApiResult<bool> {
    if MembershipRepo(pool(state))
        .delete_guarded(stale.id, protect)
        .await?
        != LockoutGuard::WouldLockOut
    {
        return Ok(false);
    }
    tracing::warn!(
        org_id = ?stale.org_id,
        user_id = %stale.user_id,
        membership_id = %stale.id,
        source = %stale.source,
        "kept an identity-provider admin grant: revoking it would leave the org without an admin"
    );
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            stale.org_id,
            None,
            "membership.last_admin_kept",
            Some("membership"),
            Some(stale.id),
            Some(serde_json::json!({
                "user_id": stale.user_id,
                "role": stale.role,
                "source": stale.source,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write audit log entry");
    }
    Ok(true)
}

/// edit a global account. superadmin-only because it reaches across every org
/// the user belongs to and can grant the cross-org superadmin bit.
async fn update_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
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
                return Err(name_taken(format!(
                    "a user with email '{new_email}' already exists"
                )));
            }
        }
    }

    // one transaction under the superadmin lock, so the guard and the write
    // cannot be split by a concurrent demotion (#2344)
    let user = match UserRepo(pool)
        .update_account(
            id,
            email.as_deref(),
            password_hash.as_deref(),
            body.is_superadmin,
            body.deactivated,
        )
        .await?
    {
        LockoutGuard::Done(user) => user,
        LockoutGuard::WouldLockOut => return Err(last_superadmin()),
    };

    let mut sessions_revoked = 0;
    let mut detail = serde_json::json!({"email": user.email, "deactivated": body.deactivated});
    if let Some(deactivated) = body.deactivated {
        if deactivated {
            // cut existing access immediately, not just at token expiry
            sessions_revoked = SessionRepo(pool).delete_for_user_except(id, None).await?;
        }
        // the gateways stop, or resume, serving the keys the account minted
        // for itself (#1841), and the audit row says how many
        detail["personal_keys"] = VirtualKeyRepo(pool).count_personal(id).await?.into();
    }
    if password_hash.is_some() && body.deactivated != Some(true) {
        // a reset usually means the old password is in someone else's hands,
        // and whoever signed in with it would otherwise keep a session for its
        // full lifetime (#1936). The session sending this request survives, so
        // a superadmin resetting their own password is not signed out of the
        // page they did it from; it matches no other account's rows
        let own = crate::auth::bearer_token(&headers)
            .map(|token| rolter_auth::hash_key(&crate::auth::session_pepper(), token));
        sessions_revoked = SessionRepo(pool)
            .delete_for_user_except(id, own.as_deref())
            .await?;
    }
    if password_hash.is_some() || body.deactivated == Some(true) {
        // a sign-in halfway through its second-factor step was started with
        // the old password, and an enrolment token among them could still arm
        // a factor for whoever held that password (#1852)
        MfaRepo(pool).delete_challenges_for_user(id).await?;
    }

    // global account edit spans orgs, so it's logged unscoped
    detail["password_changed"] = password_hash.is_some().into();
    detail["sessions_revoked"] = sessions_revoked.into();
    log_audit(&state, &principal, None, "user.update", "user", id, detail).await;
    Ok(Json(user))
}

async fn delete_user(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("user", Delete))?;
    // memberships and sessions cascade on the users fk (on delete cascade), and
    // a trigger disables the keys the account minted for itself rather than
    // letting the fk orphan them into shared keys (#1841)
    let personal_keys = VirtualKeyRepo(pool(&state)).count_personal(id).await?;
    // read before the memberships go with the account: the deletion is written
    // once per org it belonged to, since afterwards nothing ties an org-less
    // row back to those orgs' audit logs (#1854)
    let orgs: HashSet<Uuid> = MembershipRepo(pool(&state))
        .ancestors_for_user(id)
        .await?
        .into_iter()
        .filter_map(|(_membership, org, _team)| org)
        .collect();
    if UserRepo(pool(&state)).delete(id).await? == LockoutGuard::WouldLockOut {
        return Err(last_superadmin());
    }
    let detail = serde_json::json!({"personal_keys": personal_keys});
    // an account that belonged to no org still gets its row, with none
    let scopes: Vec<Option<Uuid>> = if orgs.is_empty() {
        vec![None]
    } else {
        orgs.into_iter().map(Some).collect()
    };
    for org in scopes {
        log_audit(
            &state,
            &principal,
            org,
            "user.delete",
            "user",
            id,
            detail.clone(),
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list_memberships(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<Membership>>> {
    let memberships = MembershipRepo(pool(&state)).list_in_org(org_id).await?;
    let filter = ScopeFilter::load(&state, &principal, cap!("membership", Read)).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(memberships));
    }
    // below the org: the memberships inside the teams and projects the caller
    // reads, which is what a team admin needs to see and remove (#1850)
    require_reach(&state, &filter, org_id).await?;
    let teams_of = project_teams(&state, org_id).await?;
    Ok(Json(
        memberships
            .into_iter()
            .filter(|m| filter.allows(row_chain(org_id, m.team_id, m.project_id, &teams_of)))
            .collect(),
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
    // a superadmin can always repair an org, so only the other callers are
    // kept from leaving it without an admin (#2311)
    if MembershipRepo(pool)
        .delete_guarded(id, !matches!(principal, Principal::Superadmin))
        .await?
        == LockoutGuard::WouldLockOut
    {
        return Err(last_org_admin());
    }
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

    /// a 400 refusal, coded per field or not
    fn is_config_err(res: ApiResult<impl std::fmt::Debug>) -> bool {
        matches!(
            res,
            Err(ApiError::Core(Error::Config(_)) | ApiError::InvalidField { .. })
        )
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
            Err(ApiError::InvalidField { field, message }) => {
                // the field is the head of the message, so the two never disagree
                assert!(message.starts_with(&field), "{field}: {message}");
                message
            }
            other => panic!("expected an invalid_field error, got {other:?}"),
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

    /// a 400 refusal, coded per field or not
    fn is_config_err<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(
            res,
            Err(ApiError::Core(Error::Config(_)) | ApiError::InvalidField { .. })
        )
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
        use argon2::password_hash::PasswordVerifier;
        use argon2::Argon2;
        use argon2::PasswordHash;

        assert!(is_config_err(hash_password("short")));
        let hash = hash_password("longenough").unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default()
            .verify_password(b"longenough", &parsed)
            .is_ok());
    }

    #[test]
    fn the_length_rule_is_one_shared_function() {
        // the account's own change reports the same problem the admin reset
        // does, from the same place, so the two cannot drift
        // built from lengths, so no test carries a password as a literal
        let problem = password_length_problem(&"x".repeat(5)).expect("5 characters is too short");
        assert!(
            problem.contains("at least 8"),
            "the refusal states the minimum"
        );
        assert!(password_length_problem(&"y".repeat(MIN_PASSWORD_LEN)).is_none());
        assert!(password_length_problem(&"x".repeat(0)).is_some());
    }

    #[tokio::test]
    async fn a_throttled_password_endpoint_says_so_with_a_stable_code() {
        let response =
            ApiError::TooManyAttempts(std::time::Duration::from_millis(1500)).into_response();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        // rounded up, so a sub-second remainder never invites an instant retry
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("2")
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["code"], "too_many_attempts");
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

#[cfg(test)]
mod cap_edit_tests {
    use super::*;

    fn refused<T: std::fmt::Debug>(res: ApiResult<T>) -> bool {
        matches!(res, Err(ApiError::Core(Error::Config(_))))
    }

    #[test]
    fn a_limit_is_refused_outside_what_numeric_12_4_holds() {
        for ok in [
            "0",
            "0.0001",
            "500",
            " 750.5 ",
            "99999999.9999",
            "99999999.99994",
        ] {
            assert!(validate_limit_usd(ok).is_ok(), "{ok} should be accepted");
        }
        // 99999999.99995 rounds up to 100000000.0000, one digit too many for
        // the column, so it has to be refused with the rest of the overflow
        for bad in [
            "99999999.99995",
            "100000000",
            "1e9",
            "-1",
            "-0.0001",
            "NaN",
            "inf",
            "lots",
            "",
        ] {
            assert!(refused(validate_limit_usd(bad)), "{bad} should be refused");
        }
    }

    #[test]
    fn a_cap_a_patch_sets_has_to_be_positive() {
        let patch = |body: serde_json::Value| -> UpdateRateLimit {
            serde_json::from_value(body).expect("patch body")
        };
        let check = |patch: UpdateRateLimit| {
            validate_rate_limit_caps(patch.rpm.flatten(), patch.tpm.flatten())
        };
        for ok in [
            serde_json::json!({}),
            serde_json::json!({"rpm": 1}),
            serde_json::json!({"rpm": null, "tpm": 10}),
        ] {
            assert!(check(patch(ok.clone())).is_ok(), "{ok}");
        }
        for bad in [
            serde_json::json!({"rpm": 0}),
            serde_json::json!({"tpm": -5}),
            serde_json::json!({"rpm": 10, "tpm": 0}),
        ] {
            assert!(refused(check(patch(bad.clone()))), "{bad}");
        }
    }

    /// #1903: a create is held to the per-cap rule an edit already was. The
    /// "at least one cap" half is the store's, and is covered there.
    #[test]
    fn a_cap_a_create_sets_has_to_be_positive() {
        let body = |caps: serde_json::Value| -> CreateRateLimit {
            let mut body = serde_json::json!({
                "scope_type": "org",
                "scope_id": "00000000-0000-0000-0000-000000000001",
            });
            body.as_object_mut()
                .expect("object")
                .extend(caps.as_object().expect("object").clone());
            serde_json::from_value(body).expect("create body")
        };
        let check = |create: CreateRateLimit| validate_rate_limit_caps(create.rpm, create.tpm);
        for ok in [
            serde_json::json!({"rpm": 1}),
            serde_json::json!({"tpm": 1000}),
            serde_json::json!({"rpm": null, "tpm": 10}),
        ] {
            assert!(check(body(ok.clone())).is_ok(), "{ok}");
        }
        for bad in [
            serde_json::json!({"rpm": 0}),
            serde_json::json!({"tpm": -5}),
            serde_json::json!({"rpm": 60, "tpm": 0}),
        ] {
            assert!(refused(check(body(bad.clone()))), "{bad}");
        }
    }

    /// #1902: `7d` used to be stored and enforced as monthly. A period is
    /// accepted only when the gateway reads it as the window it names, and the
    /// refusal lists every spelling that would have been accepted.
    #[test]
    fn a_period_is_one_the_gateway_recognises() {
        for ok in [
            "daily", "1d", "24h", "monthly", "30d", "total", "lifetime", "all", " Daily ",
        ] {
            assert!(validate_period(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in ["7d", "weekly", "dialy", "", "  ", "1w", "month"] {
            match validate_period(bad) {
                Err(ApiError::Core(Error::Config(message))) => {
                    for (spelling, _) in BudgetPeriod::SPELLINGS {
                        assert!(
                            message.contains(spelling),
                            "{message} should name {spelling}"
                        );
                    }
                }
                other => panic!("{bad:?} should be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_explicit_null_is_told_apart_from_an_absent_cap() {
        let lifted: UpdateRateLimit =
            serde_json::from_value(serde_json::json!({"rpm": null})).expect("patch body");
        assert_eq!(lifted.rpm, Some(None), "null lifts the cap");
        assert_eq!(lifted.tpm, None, "absent leaves it alone");
    }
}

#[cfg(test)]
mod error_body_tests {
    use super::*;

    async fn rendered(err: ApiError) -> (StatusCode, serde_json::Value) {
        let response = err.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// Driver text as `store_err` and every `e.to_string()` call site pass it
    /// on: it names the host, the credentials in the url and the schema (#2268).
    fn driver_text(password: &str) -> String {
        format!(
            "error returned from database: relation \"tenant_a.users\" \
             does not exist (postgres://rolter:{password}@db.internal:5432/rolter)"
        )
    }

    #[tokio::test]
    async fn a_raw_server_error_never_reaches_a_500_body() {
        let password = format!("pw-{}", uuid::Uuid::new_v4());
        let driver_text = driver_text(&password);
        for err in [
            Error::Store(driver_text.clone()),
            Error::Upstream(driver_text.clone()),
            Error::Io(std::io::Error::other(driver_text.clone())),
        ] {
            let (status, body) = rendered(ApiError::Core(err)).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(body["error"]["message"], INTERNAL_ERROR);
            let text = body.to_string();
            for fragment in ["db.internal", "tenant_a"] {
                assert!(!text.contains(fragment), "{fragment} reached the body");
            }
            assert!(!text.contains(&password), "the password reached the body");
        }
    }

    #[tokio::test]
    async fn a_curated_message_still_reaches_a_500_body() {
        let (status, body) = rendered(ApiError::Curated(
            crate::ingest_failure::INSERT_FAILED.to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body["error"]["message"],
            crate::ingest_failure::INSERT_FAILED
        );
    }

    /// Validation and lookups are written for the caller, and stay as they are.
    #[tokio::test]
    async fn a_4xx_keeps_its_own_message() {
        let (status, body) = rendered(ApiError::Core(Error::Config(
            "slug must be lowercase".into(),
        )))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("slug must be lowercase"));
        let (status, body) = rendered(ApiError::Core(Error::NotFound("team 42".into()))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("team 42"));
    }
}

#[cfg(test)]
mod probe_egress_tests {
    use super::*;

    /// #2392: a name that resolves only to a denied address is never dialled.
    #[tokio::test]
    async fn a_provider_resolving_to_a_denied_address_is_never_contacted() {
        let listener = crate::egress_client::testing::Counter::start().await;
        let outcome = send_provider_probe(
            &crate::egress_client::testing::deny_loopback(),
            rolter_core::ProviderKind::Openai,
            &listener.url("/v1/models"),
            Vec::new(),
            None,
        )
        .await;
        assert!(outcome.is_err_and(|e| e.is_connect()));
        assert_eq!(listener.accepted(), 0);
    }

    /// a stub that answers a model list only to a request presenting `secret`
    /// in the header `header`, and 401 to everything else, as the real APIs do
    async fn serve_keyed_catalogue(header: &'static str, secret: String) -> String {
        let app = axum::Router::new().fallback(move |headers: axum::http::HeaderMap| {
            let secret = secret.clone();
            async move {
                let presented = headers.get(header).and_then(|v| v.to_str().ok());
                // anthropic rejects a version-less request even when the key is good
                let versioned = header != "x-api-key" || headers.contains_key("anthropic-version");
                if presented == Some(secret.as_str()) && versioned {
                    (
                        StatusCode::OK,
                        axum::Json(serde_json::json!({"data": [{"id": "m"}]})),
                    )
                } else {
                    (
                        StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({"error": "unauthorized"})),
                    )
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(async move { axum::serve(listener, app).await });
        base
    }

    /// #2806: Anthropic reads the key from `x-api-key` and ignores a bearer
    /// token, so a probe that sent the bearer form reported every valid key as
    /// rejected.
    #[tokio::test]
    async fn an_anthropic_probe_presents_its_key_as_x_api_key() {
        let secret = format!("sk-{}", uuid::Uuid::new_v4());
        let base = serve_keyed_catalogue("x-api-key", secret.clone()).await;
        let kind = rolter_core::ProviderKind::Anthropic;
        let (url, headers) = rolter_core::probe_request(kind, &base, "/");

        let response = send_provider_probe(&Default::default(), kind, &url, headers, Some(&secret))
            .await
            .expect("the upstream answers");
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// every kind presents its key in the header its own API reads: the stub
    /// only opens for the header the shared mapping names
    #[tokio::test]
    async fn every_kind_is_probed_with_the_credential_its_api_reads() {
        for kind in rolter_core::ProviderKind::ALL {
            let secret = format!("sk-{}", uuid::Uuid::new_v4());
            let (name, value) = kind.auth_header(&secret);
            let base = serve_keyed_catalogue(name, value.into_owned()).await;
            let (url, headers) = rolter_core::probe_request(kind, &base, "/");
            let response =
                send_provider_probe(&Default::default(), kind, &url, headers, Some(&secret))
                    .await
                    .expect("the upstream answers");
            // the stub opens for the exact header and value auth_header gives
            // this kind, so it is that header that has to be on the wire
            assert_eq!(response.status(), StatusCode::OK, "{kind:?}");
        }
    }

    /// #2392: an upstream that passes the check cannot bounce the probe on.
    #[tokio::test]
    async fn a_redirect_from_the_provider_is_not_followed() {
        let target = crate::egress_client::testing::Counter::start().await;
        let location = format!("http://127.0.0.1:{}/", target.port);
        let app = axum::Router::new().fallback(move || {
            let location = location.clone();
            async move {
                (
                    StatusCode::FOUND,
                    [(axum::http::header::LOCATION, location)],
                )
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let base = format!(
            "http://{}/v1/models",
            listener.local_addr().expect("an address")
        );
        tokio::spawn(async move { axum::serve(listener, app).await });

        let response = send_provider_probe(
            &Default::default(),
            rolter_core::ProviderKind::Openai,
            &base,
            Vec::new(),
            None,
        )
        .await
        .expect("the upstream answers");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(target.accepted(), 0);
    }
}
