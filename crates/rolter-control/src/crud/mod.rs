//! Control CRUD API: orgs, teams, projects, providers, routes/targets,
//! virtual keys, budgets, rate limits and the model pricing catalog.
//!
//! Thin Axum handlers over the `rolter_store::postgres::repo` repositories.
//! Only mounted when the control plane is started with `--database-url`
//! (see `main.rs`), since these routes need direct pool access beyond what
//! the [`rolter_store::ConfigStore`] trait exposes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use rolter_core::slug::{is_valid_slug, slugify};
use rolter_core::Error;
use rolter_store::postgres::repo::{AuditLogRepo, ProjectRepo, RouteRepo, VirtualKeyRepo};

use crate::rbac::{authorize, reaches_org, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::Requirement;
use crate::ControlState;

mod audit_log;
mod budgets;
mod business_units;
mod customers;
mod memberships;
mod models;
mod pricing;
mod prompts;
mod provider_groups;
mod providers;
mod rate_limits;
mod routes;
mod skills;
mod tenancy;
mod users;
mod virtual_keys;

// items the rest of the control plane imports as `crate::crud::*`
pub(crate) use budgets::{KeyBudgetBody, KeyCapsBody, KeyRateLimitBody};
pub(crate) use users::{
    hash_password, password_length_problem, revoke_idp_grant, validate_email, validate_role,
};
pub(crate) use virtual_keys::{generate_virtual_key, key_pepper};

pub fn router() -> Router<ControlState> {
    Router::new()
        .merge(audit_log::router())
        .merge(tenancy::router())
        .merge(business_units::router())
        .merge(customers::router())
        .merge(prompts::router())
        .merge(skills::router())
        .merge(providers::router())
        .merge(provider_groups::router())
        .merge(routes::router())
        .merge(virtual_keys::router())
        .merge(budgets::router())
        .merge(rate_limits::router())
        .merge(pricing::router())
        .merge(models::router())
        .merge(users::router())
        .merge(memberships::router())
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
    /// The deployment cannot seal a credential until its KEK is configured (503).
    KekUnavailable(String),
    /// A [`ApiError::Curated`] failure (500) that also carries a stable `code`
    /// a client can translate. `code` is part of the API and never renamed
    CodedInternal { code: &'static str, message: String },
    /// mutation collides with a config-file-owned resource (409)
    Conflict(String),
    /// a 409 a client can branch on: `code` is part of the API and never
    /// renamed, `message` is for the person reading it
    CodedConflict { code: &'static str, message: String },
    /// A service the control plane called for the caller (an authorization
    /// server, say) could not be reached or did not answer (502). Like
    /// [`ApiError::Curated`] the message is written for the caller and rendered
    /// as it stands, so it must never carry the upstream's own words or address;
    /// those go to the log. `code` is part of the API and never renamed.
    BadGateway { code: &'static str, message: String },
    /// a 400 for one field of the request, rendered with the stable code
    /// [`INVALID_FIELD`] and the field's path, so a client can translate the
    /// refusal and point at the input it came from (#2567)
    InvalidField { field: String, message: String },
    /// a 400 for a request body that is not the JSON the endpoint takes:
    /// malformed, the wrong shape or an unknown type, rendered with the stable
    /// code [`INVALID_BODY`]. The parser cannot tell which input was wrong
    /// reliably, so unlike [`ApiError::InvalidField`] it names no field
    InvalidBody(String),
    /// a 400 for a query string that does not deserialize into the list
    /// parameters the endpoint takes (`limit=abc`, a malformed id), rendered with
    /// the stable code [`INVALID_QUERY`]
    InvalidQuery(String),
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
            Self::CodedConflict { code, .. }
            | Self::CodedForbidden { code, .. }
            | Self::CodedInternal { code, .. }
            | Self::BadGateway { code, .. } => Some(*code),
            Self::KekUnavailable(_) => Some("kek_unavailable"),
            Self::InvalidField { .. } => Some(INVALID_FIELD),
            Self::InvalidBody(_) => Some(INVALID_BODY),
            Self::InvalidQuery(_) => Some(INVALID_QUERY),
            Self::Core(Error::AlreadyExists(_)) => Some(NAME_TAKEN),
            Self::Core(Error::StoreTimeout(_)) => Some(STORE_TIMEOUT),
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
                // the pool's lock_timeout / statement_timeout ran out. The
                // driver's text stays in the log; the caller is told what to do
                Error::StoreTimeout(_) => {
                    tracing::warn!(error = %err, "control-plane write timed out in the store");
                    (
                        StatusCode::GATEWAY_TIMEOUT,
                        STORE_TIMEOUT_MESSAGE.to_string(),
                    )
                }
                _ => {
                    tracing::error!(error = %err, "control-plane request failed");
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        INTERNAL_ERROR.to_string(),
                    )
                }
            },
            Self::Curated(message) | Self::CodedInternal { message, .. } => {
                (StatusCode::INTERNAL_SERVER_ERROR, message)
            }
            Self::KekUnavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message),
            Self::BadGateway { message, .. } => (StatusCode::BAD_GATEWAY, message),
            Self::InvalidField { message, .. }
            | Self::InvalidBody(message)
            | Self::InvalidQuery(message) => (StatusCode::BAD_REQUEST, message),
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

/// stable code of the 400 for a request body that does not parse: malformed
/// JSON, or JSON that is not the shape the endpoint takes (#2729)
pub(crate) const INVALID_BODY: &str = "invalid_body";

/// stable code of the 400 for a query string that does not deserialize into
/// the parameters the endpoint takes. The analytics extractor in
/// `time_bounds.rs` answers the same code in its own envelope
pub(crate) const INVALID_QUERY: &str = "invalid_query";

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

/// stable code of the `504` for a statement the database gave up on, or a lock
/// it stopped waiting for, because the pool's `lock_timeout` or
/// `statement_timeout` ran out (#1925)
pub(crate) const STORE_TIMEOUT: &str = "store_timeout";

/// what a store timeout says. The driver's own text names a lock or a
/// statement, which is detail the caller has no use for
pub(crate) const STORE_TIMEOUT_MESSAGE: &str =
    "the database did not finish this change in time; it may be \
     waiting on another change to the same record, so try again";

/// what a unique violation from the store says. the driver's own text names a
/// constraint and a table, which is schema detail the caller has no use for
const ALREADY_EXISTS_MESSAGE: &str =
    "a record with that name or identifier already exists; choose another";

/// axum's `Query` with a refusal in the control plane's error envelope: a
/// query string that does not deserialize is a coded 400 rather than axum's
/// plain-text rejection, which the dashboard could only show verbatim
pub(crate) struct Query<T>(pub(crate) T);

impl<S, T> axum::extract::FromRequestParts<S> for Query<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<T>::try_from_uri(&parts.uri)
            .map(|axum::extract::Query(query)| Self(query))
            .map_err(|rejection| ApiError::InvalidQuery(rejection.body_text()))
    }
}

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
            .map_err(|rejection| ApiError::InvalidBody(rejection.body_text()))?;
        reject_control_chars(&value, "")?;
        let parsed = serde_json::from_value(value)
            .map_err(|err| ApiError::InvalidBody(format!("invalid request body: {err}")))?;
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
        .map_err(|rejection| ApiError::InvalidBody(rejection.body_text()))?
        else {
            return Ok(None);
        };
        reject_control_chars(&value, "")?;
        let parsed = serde_json::from_value(value)
            .map_err(|err| ApiError::InvalidBody(format!("invalid request body: {err}")))?;
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
        .map_err(|problem| invalid_field(field, problem))
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
///
/// The actor is [`Principal::account_id`]: the signed-in account, a superadmin
/// session included, and nobody for the admin token or open mode (#2844).
pub(crate) async fn log_audit(
    state: &ControlState,
    principal: &Principal,
    org_id: Option<Uuid>,
    action: &str,
    target_type: &str,
    target_id: Uuid,
    detail: serde_json::Value,
) {
    let actor = principal.account_id();
    // a token has no account to name, so the row names the token instead
    // (#1836): an automation's actions are told apart from a person's, and
    // from another token's, by `detail.api_token`
    let detail = match principal.api_token() {
        Some(token) => {
            let mut detail = match detail {
                serde_json::Value::Object(map) => map,
                other => serde_json::Map::from_iter([("detail".to_string(), other)]),
            };
            detail.insert(
                "api_token".to_string(),
                serde_json::json!({"id": token.id, "name": token.name}),
            );
            serde_json::Value::Object(detail)
        }
        None => detail,
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

pub(crate) fn normalized_filter(value: Option<String>, field: &str) -> ApiResult<Option<String>> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() > 256 || value.chars().any(char::is_control) {
                return Err(invalid_field(
                    field,
                    format!("{field} must be 1-256 visible characters"),
                ));
            }
            Ok(value)
        })
        .transpose()
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

/// Reject a mutation of the stored row `name` of `org` when the file owns that
/// very row: the same org and name (#3516). A stored row is always an org's,
/// so an org's row that shares a name with a global file row is the org's own
/// and stays editable; creating one is still refused by
/// [`require_not_config_owned`], the write-time guard of ADR-0040 section 6.
fn require_row_not_config_owned(
    owned: &std::collections::HashSet<(String, String)>,
    org: uuid::Uuid,
    name: &str,
    kind: &str,
) -> ApiResult<()> {
    if owned.contains(&(org.to_string(), name.to_string())) {
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
pub(crate) async fn require_reach(
    state: &ControlState,
    filter: &ScopeFilter,
    org_id: Uuid,
) -> ApiResult<()> {
    if reaches_org(&filter.reach(pool(state)).await?, org_id) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
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

#[cfg(test)]
mod slug_tests {
    use super::provider_groups::{to_member_tuples, validate_strategy, GroupMemberInput};
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
    use super::provider_groups::CreateProviderGroup;
    use super::providers::CreateProvider;
    use super::routes::CreateRoute;
    use super::virtual_keys::CreateVirtualKey;
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
mod error_body_tests {
    use super::*;

    #[tokio::test]
    async fn missing_kek_is_a_coded_deployment_refusal() {
        let (status, body) =
            rendered(ApiError::KekUnavailable("configure ROLTER_KEK".into())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["code"], "kek_unavailable");
        assert_eq!(body["error"]["message"], "configure ROLTER_KEK");
        assert!(body["error"].get("field").is_none());
    }

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

    #[tokio::test]
    async fn password_hash_curated_error_renders_as_500() {
        let (status, body) = rendered(ApiError::Curated("failed to hash password".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["message"], "failed to hash password");
    }

    #[tokio::test]
    async fn an_unparsable_body_is_a_coded_400_that_names_no_field() {
        let (status, body) =
            rendered(ApiError::InvalidBody("invalid request body: x".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], INVALID_BODY);
        assert!(body["error"].get("field").is_none(), "{body}");
    }

    #[tokio::test]
    async fn a_unique_violation_is_a_409_name_taken() {
        let (status, body) = rendered(ApiError::Core(Error::AlreadyExists("x".into()))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], NAME_TAKEN);
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
