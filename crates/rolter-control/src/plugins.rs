//! Org/project-scoped plugin instance registry (#567).
//!
//! Gateway dispatch remains the separate #509 scope; this module owns the
//! persisted configuration contract the dashboard and future dispatcher share.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::slug::{is_valid_slug, slugify};
use rolter_core::Error;
use rolter_store::postgres::models::PluginInstance;
use rolter_store::postgres::repo::PluginRepo;

use crate::crud::{log_audit, pool, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/plugins",
            get(list_plugins).post(create_plugin),
        )
        .route(
            "/api/v1/plugins/{id}",
            axum::routing::put(update_plugin).delete(delete_plugin),
        )
}

#[derive(Debug, Deserialize)]
struct PluginBody {
    project_id: Option<Uuid>,
    name: String,
    slug: Option<String>,
    description: String,
    kind: String,
    stage: String,
    enabled: bool,
    position: i32,
    failure_mode: String,
    endpoint: String,
    secret_env: Option<String>,
    config: serde_json::Value,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate(body: &PluginBody, creating: bool) -> ApiResult<String> {
    if body.name.trim().is_empty() {
        return Err(invalid("name must not be empty"));
    }
    let slug = body
        .slug
        .clone()
        .unwrap_or_else(|| slugify(body.name.trim()));
    if creating && !is_valid_slug(&slug) {
        return Err(invalid(
            "slug must start with a lowercase letter or digit and contain only lowercase letters, digits, hyphens or underscores",
        ));
    }
    if body.kind != "webhook" {
        return Err(invalid("kind must be webhook"));
    }
    if !matches!(
        body.stage.as_str(),
        "pre_route" | "pre_upstream" | "post_response"
    ) {
        return Err(invalid(
            "stage must be pre_route, pre_upstream or post_response",
        ));
    }
    if !matches!(body.failure_mode.as_str(), "fail_open" | "fail_closed") {
        return Err(invalid("failure_mode must be fail_open or fail_closed"));
    }
    if body.position < 0 {
        return Err(invalid("position must be zero or greater"));
    }
    if !(body.endpoint.starts_with("http://") || body.endpoint.starts_with("https://")) {
        return Err(invalid("endpoint must start with http:// or https://"));
    }
    if body
        .secret_env
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(invalid(
            "secret_env must be omitted or name an environment variable",
        ));
    }
    if !body.config.is_object() {
        return Err(invalid("config must be a JSON object"));
    }
    Ok(slug)
}

async fn ensure_project_in_org(
    state: &ControlState,
    project_id: Option<Uuid>,
    org_id: Uuid,
) -> ApiResult<()> {
    let Some(project_id) = project_id else {
        return Ok(());
    };
    let chain = ScopeChain::from_project(pool(state), project_id).await?;
    if chain.org != Some(org_id) {
        return Err(invalid("project_id must belong to the same org"));
    }
    Ok(())
}

async fn plugin_scope(state: &ControlState, plugin: &PluginInstance) -> ApiResult<ScopeChain> {
    match plugin.project_id {
        Some(project_id) => ScopeChain::from_project(pool(state), project_id).await,
        None => Ok(ScopeChain::org(plugin.org_id)),
    }
}

async fn list_plugins(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<PluginInstance>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("plugin", Read),
    )
    .await?;
    Ok(Json(PluginRepo(pool(&state)).list(org_id).await?))
}

async fn create_plugin(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<PluginBody>,
) -> ApiResult<(StatusCode, Json<PluginInstance>)> {
    let scope = match body.project_id {
        Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
        None => ScopeChain::org(org_id),
    };
    if scope.org != Some(org_id) {
        return Err(invalid("project_id must belong to the same org"));
    }
    authorize(&state, &principal, scope, cap!("plugin", Create)).await?;
    let slug = validate(&body, true)?;
    let row = PluginRepo(pool(&state))
        .create(
            org_id,
            body.project_id,
            &body.name,
            &slug,
            &body.description,
            &body.kind,
            &body.stage,
            body.enabled,
            body.position,
            &body.failure_mode,
            &body.endpoint,
            body.secret_env.as_deref(),
            &body.config,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "plugin.create",
        "plugin",
        row.id,
        serde_json::json!({"slug": row.slug, "stage": row.stage, "enabled": row.enabled}),
    )
    .await;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_plugin(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PluginBody>,
) -> ApiResult<Json<PluginInstance>> {
    let repo = PluginRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        plugin_scope(&state, &existing).await?,
        cap!("plugin", Update),
    )
    .await?;
    ensure_project_in_org(&state, body.project_id, existing.org_id).await?;
    if body.project_id != existing.project_id {
        let target_scope = match body.project_id {
            Some(project_id) => ScopeChain::from_project(pool(&state), project_id).await?,
            None => ScopeChain::org(existing.org_id),
        };
        authorize(&state, &principal, target_scope, cap!("plugin", Update)).await?;
    }
    validate(&body, false)?;
    let row = repo
        .update(
            id,
            body.project_id,
            &body.name,
            &body.description,
            &body.stage,
            body.enabled,
            body.position,
            &body.failure_mode,
            &body.endpoint,
            body.secret_env.as_deref(),
            &body.config,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "plugin.update",
        "plugin",
        id,
        serde_json::json!({"slug": row.slug, "stage": row.stage, "enabled": row.enabled}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_plugin(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = PluginRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        plugin_scope(&state, &existing).await?,
        cap!("plugin", Delete),
    )
    .await?;
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "plugin.delete",
        "plugin",
        id,
        serde_json::json!({"slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> PluginBody {
        PluginBody {
            project_id: None,
            name: "PII webhook".into(),
            slug: None,
            description: String::new(),
            kind: "webhook".into(),
            stage: "pre_upstream".into(),
            enabled: false,
            position: 10,
            failure_mode: "fail_closed".into(),
            endpoint: "https://plugins.internal/pii".into(),
            secret_env: Some("ROLTER_PLUGIN_TOKEN".into()),
            config: serde_json::json!({"redact": true}),
        }
    }

    #[test]
    fn derives_a_stable_slug() {
        assert_eq!(validate(&body(), true).unwrap(), "pii-webhook");
    }

    #[test]
    fn rejects_non_object_config() {
        let mut value = body();
        value.config = serde_json::json!([]);
        assert!(validate(&value, true).is_err());
    }
}
