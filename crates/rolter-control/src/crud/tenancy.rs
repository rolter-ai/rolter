//! Tenancy: orgs, teams, projects and per-project settings.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use rolter_store::postgres::models::{Org, OrgProject, Project, Team};
use rolter_store::postgres::repo::{OrgRepo, ProjectRepo, TeamRepo};

use super::{
    invalid_field, log_audit, pool, require_non_empty, row_chain, validate_slug, ApiError,
    ApiResult, SafeJson, REFERENCED,
};
use crate::rbac::{
    authorize, authorize_superadmin, reaches_org, Principal, ScopeChain, ScopeFilter,
};
use crate::rbac_matrix::{cap, superadmin_cap};
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/orgs", get(list_orgs).post(create_org))
        .route("/api/v1/orgs/{id}", delete(delete_org))
        .route(
            "/api/v1/orgs/{org_id}/teams",
            get(list_teams).post(create_team),
        )
        .route("/api/v1/teams/{id}", put(update_team).delete(delete_team))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateTeam {
    name: String,
}

/// Rename a team. Decided at the team's own scope, so a team admin renames
/// their team while creating and deleting one stay the org admin's.
async fn update_team(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateTeam>,
) -> ApiResult<Json<Team>> {
    let chain = ScopeChain::from_team(pool(&state), id).await?;
    authorize(&state, &principal, chain, cap!("team", Update)).await?;
    let name = body.name.trim();
    require_non_empty(name, "name")?;
    let repo = TeamRepo(pool(&state));
    let before = repo.get(id).await?;
    let team = repo.rename(id, name).await?;
    log_audit(
        &state,
        &principal,
        chain.org,
        "team.update",
        "team",
        id,
        serde_json::json!({"old_name": before.name, "new_name": team.name}),
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
    // `team` rows are org-scoped, like creating one: an admin of the team
    // itself does not delete it (#3074)
    let org_chain = ScopeChain {
        team: None,
        project: None,
        ..chain
    };
    authorize(&state, &principal, org_chain, cap!("team", Delete)).await?;
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
    // `project` rows are team-scoped, like creating one: an admin of the
    // project itself does not delete it (#3074)
    let team_chain = ScopeChain {
        project: None,
        ..chain
    };
    authorize(&state, &principal, team_chain, cap!("project", Delete)).await?;
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
        return Err(invalid_field(
            "payload_min_role",
            format!(
                "payload_min_role must be one of {}",
                PAYLOAD_MIN_ROLES.join(", ")
            ),
        ));
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
