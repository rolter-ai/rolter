//! The effective model list (config plus database) and removing a database model.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;
use serde::Serialize;

use rolter_store::postgres::repo::RouteRepo;

use super::{pool, publish_config_change, require_not_config_owned, ApiResult};
use crate::access_control::caller_policy;
use crate::rbac::{authorize, authorize_superadmin, Principal, ScopeChain};
use crate::rbac_matrix::{cap, superadmin_cap};
use crate::tenant_view::TenantView;
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/models", get(list_models))
        .route("/api/v1/models/{model}", delete(delete_model))
}

// --- effective model list (config + db, LiteLLM-style) ---

#[derive(Serialize)]
struct EffectiveModel {
    model: String,
    strategy: rolter_core::BalancingStrategy,
    targets: usize,
    /// the targets themselves, in the shape the effective config gives them
    /// (provider name, upstream model, weight), so the catalog needs no second
    /// read of `/api/v1/config` to show where a model's traffic goes (#2210)
    route_targets: Vec<rolter_core::Target>,
    /// the organization the route belongs to; absent for a route from the
    /// bootstrap file. Two organizations can each serve a `gpt-4o`, and this is
    /// what tells their entries apart
    #[serde(skip_serializing_if = "Option::is_none")]
    org_id: Option<String>,
    /// the project the route belongs to; absent for a route from the file
    #[serde(skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    /// "config" = declared in the bootstrap file, read-only at runtime;
    /// "db" = created via this API, full runtime CRUD
    source: &'static str,
}

/// The merged model list the gateway effectively serves: bootstrap-config
/// routes (read-only) plus DB routes, as exposed by the merged store.
async fn list_models(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<EffectiveModel>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model", Read),
    )
    .await?;
    let config = state.store.load().await?;
    // model visibility carried by the caller's access profiles (#534);
    // unrestricted for a superadmin and for anyone holding no policy
    let policy = caller_policy(&state, &principal).await?;
    // the organizations and projects the caller can see (#3094); a row of
    // another tenant is not listed, targets and ids included
    let mut view = TenantView::load(&state, Some(&principal)).await?;
    let mut routes = Vec::new();
    for route in &config.routes {
        if policy.permits_model(&route.model) && view.sees(&state, route.tenancy.as_ref()).await {
            routes.push(route);
        }
    }
    let models = routes
        .into_iter()
        .map(|r| EffectiveModel {
            model: r.model.clone(),
            strategy: r.strategy,
            targets: r.targets.len(),
            route_targets: r.targets.clone(),
            org_id: r.tenancy.as_ref().map(|t| t.org_id.clone()),
            project_id: r.tenancy.as_ref().and_then(|t| t.project_id.clone()),
            // an org's route that shares a name with a file route is the
            // org's, stored and editable (#3516)
            source: if state.config_owned.owns_model(r.tenancy.as_ref(), &r.model) {
                "config"
            } else {
                "db"
            },
        })
        .collect();
    Ok(Json(models))
}

/// Which org's model a delete removes.
#[derive(Deserialize)]
struct DeleteModelQuery {
    /// the org whose routes of that name go; required once two or more orgs
    /// have a route of that name (#1857)
    org_id: Option<uuid::Uuid>,
}

/// Delete a DB-defined model: the routes with that public name in one org,
/// across its projects. Config models are rejected with `409` since the file
/// owns them. Names are per org (#1857), so `org_id` says whose; it may be
/// left out only while a single org has the name, and a delete that would
/// otherwise reach several orgs is refused with `400 invalid_query`.
// deleting a public model name removes routes across potentially many projects,
// so it cannot be scoped to a single team/project and is superadmin-only
async fn delete_model(
    principal: Principal,
    State(state): State<ControlState>,
    Path(model): Path<String>,
    super::Query(query): super::Query<DeleteModelQuery>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("model", Delete))?;
    let routes = RouteRepo(pool(&state));
    let holders = routes.orgs_holding_model(&model).await?;
    // only the file's own row is the file's: an org's stored route that shares
    // the name with a file route is the org's to delete (#3516)
    match query.org_id {
        Some(org) => super::require_row_not_config_owned(
            &state.config_owned.model_rows,
            org,
            &model,
            "model",
        )?,
        None if holders == 0 => {
            require_not_config_owned(&state.config_owned.models, &model, "model")?
        }
        None => {}
    }
    if query.org_id.is_none() && holders > 1 {
        return Err(super::ApiError::InvalidQuery(format!(
            "more than one organization has a model named '{model}'; name the one to delete \
             with org_id"
        )));
    }
    routes.delete_by_model(&model, query.org_id).await?;
    publish_config_change(&state).await?;
    Ok(StatusCode::NO_CONTENT)
}
