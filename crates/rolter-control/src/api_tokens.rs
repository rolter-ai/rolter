//! Scoped control-plane API tokens for automation (#1836).
//!
//! A CI job that rotates a project key, a pipeline that applies routes and a
//! provisioning script that creates a project per customer used to choose
//! between the admin token (superadmin, forever, shared by everything that
//! holds it) and a person's session. An API token is the third thing: a named
//! service account holding **one role at one org, team or project**, with an
//! expiry, which the [`Principal`] extractor resolves to
//! [`Principal::Token`]. Every `authorize` call then applies unchanged.
//!
//! What keeps it a credential and not a back door:
//!
//! * only a peppered sha-256 is stored, like sessions, virtual keys and SCIM
//!   tokens; the secret is returned once, by the mint or a rotation;
//! * it never reaches a deployment-wide capability: the guard refuses
//!   `Authority::Superadmin` and `Authority::Deployment` for a token, and the
//!   row cannot name either;
//! * it cannot mint, rotate, list or revoke tokens, so a leaked token cannot
//!   outlive its own revocation by minting a successor;
//! * minting takes an admin at the token's own scope, so a token never holds
//!   more than the person who made it;
//! * it expires (90 days unless the minter says otherwise or opts out), a
//!   revoked or expired token stops resolving on the next request, and a
//!   rotation replaces the secret in one statement;
//! * every mint, rotation and revocation is audited, and so is every other
//!   audited action it takes, with the token's id and name in the row's
//!   `detail.api_token` (see [`crate::crud::log_audit`]).
//!
//! It is distinct from a virtual key (the data plane, `/v1/*`) and from a SCIM
//! token (provisioning, `/scim/v2/*`): the control plane refuses each at the
//! other's door.

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use rolter_store::postgres::models::{ApiToken, EffectiveGrant, Membership};
use rolter_store::postgres::repo::{ApiTokenRepo, CustomRoleRepo, NewApiToken};
use sqlx::PgPool;

use crate::auth::session_pepper;
use crate::crud::{
    invalid_field, log_audit, pool, require_non_empty, require_reach, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, parse_role, Principal, ScopeChain, ScopeFilter, TokenPrincipal};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// What every API token starts with, so the extractor knows which table to ask
/// and a secret scanner can recognise one.
pub(crate) const TOKEN_PREFIX: &str = "rolter_api_";

/// how many characters of the value are kept to recognise a token in a list
const DISPLAY_PREFIX_LEN: usize = TOKEN_PREFIX.len() + 4;

/// expiry applied when the minter names none
const DEFAULT_EXPIRY_DAYS: u32 = 90;
/// the longest expiry a mint may ask for
const MAX_EXPIRY_DAYS: u32 = 730;

/// `409` code for rotating a token that is revoked or past its expiry
pub(crate) const API_TOKEN_INACTIVE: &str = "api_token_inactive";

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/api-tokens",
            get(list_tokens).post(create_token),
        )
        .route("/api/v1/api-tokens/{id}/rotate", post(rotate_token))
        .route("/api/v1/api-tokens/{id}", delete(revoke_token))
}

/// Resolve a presented token's digest to the authority it holds, or `None`
/// when no live token has it. Custom-role grants are read here, per request,
/// so editing or deleting the role takes effect on the token's next call.
pub(crate) async fn resolve(
    pool: &PgPool,
    token_hash: &str,
) -> ApiResult<Option<Box<TokenPrincipal>>> {
    let repo = ApiTokenRepo(pool);
    let Some(token) = repo.find_active_by_hash(token_hash).await? else {
        return Ok(None);
    };
    let held = token_authority(pool, token).await?;
    // best effort: a failed stamp must not fail the request it describes
    let _ = repo.touch(held.token.id).await;
    Ok(Some(Box::new(held)))
}

/// [`resolve`] by the token's row id: the principal a download ticket minted
/// with this token redeems as (#3504). A token revoked or expired since is
/// `None`, as it is for a bearer.
pub(crate) async fn resolve_id(pool: &PgPool, id: Uuid) -> ApiResult<Option<Box<TokenPrincipal>>> {
    let Some(token) = ApiTokenRepo(pool).find_active_by_id(id).await? else {
        return Ok(None);
    };
    Ok(Some(Box::new(token_authority(pool, token).await?)))
}

/// The membership or the grants a token stands for. A token's scope ids are
/// copied as stored, so a project token matches the chain its project names.
async fn token_authority(pool: &PgPool, token: ApiToken) -> ApiResult<TokenPrincipal> {
    let (memberships, grants) = match (&token.role, token.custom_role_id) {
        (Some(role), _) => (
            vec![Membership {
                id: token.id,
                user_id: token.id,
                org_id: Some(token.org_id),
                team_id: token.team_id,
                project_id: token.project_id,
                role: role.clone(),
                source: "api_token".into(),
                created_at: token.created_at,
            }],
            Vec::new(),
        ),
        (None, Some(role_id)) => {
            let repo = CustomRoleRepo(pool);
            let role = repo.get(role_id).await?;
            let rows = repo.list_grants_for_roles(&[role_id]).await?;
            let grant = |resource: Option<String>, action: Option<String>| EffectiveGrant {
                profile_id: Uuid::nil(),
                role_id,
                role_slug: role.slug.clone(),
                base_role: role.base_role.clone(),
                org_id: Some(token.org_id),
                team_id: token.team_id,
                project_id: token.project_id,
                resource,
                action,
            };
            // a role with no explicit grants still carries its base role
            let grants = if rows.is_empty() {
                vec![grant(None, None)]
            } else {
                rows.into_iter()
                    .map(|row| grant(Some(row.resource), Some(row.action)))
                    .collect()
            };
            (Vec::new(), grants)
        }
        // the table's check constraint forbids a row with neither; hold
        // nothing rather than guess
        (None, None) => (Vec::new(), Vec::new()),
    };
    Ok(TokenPrincipal {
        token,
        memberships,
        grants,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateApiToken {
    name: String,
    /// the team the token is scoped to; omit both ids for an org-wide token
    team_id: Option<Uuid>,
    /// the project the token is scoped to; its team may be named too
    project_id: Option<Uuid>,
    /// a built-in role: `admin`, `member` or `viewer`
    role: Option<String>,
    /// or an org-defined role of this org; exactly one of the two is given
    custom_role_id: Option<Uuid>,
    /// days until the token stops working, `1..=730`; 90 when omitted
    expires_in_days: Option<u32>,
    /// opt out of the expiry. Not combinable with `expires_in_days`
    #[serde(default)]
    never_expires: bool,
}

/// A minted or rotated token: the only time the secret is ever returned.
#[derive(Debug, Serialize)]
struct SecretApiToken {
    #[serde(flatten)]
    token: ApiToken,
    /// the bearer value; not stored and not recoverable, so a lost one is
    /// rotated, not looked up
    secret: String,
}

/// Tokens cannot manage tokens: a leaked one would otherwise mint a successor
/// and outlive its own revocation.
fn refuse_tokens(principal: &Principal) -> ApiResult<()> {
    match principal {
        Principal::Token(_) => Err(ApiError::Forbidden),
        Principal::Superadmin { .. } | Principal::User(_) => Ok(()),
    }
}

fn chain_of(token: &ApiToken) -> ScopeChain {
    ScopeChain {
        org: Some(token.org_id),
        team: token.team_id,
        project: token.project_id,
    }
}

/// The chain a mint names, checked against the org in the path: a team or
/// project of another org is refused, never silently re-homed.
async fn requested_chain(
    pool: &PgPool,
    org_id: Uuid,
    team_id: Option<Uuid>,
    project_id: Option<Uuid>,
) -> ApiResult<ScopeChain> {
    let chain = match (team_id, project_id) {
        (_, Some(project_id)) => {
            let chain = ScopeChain::from_project(pool, project_id).await?;
            if team_id.is_some_and(|team| Some(team) != chain.team) {
                return Err(invalid_field(
                    "team_id",
                    "the project does not belong to this team",
                ));
            }
            chain
        }
        (Some(team_id), None) => ScopeChain::from_team(pool, team_id).await?,
        (None, None) => ScopeChain::org(org_id),
    };
    if chain.org != Some(org_id) {
        return Err(invalid_field(
            "project_id",
            "the team or project is not in this organization",
        ));
    }
    Ok(chain)
}

/// Mint a secret and its peppered digest. The prefix is for display only.
fn generate_secret(pepper: &str) -> (String, String, String) {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let secret = format!("{TOKEN_PREFIX}{}", rolter_auth::hex::encode(&bytes));
    let hash = rolter_auth::hash_key(pepper, &secret);
    let prefix = secret[..DISPLAY_PREFIX_LEN].to_string();
    (secret, hash, prefix)
}

fn expiry(body: &CreateApiToken) -> ApiResult<Option<DateTime<Utc>>> {
    if body.never_expires {
        if body.expires_in_days.is_some() {
            return Err(invalid_field(
                "never_expires",
                "never_expires cannot be combined with expires_in_days",
            ));
        }
        return Ok(None);
    }
    let days = body.expires_in_days.unwrap_or(DEFAULT_EXPIRY_DAYS);
    if !(1..=MAX_EXPIRY_DAYS).contains(&days) {
        return Err(invalid_field(
            "expires_in_days",
            format!("expires_in_days must be between 1 and {MAX_EXPIRY_DAYS}"),
        ));
    }
    Ok(Some(Utc::now() + Duration::days(i64::from(days))))
}

fn audit_detail(token: &ApiToken) -> serde_json::Value {
    json!({
        "name": token.name,
        "role": token.role,
        "custom_role_id": token.custom_role_id,
        "team_id": token.team_id,
        "project_id": token.project_id,
        "expires_at": token.expires_at,
    })
}

async fn create_token(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateApiToken>,
) -> ApiResult<Json<SecretApiToken>> {
    refuse_tokens(&principal)?;
    require_non_empty(&body.name, "name")?;
    let name = body.name.trim();
    if name.chars().count() > 80 {
        return Err(invalid_field("name", "name must be at most 80 characters"));
    }
    let role = match (body.role.as_deref(), body.custom_role_id) {
        (Some(role), None) => {
            if parse_role(role).is_none() {
                return Err(invalid_field(
                    "role",
                    "role must be one of admin, member, viewer",
                ));
            }
            Some(role)
        }
        (None, Some(_)) => None,
        _ => {
            return Err(invalid_field(
                "role",
                "give exactly one of role and custom_role_id",
            ))
        }
    };
    let expires_at = expiry(&body)?;
    let pool = pool(&state);
    let chain = requested_chain(pool, org_id, body.team_id, body.project_id).await?;
    // the token's own scope is where the minter needs admin, so a token never
    // holds more than the person who made it
    authorize(&state, &principal, chain, cap!("api_token", Create)).await?;
    if let Some(role_id) = body.custom_role_id {
        let role = CustomRoleRepo(pool).get(role_id).await?;
        if role.org_id != org_id {
            return Err(invalid_field(
                "custom_role_id",
                "the role belongs to another organization",
            ));
        }
    }
    let (secret, hash, prefix) = generate_secret(&session_pepper());
    let token = ApiTokenRepo(pool)
        .create(NewApiToken {
            org_id,
            team_id: chain.team,
            project_id: chain.project,
            name,
            role,
            custom_role_id: body.custom_role_id,
            token_hash: &hash,
            token_prefix: &prefix,
            expires_at,
            created_by: principal.account_id(),
        })
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "api_token.create",
        "api_token",
        token.id,
        audit_detail(&token),
    )
    .await;
    Ok(Json(SecretApiToken { token, secret }))
}

async fn list_tokens(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ApiToken>>> {
    refuse_tokens(&principal)?;
    let filter = ScopeFilter::load(&state, &principal, cap!("api_token", Read)).await?;
    let tokens = ApiTokenRepo(pool(&state)).list(org_id).await?;
    if filter.allows(ScopeChain::org(org_id)) {
        return Ok(Json(tokens));
    }
    // below the org: the tokens of the teams and projects the caller administers
    require_reach(&state, &filter, org_id).await?;
    Ok(Json(
        tokens
            .into_iter()
            .filter(|token| filter.allows(chain_of(token)))
            .collect(),
    ))
}

async fn rotate_token(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<SecretApiToken>> {
    refuse_tokens(&principal)?;
    let repo = ApiTokenRepo(pool(&state));
    let current = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        chain_of(&current),
        cap!("api_token", Update),
    )
    .await?;
    let expired = current.expires_at.is_some_and(|at| at <= Utc::now());
    if current.revoked_at.is_some() || expired {
        return Err(ApiError::CodedConflict {
            code: API_TOKEN_INACTIVE,
            message: "this token is revoked or expired; mint a new one".to_string(),
        });
    }
    let (secret, hash, prefix) = generate_secret(&session_pepper());
    // the old digest stops resolving in this same statement
    let token = repo
        .rotate(id, &hash, &prefix)
        .await?
        .ok_or(ApiError::CodedConflict {
            code: API_TOKEN_INACTIVE,
            message: "this token is revoked or expired; mint a new one".to_string(),
        })?;
    log_audit(
        &state,
        &principal,
        Some(token.org_id),
        "api_token.rotate",
        "api_token",
        id,
        audit_detail(&token),
    )
    .await;
    Ok(Json(SecretApiToken { token, secret }))
}

async fn revoke_token(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiToken>> {
    refuse_tokens(&principal)?;
    let repo = ApiTokenRepo(pool(&state));
    let current = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        chain_of(&current),
        cap!("api_token", Delete),
    )
    .await?;
    let token = repo.revoke(id).await?;
    log_audit(
        &state,
        &principal,
        Some(token.org_id),
        "api_token.revoke",
        "api_token",
        id,
        audit_detail(&token),
    )
    .await;
    Ok(Json(token))
}
