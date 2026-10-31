//! Skills: the org skills repository and its versions.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::{Skill, SkillVersion};
use rolter_store::postgres::repo::{SkillRepo, TeamRepo};

use super::{
    invalid_field, log_audit, pool, require_non_empty, resolve_new_slug, ApiError, ApiResult,
    SafeJson,
};
use crate::rbac::{authorize, policy_allows, PolicyContext, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
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
    // the caller's authorization data once, not once per skill (#1098)
    let policy = PolicyContext::load(&state, &principal, org_id).await?;
    let visible = SkillRepo(pool(&state))
        .list_skills(org_id)
        .await?
        .into_iter()
        .filter(|skill| policy.allows(&skill.allowed_team_ids, &skill.minimum_role))
        .collect();
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
        _ => Err(invalid_field(
            "minimum_role",
            "minimum_role must be one of viewer, member, admin",
        )),
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
        return Err(invalid_field(
            "content_ref",
            "content_ref must use https, git+https, oci, or s3 without embedded credentials",
        ));
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
        return Err(invalid_field("metadata", "metadata must be a JSON object"));
    }
    if metadata_contains_sensitive_key(&body.metadata) {
        return Err(invalid_field(
            "metadata",
            "metadata must not contain secret-bearing fields",
        ));
    }
    match (&body.content, &body.content_ref) {
        (Some(content), None) => require_non_empty(content, "content")?,
        (None, Some(reference)) => validate_skill_reference(reference)?,
        _ => {
            return Err(invalid_field(
                "content",
                "exactly one of content or content_ref is required",
            ))
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
        return Err(invalid_field(
            "version",
            "version must be greater than zero",
        ));
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
