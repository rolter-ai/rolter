//! Prompt templates: the versioned prompt repository and its scopes.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::{PromptTemplate, PromptTemplateScope, PromptTemplateVersion};
use rolter_store::postgres::repo::{PromptTemplateRepo, RouteRepo, VirtualKeyRepo};

use super::{
    invalid_field, log_audit, pool, require_non_empty, resolve_new_slug, ApiError, ApiResult,
    SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
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
        return Err(invalid_field("variables", "variables must be a JSON array"));
    }
    if !body.decorators.is_array() {
        return Err(invalid_field(
            "decorators",
            "decorators must be a JSON array",
        ));
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
        return Err(invalid_field(
            "version",
            "version must be greater than zero",
        ));
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
        return Err(invalid_field(
            "version",
            "version must be greater than zero",
        ));
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
