//! Memberships: the role an account holds at an org, team or project.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::Membership;
use rolter_store::postgres::repo::{LockoutGuard, MembershipRepo, UserRepo};

use super::{
    invalid_field, log_audit, pool, project_teams, require_reach, row_chain, ApiError, ApiResult,
    SafeJson,
};
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain, ScopeFilter};
use crate::rbac_matrix::{cap, Requirement};
use crate::ControlState;

use super::users::last_org_admin;

use super::users::validate_role;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/memberships",
            get(list_memberships).post(create_membership),
        )
        .route(
            "/api/v1/memberships/{id}",
            put(update_membership).delete(delete_membership),
        )
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
            return Err(invalid_field(
                "scope_type",
                format!("scope_type must be one of org, team, project (got '{other}')"),
            ))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMembership {
    role: String,
}

/// the scope a grant sits at, or `None` for the row the schema should have
/// made impossible
async fn membership_chain(pool: &PgPool, m: &Membership) -> ApiResult<Option<ScopeChain>> {
    Ok(if let Some(project) = m.project_id {
        Some(ScopeChain::from_project(pool, project).await?)
    } else if let Some(team) = m.team_id {
        Some(ScopeChain::from_team(pool, team).await?)
    } else {
        m.org_id.map(ScopeChain::org)
    })
}

/// change a grant's role in one statement (#2310), authorized at the grant's
/// own scope like the delete. Demoting an org's last admin is refused for
/// everyone but a superadmin, as revoking that grant is (#2311)
async fn update_membership(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateMembership>,
) -> ApiResult<Json<Membership>> {
    validate_role(&body.role)?;
    let pool = pool(&state);
    let current = MembershipRepo(pool).get(id).await?;
    let Some(chain) = membership_chain(pool, &current).await? else {
        // data-integrity fallback, not a route capability: see delete_membership
        authorize_superadmin(&principal, Requirement::unscoped_superadmin())?;
        return Err(ApiError::Core(Error::Config(
            "the grant has no scope".to_string(),
        )));
    };
    let org_id = chain.org;
    authorize(&state, &principal, chain, cap!("membership", Update)).await?;
    if current.role == body.role {
        return Ok(Json(current));
    }
    let (before, after) = match MembershipRepo(pool)
        .update_role_guarded(
            id,
            &body.role,
            !matches!(principal, Principal::Superadmin { .. }),
        )
        .await?
    {
        LockoutGuard::Done(pair) => pair,
        LockoutGuard::WouldLockOut => return Err(last_org_admin()),
    };
    log_audit(
        &state,
        &principal,
        org_id,
        "membership.update",
        "membership",
        id,
        serde_json::json!({
            "user_id": after.user_id,
            "old_role": before.role,
            "new_role": after.role,
        }),
    )
    .await;
    Ok(Json(after))
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
        .delete_guarded(id, !matches!(principal, Principal::Superadmin { .. }))
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
