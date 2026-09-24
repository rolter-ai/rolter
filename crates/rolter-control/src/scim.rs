//! SCIM 2.0 Users provisioning (#540).
//!
//! An IdP authenticates with an org-scoped bearer token minted through the
//! admin API, and drives `/scim/v2/Users` to create, update, deactivate and
//! reconcile accounts. Three properties shape the implementation:
//!
//! * **The token carries the tenant.** There is no org id in the SCIM path;
//!   the presented token resolves to exactly one org, so an IdP physically
//!   cannot address another tenant's users.
//! * **Idempotence.** IdPs retry. Creating an existing `userName` returns the
//!   SCIM `uniqueness` conflict rather than a second account; a replayed
//!   deactivate is a no-op with the same response.
//! * **No password path.** A `password` attribute is ignored rather than
//!   honoured: provisioning creates an SSO-shaped account with no local
//!   credential, and can never set or read one.
//!
//! Groups, group membership and group→team mapping live next door in
//! [`crate::scim_groups`]; this module is the Users half plus the token
//! lifecycle the provisioning screen needs.

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use rolter_store::postgres::models::{ScimIdentity, ScimToken, User};
use rolter_store::postgres::repo::{
    MembershipRepo, ScimIdentityRepo, ScimTokenRepo, SessionRepo, UserRepo, VirtualKeyRepo,
};

use crate::auth::session_pepper;
use crate::crud::{log_audit, pool, require_non_empty, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

const USER_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:User";
pub(crate) const LIST_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:ListResponse";
const ERROR_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:Error";
pub(crate) const PATCH_SCHEMA: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

/// Role a provisioned account is granted at the org merely for existing. Least
/// privilege on purpose: SCIM decides *who exists*, and anything beyond a read
/// comes from a group mapping an operator wrote (see [`crate::scim_groups`]).
const PROVISIONED_ROLE: &str = "viewer";

/// Largest page a SCIM client can ask for, so a `count=100000` cannot turn one
/// request into an unbounded scan.
pub(crate) const MAX_PAGE: usize = 200;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        // the IdP-facing surface, authenticated by a provisioning token
        .route("/scim/v2/Users", get(list_users).post(create_user))
        .route(
            "/scim/v2/Users/{id}",
            get(get_user)
                .put(replace_user)
                .patch(patch_user)
                .delete(delete_user),
        )
        // the operator-facing token lifecycle, authenticated as usual
        .route(
            "/api/v1/orgs/{org_id}/scim-tokens",
            post(create_token).get(list_tokens),
        )
        .route(
            "/api/v1/scim-tokens/{id}",
            axum::routing::delete(revoke_token),
        )
}

/// A SCIM error response. SCIM clients parse this envelope, so a plain
/// `ApiError` body would read as a transport failure to an IdP.
#[derive(Debug)]
pub(crate) struct ScimError {
    status: StatusCode,
    scim_type: Option<&'static str>,
    detail: String,
}

impl ScimError {
    pub(crate) fn new(
        status: StatusCode,
        scim_type: Option<&'static str>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            status,
            scim_type,
            detail: detail.into(),
        }
    }

    pub(crate) fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            None,
            "a valid SCIM provisioning token is required",
        )
    }

    pub(crate) fn not_found(id: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            None,
            format!("resource {id} not found"),
        )
    }

    pub(crate) fn invalid(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, Some("invalidValue"), detail)
    }
}

impl IntoResponse for ScimError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "schemas": [ERROR_SCHEMA],
            "status": self.status.as_u16().to_string(),
            "detail": self.detail,
        });
        if let Some(scim_type) = self.scim_type {
            body["scimType"] = json!(scim_type);
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<rolter_core::Error> for ScimError {
    fn from(err: rolter_core::Error) -> Self {
        match err {
            rolter_core::Error::NotFound(what) => {
                Self::new(StatusCode::NOT_FOUND, None, what.to_string())
            }
            other => Self::new(StatusCode::INTERNAL_SERVER_ERROR, None, other.to_string()),
        }
    }
}

pub(crate) type ScimResult<T> = std::result::Result<T, ScimError>;

/// The org a presented provisioning token belongs to. Extracting this is what
/// authenticates and scopes every SCIM handler.
pub(crate) struct ScimPrincipal {
    pub(crate) org_id: Uuid,
    pub(crate) token_id: Uuid,
}

impl FromRequestParts<ControlState> for ScimPrincipal {
    type Rejection = ScimError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &ControlState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let token =
            crate::auth::bearer_token(&parts.headers).ok_or_else(ScimError::unauthorized)?;
        let hash = rolter_auth::hash_key(&session_pepper(), token);
        let pool = state
            .pool
            .as_ref()
            .ok_or_else(|| ScimError::new(StatusCode::SERVICE_UNAVAILABLE, None, "no database"))?;
        let record = ScimTokenRepo(pool)
            .find_active_by_hash(&hash)
            .await
            .map_err(ScimError::from)?
            .ok_or_else(ScimError::unauthorized)?;
        // best-effort: the provisioning screen shows when an IdP last synced
        let _ = ScimTokenRepo(pool).touch(record.id).await;
        Ok(Self {
            org_id: record.org_id,
            token_id: record.id,
        })
    }
}

/// Render a user + identity as a SCIM Users resource. `active` mirrors the
/// account's deactivation flag, which is the only lifecycle state SCIM and the
/// dashboard both act on.
fn user_resource(user: &User, identity: &ScimIdentity) -> Value {
    let mut resource = json!({
        "schemas": [USER_SCHEMA],
        "id": user.id.to_string(),
        "userName": identity.user_name,
        "displayName": identity.display_name,
        "active": user.deactivated_at.is_none(),
        "emails": [{"value": user.email, "primary": true}],
        "meta": {
            "resourceType": "User",
            "created": user.created_at.to_rfc3339(),
            "lastModified": identity.updated_at.to_rfc3339(),
            "location": format!("/scim/v2/Users/{}", user.id),
        },
    });
    if let Some(external_id) = &identity.external_id {
        resource["externalId"] = json!(external_id);
    }
    resource
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListQuery {
    pub(crate) filter: Option<String>,
    #[serde(rename = "startIndex")]
    pub(crate) start_index: Option<usize>,
    pub(crate) count: Option<usize>,
}

/// Parse the one filter shape provisioning actually needs: `userName eq "x"`.
/// Anything else is rejected explicitly rather than silently ignored, which
/// would hand the IdP a full listing it would read as "no match".
fn parse_user_name_filter(filter: &str) -> ScimResult<String> {
    let trimmed = filter.trim();
    let rest = trimmed
        .strip_prefix("userName eq ")
        .or_else(|| trimmed.strip_prefix("username eq "))
        .or_else(|| trimmed.strip_prefix("userName Eq "))
        .ok_or_else(|| {
            ScimError::new(
                StatusCode::BAD_REQUEST,
                Some("invalidFilter"),
                "only `userName eq \"value\"` filters are supported",
            )
        })?;
    let value = rest.trim().trim_matches('"');
    if value.is_empty() {
        return Err(ScimError::new(
            StatusCode::BAD_REQUEST,
            Some("invalidFilter"),
            "filter value must not be empty",
        ));
    }
    Ok(value.to_string())
}

async fn list_users(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Query(query): Query<ListQuery>,
) -> ScimResult<Json<Value>> {
    let pool = pool(&state);
    let identities = match &query.filter {
        Some(filter) => {
            let user_name = parse_user_name_filter(filter)?;
            ScimIdentityRepo(pool)
                .find_by_user_name(principal.org_id, &user_name)
                .await?
                .into_iter()
                .collect()
        }
        None => ScimIdentityRepo(pool).list(principal.org_id).await?,
    };
    let total = identities.len();
    // SCIM paging is 1-based
    let start = query.start_index.unwrap_or(1).max(1) - 1;
    let count = query.count.unwrap_or(MAX_PAGE).min(MAX_PAGE);
    let mut resources = Vec::new();
    for identity in identities.into_iter().skip(start).take(count) {
        let user = UserRepo(pool).get(identity.user_id).await?;
        resources.push(user_resource(&user, &identity));
    }
    Ok(Json(json!({
        "schemas": [LIST_SCHEMA],
        "totalResults": total,
        "startIndex": start + 1,
        "itemsPerPage": resources.len(),
        "Resources": resources,
    })))
}

#[derive(Debug, Deserialize)]
struct ScimEmail {
    value: String,
    #[serde(default)]
    primary: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ScimUserBody {
    #[serde(rename = "userName")]
    user_name: Option<String>,
    #[serde(rename = "externalId")]
    external_id: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(default)]
    emails: Vec<ScimEmail>,
    active: Option<bool>,
    // `password` is intentionally absent: a provisioned account has no local
    // credential, and accepting the field would imply rolter stores one
}

impl ScimUserBody {
    /// The email to key the local account on: the primary address if the IdP
    /// marked one, else the first, else the `userName` when it is an address
    /// (Entra and Okta both send one).
    fn email(&self) -> Option<String> {
        self.emails
            .iter()
            .find(|e| e.primary.unwrap_or(false))
            .or_else(|| self.emails.first())
            .map(|e| e.value.clone())
            .or_else(|| {
                self.user_name
                    .as_ref()
                    .filter(|name| name.contains('@'))
                    .cloned()
            })
    }
}

async fn create_user(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Json(body): Json<ScimUserBody>,
) -> ScimResult<Response> {
    let pool = pool(&state);
    let user_name = body
        .user_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .ok_or_else(|| ScimError::invalid("userName is required"))?;
    let email = body
        .email()
        .ok_or_else(|| ScimError::invalid("an email address is required"))?;

    // an IdP replaying a create must not mint a second account
    if ScimIdentityRepo(pool)
        .find_by_user_name(principal.org_id, &user_name)
        .await?
        .is_some()
    {
        return Err(ScimError::new(
            StatusCode::CONFLICT,
            Some("uniqueness"),
            format!("userName {user_name} already exists"),
        ));
    }

    // an account may already exist locally (invited by an admin, or
    // provisioned by another org's IdP): adopt it rather than failing on the
    // unique email, so provisioning converges instead of wedging
    let user = match UserRepo(pool).find_by_email(&email).await? {
        Some(existing) => existing,
        None => UserRepo(pool).create(&email, None, false).await?,
    };
    let identity = ScimIdentityRepo(pool)
        .upsert(
            user.id,
            principal.org_id,
            body.external_id.as_deref(),
            &user_name,
            body.display_name.as_deref().unwrap_or_default(),
        )
        .await?;
    // give the account a least-privilege foothold in the org it was
    // provisioned into; nothing here can grant more than viewer
    ensure_membership(&state, principal.org_id, user.id).await?;
    if body.active == Some(false) {
        deactivate(&state, user.id, true).await?;
    }
    audit_scim(
        &state,
        &principal,
        "scim.user.create",
        "user",
        user.id,
        json!({"user_name": user_name}),
    )
    .await;
    let user = UserRepo(pool).get(user.id).await?;
    Ok((StatusCode::CREATED, Json(user_resource(&user, &identity))).into_response())
}

/// Grant the provisioned account its org membership if it has none there yet.
async fn ensure_membership(state: &ControlState, org_id: Uuid, user_id: Uuid) -> ScimResult<()> {
    let repo = MembershipRepo(pool(state));
    let existing = repo.list_for_user(user_id).await?;
    if existing.iter().any(|m| m.org_id == Some(org_id)) {
        return Ok(());
    }
    repo.create(user_id, Some(org_id), None, None, PROVISIONED_ROLE)
        .await?;
    Ok(())
}

/// Look up a SCIM resource id inside the token's org. A user outside the org —
/// or one the IdP never provisioned — is `404`, never another tenant's row.
async fn resolve(
    state: &ControlState,
    principal: &ScimPrincipal,
    id: &str,
) -> ScimResult<(User, ScimIdentity)> {
    let user_id: Uuid = id.parse().map_err(|_| ScimError::not_found(id))?;
    let pool = pool(state);
    let identity = ScimIdentityRepo(pool)
        .get(user_id, principal.org_id)
        .await?
        .ok_or_else(|| ScimError::not_found(id))?;
    let user = UserRepo(pool).get(user_id).await?;
    Ok((user, identity))
}

async fn get_user(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> ScimResult<Json<Value>> {
    let (user, identity) = resolve(&state, &principal, &id).await?;
    Ok(Json(user_resource(&user, &identity)))
}

async fn replace_user(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
    Json(body): Json<ScimUserBody>,
) -> ScimResult<Json<Value>> {
    let (user, identity) = resolve(&state, &principal, &id).await?;
    let pool = pool(&state);
    let user_name = body.user_name.clone().unwrap_or(identity.user_name);
    let identity = ScimIdentityRepo(pool)
        .upsert(
            user.id,
            principal.org_id,
            body.external_id
                .as_deref()
                .or(identity.external_id.as_deref()),
            &user_name,
            body.display_name
                .as_deref()
                .unwrap_or(&identity.display_name),
        )
        .await?;
    let mut detail = json!({"user_name": identity.user_name});
    if let Some(active) = body.active {
        detail["personal_keys"] = deactivate(&state, user.id, !active).await?.into();
    }
    audit_scim(
        &state,
        &principal,
        "scim.user.update",
        "user",
        user.id,
        detail,
    )
    .await;
    let user = UserRepo(pool).get(user.id).await?;
    Ok(Json(user_resource(&user, &identity)))
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchOp {
    #[serde(default)]
    pub(crate) op: String,
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) value: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchBody {
    #[serde(default)]
    pub(crate) schemas: Vec<String>,
    #[serde(rename = "Operations", alias = "operations", default)]
    pub(crate) operations: Vec<PatchOp>,
}

/// The `active` toggle is what every IdP uses to deactivate a leaver, so it is
/// the operation this slice implements. Other paths are refused explicitly —
/// an IdP that gets `204 No Content` for an operation nothing applied would
/// believe a change landed.
async fn patch_user(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> ScimResult<Json<Value>> {
    if !body.schemas.is_empty() && !body.schemas.iter().any(|s| s == PATCH_SCHEMA) {
        return Err(ScimError::invalid(format!(
            "schemas must include {PATCH_SCHEMA}"
        )));
    }
    let (user, identity) = resolve(&state, &principal, &id).await?;
    let mut applied = false;
    let mut personal_keys = 0;
    for op in &body.operations {
        let verb = op.op.to_ascii_lowercase();
        if verb != "replace" && verb != "add" {
            return Err(ScimError::invalid(format!("unsupported op '{}'", op.op)));
        }
        let active = active_from_op(op)?;
        personal_keys = deactivate(&state, user.id, !active).await?;
        applied = true;
    }
    if !applied {
        return Err(ScimError::invalid("no supported operation in the request"));
    }
    audit_scim(
        &state,
        &principal,
        "scim.user.update",
        "user",
        user.id,
        json!({"user_name": identity.user_name, "personal_keys": personal_keys}),
    )
    .await;
    let user = UserRepo(pool(&state)).get(user.id).await?;
    Ok(Json(user_resource(&user, &identity)))
}

/// Read the `active` value out of a patch operation, in both the
/// `path: "active"` and the bare `{"active": false}` value forms IdPs send.
fn active_from_op(op: &PatchOp) -> ScimResult<bool> {
    let value = op
        .value
        .as_ref()
        .ok_or_else(|| ScimError::invalid("operation is missing a value"))?;
    let candidate = match op.path.as_deref().map(str::trim) {
        Some("active") => value.clone(),
        Some(other) => {
            return Err(ScimError::invalid(format!("unsupported path '{other}'")));
        }
        None => value.get("active").cloned().ok_or_else(|| {
            ScimError::invalid("only the `active` attribute can be patched today")
        })?,
    };
    match candidate {
        Value::Bool(active) => Ok(active),
        // some IdPs send the string form
        Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(true),
        Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(false),
        other => Err(ScimError::invalid(format!(
            "active must be a boolean, got {other}"
        ))),
    }
}

/// Deactivating drops the account's live sessions in the same step: an IdP
/// disabling a leaver expects them logged out, not merely unable to log in
/// again.
///
/// The gateways stop serving the keys the account minted for itself, and
/// serve them again on reactivation (#1841). Returns how many there are, for
/// the audit row.
async fn deactivate(state: &ControlState, user_id: Uuid, deactivated: bool) -> ScimResult<i64> {
    let pool = pool(state);
    UserRepo(pool).set_deactivated(user_id, deactivated).await?;
    if deactivated {
        SessionRepo(pool).delete_for_user(user_id).await?;
    }
    Ok(VirtualKeyRepo(pool).count_personal(user_id).await?)
}

/// SCIM `DELETE` deprovisions: the account is deactivated and its sessions
/// dropped, its group membership dropped with everything those groups granted,
/// and the org's identity mapping removed. The `users` row itself stays,
/// because its memberships and audit trail must outlive any one IdP — and a
/// re-provision through `POST` adopts it again.
async fn delete_user(
    principal: ScimPrincipal,
    State(state): State<ControlState>,
    Path(id): Path<String>,
) -> ScimResult<StatusCode> {
    let (user, identity) = resolve(&state, &principal, &id).await?;
    let personal_keys = deactivate(&state, user.id, true).await?;
    // before the identity goes: the group-granted roles must go with it, or a
    // deprovisioned account would keep admin on a team nobody can see it on
    crate::scim_groups::forget_user(&state, principal.org_id, user.id).await?;
    ScimIdentityRepo(pool(&state))
        .delete(user.id, principal.org_id)
        .await?;
    audit_scim(
        &state,
        &principal,
        "scim.user.deprovision",
        "user",
        user.id,
        json!({"user_name": identity.user_name, "personal_keys": personal_keys}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Record a provisioning action. The actor is the token, not a human, so the
/// audit row carries the token id rather than a user id.
pub(crate) async fn audit_scim(
    state: &ControlState,
    principal: &ScimPrincipal,
    action: &str,
    resource_type: &str,
    resource_id: Uuid,
    mut detail: Value,
) {
    detail["scim_token_id"] = json!(principal.token_id.to_string());
    if let Err(err) = rolter_store::postgres::repo::AuditLogRepo(pool(state))
        .create(
            Some(principal.org_id),
            None,
            action,
            Some(resource_type),
            Some(resource_id),
            Some(detail),
        )
        .await
    {
        tracing::warn!(error = %err, action, "failed to write scim audit log entry");
    }
}

// ---------------------------------------------------------------------------
// token lifecycle (operator-facing)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateScimToken {
    name: String,
}

/// A minted token: the only time the plaintext is ever returned.
#[derive(Debug, Serialize)]
struct CreatedScimToken {
    #[serde(flatten)]
    token: ScimToken,
    /// the bearer value the IdP is configured with; not stored and not
    /// recoverable — a lost token is rotated, not looked up
    secret: String,
}

async fn create_token(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateScimToken>,
) -> ApiResult<Json<CreatedScimToken>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("scim_token", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    let (secret, hash) = generate_scim_token(&session_pepper());
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    let token = ScimTokenRepo(pool(&state))
        .create(org_id, &body.name, &hash, actor)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "scim_token.create",
        "scim_token",
        token.id,
        json!({"name": token.name}),
    )
    .await;
    Ok(Json(CreatedScimToken { token, secret }))
}

async fn list_tokens(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ScimToken>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("scim_token", Read),
    )
    .await?;
    Ok(Json(ScimTokenRepo(pool(&state)).list(org_id).await?))
}

async fn revoke_token(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ScimToken>> {
    let repo = ScimTokenRepo(pool(&state));
    let token = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(token.org_id),
        cap!("scim_token", Delete),
    )
    .await?;
    let revoked = repo.revoke(id).await?;
    log_audit(
        &state,
        &principal,
        Some(token.org_id),
        "scim_token.revoke",
        "scim_token",
        id,
        json!({"name": token.name}),
    )
    .await;
    Ok(Json(revoked))
}

/// Mint an opaque provisioning token and its peppered digest. Same shape and
/// pepper as session tokens, so a database dump alone cannot be replayed.
fn generate_scim_token(pepper: &str) -> (String, String) {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token = format!("rolter_scim_{}", rolter_auth::hex::encode(&bytes));
    let hash = rolter_auth::hash_key(pepper, &token);
    (token, hash)
}

/// Reject a SCIM body the router cannot serve, as a SCIM error rather than an
/// axum rejection an IdP would read as a transport fault.
impl From<ApiError> for ScimError {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::Unauthenticated => Self::unauthorized(),
            ApiError::Forbidden => Self::new(StatusCode::FORBIDDEN, None, "forbidden"),
            ApiError::Core(err) => err.into(),
            other => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                None,
                format!("{other:?}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_user_name_filter_idps_send() {
        assert_eq!(
            parse_user_name_filter("userName eq \"ada@example.com\"").unwrap(),
            "ada@example.com"
        );
        // casing varies between IdPs
        assert_eq!(
            parse_user_name_filter("username eq \"ada\"").unwrap(),
            "ada"
        );
    }

    #[test]
    fn rejects_filters_it_cannot_honour() {
        // silently ignoring these would return the whole directory, which an
        // IdP reads as "no such user" and then re-creates
        for filter in [
            "displayName eq \"Ada\"",
            "userName sw \"a\"",
            "userName eq \"\"",
            "",
        ] {
            assert!(parse_user_name_filter(filter).is_err(), "accepted {filter}");
        }
    }

    #[test]
    fn reads_active_from_both_patch_shapes() {
        let with_path = PatchOp {
            op: "replace".into(),
            path: Some("active".into()),
            value: Some(json!(false)),
        };
        assert!(!active_from_op(&with_path).unwrap());

        let bare = PatchOp {
            op: "replace".into(),
            path: None,
            value: Some(json!({"active": true})),
        };
        assert!(active_from_op(&bare).unwrap());

        // the string form some IdPs send
        let stringly = PatchOp {
            op: "replace".into(),
            path: Some("active".into()),
            value: Some(json!("False")),
        };
        assert!(!active_from_op(&stringly).unwrap());
    }

    #[test]
    fn rejects_patch_operations_it_would_silently_drop() {
        for op in [
            PatchOp {
                op: "replace".into(),
                path: Some("displayName".into()),
                value: Some(json!("Ada")),
            },
            PatchOp {
                op: "replace".into(),
                path: None,
                value: Some(json!({"displayName": "Ada"})),
            },
            PatchOp {
                op: "replace".into(),
                path: Some("active".into()),
                value: None,
            },
        ] {
            assert!(active_from_op(&op).is_err());
        }
    }

    #[test]
    fn a_minted_token_is_opaque_and_hashed() {
        let (token, hash) = generate_scim_token("pepper");
        assert!(token.starts_with("rolter_scim_"));
        assert_ne!(token, hash);
        let (other, other_hash) = generate_scim_token("pepper");
        assert_ne!(token, other);
        assert_ne!(hash, other_hash);
    }

    #[test]
    fn primary_email_wins_then_first_then_user_name() {
        let body = ScimUserBody {
            user_name: Some("ada".into()),
            external_id: None,
            display_name: None,
            emails: vec![
                ScimEmail {
                    value: "work@example.com".into(),
                    primary: Some(false),
                },
                ScimEmail {
                    value: "primary@example.com".into(),
                    primary: Some(true),
                },
            ],
            active: None,
        };
        assert_eq!(body.email().as_deref(), Some("primary@example.com"));

        let no_primary = ScimUserBody {
            user_name: Some("ada".into()),
            external_id: None,
            display_name: None,
            emails: vec![ScimEmail {
                value: "only@example.com".into(),
                primary: None,
            }],
            active: None,
        };
        assert_eq!(no_primary.email().as_deref(), Some("only@example.com"));

        let from_user_name = ScimUserBody {
            user_name: Some("ada@example.com".into()),
            external_id: None,
            display_name: None,
            emails: vec![],
            active: None,
        };
        assert_eq!(from_user_name.email().as_deref(), Some("ada@example.com"));

        // a non-address userName with no emails cannot key a local account
        let unkeyed = ScimUserBody {
            user_name: Some("ada".into()),
            external_id: None,
            display_name: None,
            emails: vec![],
            active: None,
        };
        assert_eq!(unkeyed.email(), None);
    }
}
