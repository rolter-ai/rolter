//! Configurable RBAC (#534): org-defined roles, the explicit grants they carry,
//! and the access profiles that hand them to people.
//!
//! ## What this adds, and what it deliberately does not
//!
//! The built-in `admin`/`member`/`viewer` roles and the `memberships` table are
//! untouched. Everything here is **additive**: a custom grant can only widen
//! what a membership already allows. [`crate::rbac::authorize`] consults these
//! tables only *after* the membership check has already failed, so
//!
//! * a deployment with no custom roles behaves exactly as it did before — same
//!   answers, and not one extra query on any request that is allowed;
//! * no custom role can take access away, which means an operator cannot lock
//!   themselves out by defining one;
//! * a custom role can never reach a **superadmin** capability. Deployment-wide
//!   policy has no org for an org admin to define a role in, so granting it
//!   from inside a tenant would be an escalation path out of that tenant.
//!
//! ## Composition
//!
//! ```text
//! custom_role ──grants──▶ (resource, action)*        + a base_role floor
//!      ▲
//!      └── access_profile_roles (role @ org|team|project scope)
//!                 ▲
//!                 ├── access_profile ──▶ model/route policy
//!                 └── access_profile_assignments (user | team)
//! ```
//!
//! A profile assigned to a team reaches every member of that team, including
//! someone whose only membership is on a project inside it. Scope inheritance
//! follows the same most-specific-non-null convention as memberships, so a role
//! composed at team scope applies to the projects under that team.
//!
//! ## Deleting a role
//!
//! A custom role still composed into any profile **cannot be deleted** (`409`,
//! naming how many compositions block it). The alternatives were considered and
//! rejected: cascading the composition away, or silently falling back to the
//! base role, both change what real people can do while producing an audit
//! entry that names only the role — never the profiles, and never the users. A
//! block forces the operator to detach first, and each detach is its own
//! audited `access_profile.update`. Deleting a *profile* does cascade its
//! assignments and policy, because that only ever removes access.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use rolter_auth::Role;
use rolter_store::postgres::models::{
    AccessProfile, AccessProfileAssignment, AccessProfilePolicy, AccessProfileRole, CustomRole,
    CustomRoleGrant,
};
use rolter_store::postgres::repo::{AccessProfileRepo, CustomRoleRepo, ProjectRepo, TeamRepo};

use crate::crud::{
    log_audit, pool, require_non_empty, resolve_new_slug, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/custom-roles",
            get(list_custom_roles).post(create_custom_role),
        )
        .route(
            "/api/v1/custom-roles/{id}",
            get(get_custom_role)
                .put(update_custom_role)
                .delete(delete_custom_role),
        )
        .route(
            "/api/v1/orgs/{org_id}/access-profiles",
            get(list_profiles).post(create_profile),
        )
        .route(
            "/api/v1/access-profiles/{id}",
            get(get_profile).put(update_profile).delete(delete_profile),
        )
        .route(
            "/api/v1/access-profiles/{id}/assignments",
            get(list_assignments).post(create_assignment),
        )
        .route(
            "/api/v1/access-profile-assignments/{id}",
            delete(delete_assignment),
        )
        .route("/api/v1/access-profiles/{id}/policy", put(set_policy))
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(rolter_core::Error::Config(message.into()))
}

/// Interpret a stored `base_role` string. The column is `check`-constrained to
/// the three built-ins, so an unrecognized value can only come from a hand-
/// edited row; it degrades to the weakest role rather than the strongest.
pub(crate) fn parse_base_role(role: &str) -> Role {
    match role {
        "admin" => Role::Admin,
        "member" => Role::Member,
        _ => Role::Viewer,
    }
}

const BASE_ROLES: [&str; 3] = ["admin", "member", "viewer"];
const ACTIONS: [&str; 4] = ["read", "create", "update", "delete"];

fn validate_base_role(role: &str) -> ApiResult<()> {
    BASE_ROLES
        .contains(&role)
        .then_some(())
        .ok_or_else(|| invalid("base_role must be one of admin, member, viewer"))
}

// ---------------------------------------------------------------------------
// custom roles
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GrantBody {
    resource: String,
    action: String,
}

#[derive(Debug, Deserialize)]
struct CreateCustomRole {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_base_role")]
    base_role: String,
    #[serde(default)]
    grants: Vec<GrantBody>,
}

fn default_base_role() -> String {
    "viewer".to_string()
}

#[derive(Debug, Serialize)]
struct CustomRoleDetail {
    #[serde(flatten)]
    role: CustomRole,
    grants: Vec<CustomRoleGrant>,
}

/// Normalize and validate a grant list. Resources are not checked against the
/// capability table: it is code, and a role naming a resource this build does
/// not define must stay storable (it simply grants nothing) so a rollback does
/// not make an org's roles unsaveable. `GET /api/v1/rbac/matrix?org_id=` reports
/// such grants separately as `unknown_grants`.
fn normalize_grants(grants: &[GrantBody]) -> ApiResult<Vec<(String, String)>> {
    let mut out = Vec::with_capacity(grants.len());
    for grant in grants {
        let resource = grant.resource.trim().to_string();
        require_non_empty(&resource, "grants[].resource")?;
        let action = grant.action.trim().to_lowercase();
        if !ACTIONS.contains(&action.as_str()) {
            return Err(invalid(
                "grants[].action must be one of read, create, update, delete",
            ));
        }
        out.push((resource, action));
    }
    Ok(out)
}

async fn list_custom_roles(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<CustomRole>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("custom_role", Read),
    )
    .await?;
    Ok(Json(CustomRoleRepo(pool(&state)).list(org_id).await?))
}

async fn get_custom_role(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<CustomRoleDetail>> {
    let repo = CustomRoleRepo(pool(&state));
    let role = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(role.org_id),
        cap!("custom_role", Read),
    )
    .await?;
    let grants = repo.list_grants(id).await?;
    Ok(Json(CustomRoleDetail { role, grants }))
}

async fn create_custom_role(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateCustomRole>,
) -> ApiResult<Json<CustomRoleDetail>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("custom_role", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    validate_base_role(&body.base_role)?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let grants = normalize_grants(&body.grants)?;
    let repo = CustomRoleRepo(pool(&state));
    let role = repo
        .create(
            org_id,
            &slug,
            &body.name,
            body.description.as_deref(),
            &body.base_role,
        )
        .await?;
    repo.set_grants(role.id, &grants).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "custom_role.create",
        "custom_role",
        role.id,
        json!({"slug": role.slug, "base_role": role.base_role, "grants": grants.len()}),
    )
    .await;
    let grants = repo.list_grants(role.id).await?;
    Ok(Json(CustomRoleDetail { role, grants }))
}

#[derive(Debug, Deserialize)]
struct UpdateCustomRole {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    base_role: Option<String>,
    /// absent leaves the grants alone; present replaces them wholesale
    #[serde(default)]
    grants: Option<Vec<GrantBody>>,
}

async fn update_custom_role(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateCustomRole>,
) -> ApiResult<Json<CustomRoleDetail>> {
    let repo = CustomRoleRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("custom_role", Update),
    )
    .await?;
    let name = match body.name.as_deref() {
        Some(name) => {
            require_non_empty(name, "name")?;
            name.to_string()
        }
        None => existing.name.clone(),
    };
    let base_role = body.base_role.clone().unwrap_or(existing.base_role.clone());
    validate_base_role(&base_role)?;
    let description = body.description.clone().or(existing.description.clone());
    let role = repo
        .update(id, &name, description.as_deref(), &base_role)
        .await?;
    // an edit takes effect for everyone who holds the role, immediately: that
    // is what a role is for. It is audited with the before/after so the change
    // is attributable even though no assignment row moved.
    if let Some(grants) = &body.grants {
        repo.set_grants(id, &normalize_grants(grants)?).await?;
    }
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "custom_role.update",
        "custom_role",
        id,
        json!({
            "slug": role.slug,
            "base_role_from": existing.base_role,
            "base_role_to": role.base_role,
            "grants_replaced": body.grants.is_some(),
        }),
    )
    .await;
    let grants = repo.list_grants(id).await?;
    Ok(Json(CustomRoleDetail { role, grants }))
}

async fn delete_custom_role(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = CustomRoleRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("custom_role", Delete),
    )
    .await?;
    // block-if-referenced: see the module docs. Detaching is an audited profile
    // update, so nobody's access changes without a record that names the profile
    let references = repo.reference_count(id).await?;
    if references > 0 {
        return Err(ApiError::Conflict(format!(
            "custom role '{}' is still composed into {references} access profile(s); \
             remove it from them first",
            existing.slug
        )));
    }
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "custom_role.delete",
        "custom_role",
        id,
        json!({"slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// access profiles
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProfileRoleBody {
    role_id: Uuid,
    #[serde(default)]
    org_id: Option<Uuid>,
    #[serde(default)]
    team_id: Option<Uuid>,
    #[serde(default)]
    project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct PolicyBody {
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    denied_models: Vec<String>,
    #[serde(default)]
    allowed_routes: Vec<String>,
    #[serde(default)]
    denied_routes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CreateProfile {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    roles: Vec<ProfileRoleBody>,
    #[serde(default)]
    policy: Option<PolicyBody>,
}

#[derive(Debug, Serialize)]
struct ProfileDetail {
    #[serde(flatten)]
    profile: AccessProfile,
    roles: Vec<AccessProfileRole>,
    assignments: Vec<AccessProfileAssignment>,
    policy: Option<AccessProfilePolicy>,
}

/// Resolve and validate a profile's composition.
///
/// Two things must hold, and both are tenancy containment rather than
/// bookkeeping: every composed role must belong to this profile's org, and
/// every scope must sit inside it. Without either, an org admin could compose a
/// role into a scope belonging to a different tenant and grant themselves a
/// foothold there.
async fn resolve_roles(
    state: &ControlState,
    org_id: Uuid,
    roles: &[ProfileRoleBody],
) -> ApiResult<Vec<(Uuid, Option<Uuid>, Option<Uuid>, Option<Uuid>)>> {
    let repo = CustomRoleRepo(pool(state));
    let mut out = Vec::with_capacity(roles.len());
    for entry in roles {
        let role = repo.get(entry.role_id).await?;
        if role.org_id != org_id {
            return Err(invalid(
                "a profile may only compose custom roles from its own org",
            ));
        }
        let scope_org = match (entry.org_id, entry.team_id, entry.project_id) {
            (_, _, Some(project)) => {
                let project = ProjectRepo(pool(state)).get(project).await?;
                TeamRepo(pool(state)).get(project.team_id).await?.org_id
            }
            (_, Some(team), None) => TeamRepo(pool(state)).get(team).await?.org_id,
            (Some(org), None, None) => org,
            // no scope at all defaults to the profile's own org
            (None, None, None) => org_id,
        };
        if scope_org != org_id {
            return Err(invalid(
                "a profile may only grant a role inside its own org",
            ));
        }
        out.push((
            entry.role_id,
            // an org-scoped grant only when neither a team nor a project is
            // named; a narrower scope carries its org implicitly
            (entry.team_id.is_none() && entry.project_id.is_none())
                .then(|| entry.org_id.unwrap_or(org_id)),
            entry.team_id,
            entry.project_id,
        ));
    }
    Ok(out)
}

async fn list_profiles(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<AccessProfile>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("access_profile", Read),
    )
    .await?;
    Ok(Json(AccessProfileRepo(pool(&state)).list(org_id).await?))
}

async fn get_profile(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ProfileDetail>> {
    let repo = AccessProfileRepo(pool(&state));
    let profile = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(profile.org_id),
        cap!("access_profile", Read),
    )
    .await?;
    Ok(Json(ProfileDetail {
        roles: repo.list_roles(id).await?,
        assignments: repo.list_assignments(id).await?,
        policy: repo.get_policy(id).await?,
        profile,
    }))
}

async fn create_profile(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateProfile>,
) -> ApiResult<Json<ProfileDetail>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("access_profile", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    let roles = resolve_roles(&state, org_id, &body.roles).await?;
    let repo = AccessProfileRepo(pool(&state));
    let profile = repo
        .create(org_id, &slug, &body.name, body.description.as_deref())
        .await?;
    repo.set_roles(profile.id, &roles).await?;
    if let Some(policy) = &body.policy {
        write_policy(&repo, profile.id, policy).await?;
    }
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "access_profile.create",
        "access_profile",
        profile.id,
        json!({"slug": profile.slug, "roles": roles.len()}),
    )
    .await;
    Ok(Json(ProfileDetail {
        roles: repo.list_roles(profile.id).await?,
        assignments: Vec::new(),
        policy: repo.get_policy(profile.id).await?,
        profile,
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateProfile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// absent leaves the composition alone; present replaces it wholesale,
    /// which is also how a role is detached before it can be deleted
    #[serde(default)]
    roles: Option<Vec<ProfileRoleBody>>,
    #[serde(default)]
    policy: Option<PolicyBody>,
}

async fn update_profile(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateProfile>,
) -> ApiResult<Json<ProfileDetail>> {
    let repo = AccessProfileRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("access_profile", Update),
    )
    .await?;
    let name = match body.name.as_deref() {
        Some(name) => {
            require_non_empty(name, "name")?;
            name.to_string()
        }
        None => existing.name.clone(),
    };
    let description = body.description.clone().or(existing.description.clone());
    let profile = repo.update(id, &name, description.as_deref()).await?;
    if let Some(roles) = &body.roles {
        let resolved = resolve_roles(&state, existing.org_id, roles).await?;
        repo.set_roles(id, &resolved).await?;
    }
    if let Some(policy) = &body.policy {
        write_policy(&repo, id, policy).await?;
    }
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "access_profile.update",
        "access_profile",
        id,
        json!({
            "slug": profile.slug,
            "roles_replaced": body.roles.is_some(),
            "policy_replaced": body.policy.is_some(),
        }),
    )
    .await;
    Ok(Json(ProfileDetail {
        roles: repo.list_roles(id).await?,
        assignments: repo.list_assignments(id).await?,
        policy: repo.get_policy(id).await?,
        profile,
    }))
}

async fn delete_profile(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = AccessProfileRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("access_profile", Delete),
    )
    .await?;
    // unlike a custom role, a profile cascades: its compositions, assignments
    // and policy all go with it, and every one of those only removes access
    let assignments = repo.list_assignments(id).await?.len();
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "access_profile.delete",
        "access_profile",
        id,
        json!({"slug": existing.slug, "assignments_revoked": assignments}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_policy(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PolicyBody>,
) -> ApiResult<Json<AccessProfilePolicy>> {
    let repo = AccessProfileRepo(pool(&state));
    let profile = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(profile.org_id),
        cap!("access_profile", Update),
    )
    .await?;
    let policy = write_policy(&repo, id, &body).await?;
    log_audit(
        &state,
        &principal,
        Some(profile.org_id),
        "access_profile.policy.update",
        "access_profile",
        id,
        json!({
            "allowed_models": policy.allowed_models.len(),
            "denied_models": policy.denied_models.len(),
            "allowed_routes": policy.allowed_routes.len(),
            "denied_routes": policy.denied_routes.len(),
        }),
    )
    .await;
    Ok(Json(policy))
}

async fn write_policy(
    repo: &AccessProfileRepo<'_>,
    profile_id: Uuid,
    body: &PolicyBody,
) -> ApiResult<AccessProfilePolicy> {
    let clean = |patterns: &[String]| -> Vec<String> {
        patterns
            .iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    };
    Ok(repo
        .set_policy(
            profile_id,
            &clean(&body.allowed_models),
            &clean(&body.denied_models),
            &clean(&body.allowed_routes),
            &clean(&body.denied_routes),
        )
        .await?)
}

// ---------------------------------------------------------------------------
// assignments
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateAssignment {
    #[serde(default)]
    user_id: Option<Uuid>,
    #[serde(default)]
    team_id: Option<Uuid>,
}

async fn list_assignments(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<AccessProfileAssignment>>> {
    let repo = AccessProfileRepo(pool(&state));
    let profile = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(profile.org_id),
        cap!("access_profile_assignment", Read),
    )
    .await?;
    Ok(Json(repo.list_assignments(id).await?))
}

async fn create_assignment(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateAssignment>,
) -> ApiResult<Json<AccessProfileAssignment>> {
    let repo = AccessProfileRepo(pool(&state));
    let profile = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(profile.org_id),
        cap!("access_profile_assignment", Create),
    )
    .await?;
    let subject = match (body.user_id, body.team_id) {
        (Some(user), None) => ("user", user),
        (None, Some(team)) => {
            // a profile grants inside its own org, so it may only be handed to
            // a team of that org
            let team_row = TeamRepo(pool(&state)).get(team).await?;
            if team_row.org_id != profile.org_id {
                return Err(invalid(
                    "an access profile may only be assigned to a team in its own org",
                ));
            }
            ("team", team)
        }
        _ => {
            return Err(invalid(
                "an assignment names exactly one of user_id or team_id",
            ))
        }
    };
    let assignment = repo.assign(id, body.user_id, body.team_id).await?;
    log_audit(
        &state,
        &principal,
        Some(profile.org_id),
        "access_profile_assignment.create",
        "access_profile_assignment",
        assignment.id,
        json!({"profile": profile.slug, "subject_type": subject.0, "subject_id": subject.1}),
    )
    .await;
    Ok(Json(assignment))
}

async fn delete_assignment(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = AccessProfileRepo(pool(&state));
    let assignment = repo.get_assignment(id).await?;
    let profile = repo.get(assignment.profile_id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(profile.org_id),
        cap!("access_profile_assignment", Delete),
    )
    .await?;
    repo.unassign(id).await?;
    log_audit(
        &state,
        &principal,
        Some(profile.org_id),
        "access_profile_assignment.delete",
        "access_profile_assignment",
        id,
        json!({
            "profile": profile.slug,
            "user_id": assignment.user_id,
            "team_id": assignment.team_id,
        }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// model / route policy evaluation
// ---------------------------------------------------------------------------

/// The union of every model/route policy the caller's profiles carry.
///
/// The type, its merge rule and its allow/deny matching live in `rolter-core`:
/// the gateway now enforces the same policy this plane reports (#791), and a
/// second implementation here would be free to drift from the one guarding
/// traffic.
pub(crate) type MergedPolicy = rolter_core::ModelPolicy;

/// Combine the policies of several profiles the same caller holds. See
/// [`rolter_core::ModelPolicy::merge`] for why this unions rather than
/// intersects.
pub(crate) fn merge_policies(policies: &[AccessProfilePolicy]) -> MergedPolicy {
    MergedPolicy::merge(policies.iter().map(|p| MergedPolicy {
        allowed_models: p.allowed_models.clone(),
        denied_models: p.denied_models.clone(),
        allowed_routes: p.allowed_routes.clone(),
        denied_routes: p.denied_routes.clone(),
    }))
}

/// The model/route policy that applies to this caller. Unrestricted for a
/// superadmin and for anyone whose profiles carry none — which is every caller
/// on a deployment that defines no access profiles.
pub(crate) async fn caller_policy(
    state: &ControlState,
    principal: &Principal,
) -> ApiResult<MergedPolicy> {
    let user = match principal {
        Principal::Superadmin { .. } => return Ok(MergedPolicy::default()),
        Principal::User(user) => user,
    };
    let policies = AccessProfileRepo(pool(state))
        .policies_for_user(user.id)
        .await?;
    Ok(merge_policies(&policies))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn policy(allowed: &[&str], denied: &[&str]) -> AccessProfilePolicy {
        AccessProfilePolicy {
            profile_id: Uuid::new_v4(),
            allowed_models: allowed.iter().map(|s| s.to_string()).collect(),
            denied_models: denied.iter().map(|s| s.to_string()).collect(),
            allowed_routes: Vec::new(),
            denied_routes: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn no_policy_permits_everything() {
        let merged = merge_policies(&[]);
        assert!(merged.is_unrestricted());
        assert!(merged.permits_model("anything-at-all"));
        assert!(merged.permits_route("anything-at-all"));
    }

    #[test]
    fn an_allow_list_restricts_and_a_glob_matches_by_prefix() {
        let merged = merge_policies(&[policy(&["gpt-*", "claude-opus"], &[])]);
        assert!(!merged.is_unrestricted());
        assert!(merged.permits_model("gpt-4o"));
        assert!(merged.permits_model("claude-opus"));
        assert!(!merged.permits_model("claude-haiku"));
        assert!(!merged.permits_model("llama-3"));
    }

    #[test]
    fn deny_beats_allow() {
        let merged = merge_policies(&[policy(&["gpt-*"], &["gpt-4o"])]);
        assert!(merged.permits_model("gpt-4o-mini"));
        assert!(!merged.permits_model("gpt-4o"));
        // and a deny with no allow-list still restricts
        let deny_only = merge_policies(&[policy(&[], &["secret-*"])]);
        assert!(!deny_only.is_unrestricted());
        assert!(!deny_only.permits_model("secret-model"));
        assert!(deny_only.permits_model("public-model"));
    }

    #[test]
    fn two_profiles_union_rather_than_intersect() {
        // holding another profile must never take access away
        let merged = merge_policies(&[policy(&["gpt-*"], &[]), policy(&["claude-*"], &[])]);
        assert!(merged.permits_model("gpt-4o"));
        assert!(merged.permits_model("claude-opus"));
        assert!(!merged.permits_model("llama-3"));
    }

    #[test]
    fn an_unrestricted_profile_makes_the_union_unrestricted() {
        // one profile already permitted everything; adding a narrow second one
        // cannot retroactively narrow the first
        let merged = merge_policies(&[policy(&["gpt-*"], &[]), policy(&[], &[])]);
        assert!(merged.allowed_models.is_empty());
        assert!(merged.permits_model("llama-3"));
    }

    #[test]
    fn a_deny_survives_the_union() {
        // a deny is the one direction that does compose restrictively, because
        // it is an explicit statement rather than the absence of one
        let merged = merge_policies(&[policy(&[], &["secret-*"]), policy(&[], &[])]);
        assert!(!merged.permits_model("secret-x"));
    }

    #[test]
    fn a_bare_star_is_everything() {
        // pattern matching itself now lives in `rolter_core::ModelPolicy`; this
        // asserts the control plane still reaches it through `merge_policies`
        assert!(merge_policies(&[policy(&["*"], &[])]).permits_model("anything"));
        // a pattern without a trailing star is an exact name, not a prefix
        assert!(!merge_policies(&[policy(&["any"], &[])]).permits_model("anything"));
        assert!(merge_policies(&[policy(&["any*"], &[])]).permits_model("anything"));
    }

    #[test]
    fn base_role_parsing_degrades_to_the_weakest_role() {
        assert_eq!(parse_base_role("admin"), Role::Admin);
        assert_eq!(parse_base_role("member"), Role::Member);
        assert_eq!(parse_base_role("viewer"), Role::Viewer);
        // a hand-edited row must not silently become an admin
        assert_eq!(parse_base_role("superadmin"), Role::Viewer);
        assert_eq!(parse_base_role(""), Role::Viewer);
    }

    #[test]
    fn only_the_three_built_ins_are_valid_base_roles() {
        assert!(validate_base_role("admin").is_ok());
        assert!(validate_base_role("viewer").is_ok());
        assert!(validate_base_role("superadmin").is_err());
    }

    #[test]
    fn grants_are_normalized_and_bad_actions_rejected() {
        let grants = normalize_grants(&[
            GrantBody {
                resource: "  provider ".into(),
                action: "CREATE".into(),
            },
            GrantBody {
                resource: "route".into(),
                action: "read".into(),
            },
        ])
        .unwrap();
        assert_eq!(
            grants,
            vec![
                ("provider".to_string(), "create".to_string()),
                ("route".to_string(), "read".to_string()),
            ]
        );
        assert!(normalize_grants(&[GrantBody {
            resource: "provider".into(),
            action: "execute".into(),
        }])
        .is_err());
        // and a resource this build does not know is stored, not rejected: the
        // capability table is code, and a rollback must not make roles unsaveable
        assert!(normalize_grants(&[GrantBody {
            resource: "a-resource-from-a-future-release".into(),
            action: "read".into(),
        }])
        .is_ok());
    }
}
