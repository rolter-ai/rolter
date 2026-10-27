//! Customers: the end-customer accounts an org attributes spend to.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::{Customer, CustomerListing};
use rolter_store::postgres::repo::{BusinessUnitRepo, CustomerRepo};

use super::{
    log_audit, pool, require_non_empty, resolve_new_slug, resolve_slug_change, ApiError, ApiResult,
    SafeJson,
};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/customers",
            get(list_customers).post(create_customer),
        )
        .route(
            "/api/v1/customers/{id}",
            put(update_customer).delete(delete_customer),
        )
}

// --- customers ---

async fn list_customers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<CustomerListing>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("customer", Read),
    )
    .await?;
    // see list_business_units for why the count is part of the listing
    Ok(Json(
        CustomerRepo(pool(&state))
            .list_with_key_counts(org_id)
            .await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateCustomer {
    name: String,
    /// Stable URL-safe identity; derived from `name` when omitted.
    slug: Option<String>,
    business_unit_id: Option<Uuid>,
}

async fn create_customer(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateCustomer>,
) -> ApiResult<Json<Customer>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("customer", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    let slug = resolve_new_slug(&body.name, body.slug.as_deref())?;
    if let Some(business_unit_id) = body.business_unit_id {
        let unit = BusinessUnitRepo(pool(&state)).get(business_unit_id).await?;
        if unit.org_id != org_id {
            return Err(ApiError::Core(Error::Config(
                "business_unit_id must belong to the same org".to_string(),
            )));
        }
    }
    let customer = CustomerRepo(pool(&state))
        .create(org_id, body.business_unit_id, &body.name, &slug)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "customer.create",
        "customer",
        customer.id,
        serde_json::json!({"name": customer.name, "slug": customer.slug, "business_unit_id": customer.business_unit_id}),
    )
    .await;
    Ok(Json(customer))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateCustomer {
    name: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    allow_slug_change: bool,
    /// Omit to leave unchanged, null to clear, UUID to set.
    #[serde(default)]
    business_unit_id: NullableUuid,
    retired: Option<bool>,
}

#[derive(Default)]
pub(super) enum NullableUuid {
    #[default]
    Missing,
    Null,
    Value(Uuid),
}

impl<'de> Deserialize<'de> for NullableUuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match Option::<Uuid>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

async fn update_customer(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateCustomer>,
) -> ApiResult<Json<Customer>> {
    let repo = CustomerRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("customer", Update),
    )
    .await?;
    if let Some(name) = &body.name {
        require_non_empty(name, "name")?;
    }
    let slug_change =
        resolve_slug_change(body.slug.as_deref(), &existing.slug, body.allow_slug_change)?;
    if let NullableUuid::Value(business_unit_id) = &body.business_unit_id {
        let unit = BusinessUnitRepo(pool(&state))
            .get(*business_unit_id)
            .await?;
        if unit.org_id != existing.org_id {
            return Err(ApiError::Core(Error::Config(
                "business_unit_id must belong to the same org".to_string(),
            )));
        }
    }
    let business_unit_id = match body.business_unit_id {
        NullableUuid::Missing => None,
        NullableUuid::Null => Some(None),
        NullableUuid::Value(id) => Some(Some(id)),
    };
    let customer = repo
        .update(
            id,
            business_unit_id,
            body.name.as_deref(),
            slug_change.as_deref(),
            body.retired,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "customer.update",
        "customer",
        id,
        serde_json::json!({"slug": customer.slug, "retired": customer.retired_at.is_some(), "business_unit_id": customer.business_unit_id}),
    )
    .await;
    Ok(Json(customer))
}

async fn delete_customer(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = CustomerRepo(pool(&state));
    let existing = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(existing.org_id),
        cap!("customer", Delete),
    )
    .await?;
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(existing.org_id),
        "customer.delete",
        "customer",
        id,
        serde_json::json!({"name": existing.name, "slug": existing.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
