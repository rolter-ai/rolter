//! Business units: an org's reporting hierarchy.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_store::postgres::models::{BusinessUnit, BusinessUnitListing};
use rolter_store::postgres::repo::BusinessUnitRepo;

use super::{
    log_audit, pool, require_non_empty, resolve_new_slug, resolve_slug_change, ApiResult, SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/business-units",
            get(list_business_units).post(create_business_unit),
        )
        .route(
            "/api/v1/business-units/{id}",
            put(update_business_unit).delete(delete_business_unit),
        )
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
