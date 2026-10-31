//! Rate limits: request and token caps per scope.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_store::postgres::models::RateLimit;
use rolter_store::postgres::repo::{RateLimitRepo, Within};

use super::{
    explicit_null, invalid_field, log_audit, pool, publish_config_change, ApiResult, Query,
    SafeJson,
};
use crate::rbac::{authorize_cap, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

use super::budgets::readable_caps;

use super::budgets::validate_scope;

use super::budgets::authorize_scope_read;

use super::budgets::ScopeQuery;

pub(super) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/rate-limits",
            get(list_org_rate_limits),
        )
        .route(
            "/api/v1/rate-limits",
            get(list_rate_limits).post(create_rate_limit),
        )
        .route(
            "/api/v1/rate-limits/{id}",
            patch(update_rate_limit).delete(delete_rate_limit),
        )
}

// --- rate limits ---

async fn list_rate_limits(
    principal: Principal,
    State(state): State<ControlState>,
    Query(scope): Query<ScopeQuery>,
) -> ApiResult<Json<Vec<RateLimit>>> {
    validate_scope(&scope.scope_type)?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &scope.scope_type,
        scope.scope_id,
        scope.within(),
    )
    .await?;
    authorize_scope_read(&state, &principal, &scope, chain, cap!("rate_limit", Read)).await?;
    let repo = RateLimitRepo(pool(&state));
    Ok(Json(if scope.scope_type == "user" {
        repo.list_for_user(scope.scope_id, scope.within()).await?
    } else {
        repo.list_for_scope(&scope.scope_type, scope.scope_id)
            .await?
    }))
}

/// Every rate limit the caller may read across one org; see
/// [`list_org_budgets`].
async fn list_org_rate_limits(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<OrgRateLimit>>> {
    let rows = RateLimitRepo(pool(&state)).list_in_org(org_id).await?;
    let rows = readable_caps(
        &state,
        &principal,
        cap!("rate_limit", Read),
        org_id,
        rows,
        |r| {
            (
                r.scope_type.as_str(),
                r.scope_id,
                Within {
                    org: r.within_org_id,
                    project: r.within_project_id,
                },
            )
        },
    )
    .await?;
    let names = super::budgets::key_names(
        &state,
        rows.iter().map(|r| (r.scope_type.as_str(), r.scope_id)),
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| OrgRateLimit {
                scope_name: (row.scope_type == "virtual_key")
                    .then(|| names.get(&row.scope_id).cloned())
                    .flatten(),
                row,
            })
            .collect(),
    ))
}

/// A rate limit of an org-wide list, with the name of the key it caps (#3128);
/// see `OrgBudget`.
#[derive(serde::Serialize)]
struct OrgRateLimit {
    #[serde(flatten)]
    row: RateLimit,
    scope_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateRateLimit {
    scope_type: String,
    scope_id: Uuid,
    /// a `user` limit only: count only the keys the person minted in this
    /// org (#1830); see [`CreateBudget`](super::budgets)
    #[serde(default)]
    within_org_id: Option<Uuid>,
    /// a `user` limit only: count only the keys the person minted in this
    /// project
    #[serde(default)]
    within_project_id: Option<Uuid>,
    pub(super) rpm: Option<i32>,
    pub(super) tpm: Option<i32>,
}

async fn create_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateRateLimit>,
) -> ApiResult<Json<RateLimit>> {
    validate_scope(&body.scope_type)?;
    let within = Within {
        org: body.within_org_id,
        project: body.within_project_id,
    };
    super::budgets::validate_within(&body.scope_type, within)?;
    let chain = ScopeChain::from_cap(pool(&state), &body.scope_type, body.scope_id, within).await?;
    let org_id = chain.org;
    authorize_cap(&state, &principal, chain, cap!("rate_limit", Create)).await?;
    // each cap sent has to limit something, and the store refuses a limit
    // left with none, as it does for an edit (#1903)
    validate_rate_limit_caps(body.rpm, body.tpm)?;
    let row = RateLimitRepo(pool(&state))
        .create(&body.scope_type, body.scope_id, body.rpm, body.tpm, within)
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "rate_limit.create",
        "rate_limit",
        row.id,
        serde_json::json!({"scope_type": body.scope_type, "scope_id": body.scope_id, "within_org_id": body.within_org_id, "within_project_id": body.within_project_id, "rpm": body.rpm, "tpm": body.tpm}),
    )
    .await;
    Ok(Json(row))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateRateLimit {
    // each cap is doubly optional: absent leaves it, null lifts it, a number
    // sets it. a plain Option would make "lift the rpm cap" unsayable
    #[serde(default, deserialize_with = "explicit_null")]
    pub(super) rpm: Option<Option<i32>>,
    #[serde(default, deserialize_with = "explicit_null")]
    pub(super) tpm: Option<Option<i32>>,
}

/// Check each cap a rate-limit create or edit sets.
///
/// `None` is a cap that is not being set: absent or null on a create, absent or
/// lifted on an edit. A cap that is set has to be at least 1, since the
/// snapshot loader reads zero and below as "no cap", so storing one would look
/// like a limit while admitting everything. Whether any cap is left at all is
/// the store's to check: on an edit that depends on the row as it stands when
/// the edit lands, so [`RateLimitRepo::update`] checks it under its row lock,
/// and [`RateLimitRepo::create`] applies the same rule to a new row.
pub(super) fn validate_rate_limit_caps(rpm: Option<i32>, tpm: Option<i32>) -> ApiResult<()> {
    for (field, value) in [("rpm", rpm), ("tpm", tpm)] {
        if matches!(value, Some(cap) if cap < 1) {
            return Err(invalid_field(
                field,
                format!("{field} must be at least 1, or null for no cap"),
            ));
        }
    }
    Ok(())
}

/// Change a rate limit's caps in place (#1285), for the reason
/// [`update_budget`] gives: delete-and-recreate opened a window with no limit
/// at all.
async fn update_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateRateLimit>,
) -> ApiResult<Json<RateLimit>> {
    let existing = RateLimitRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &existing.scope_type,
        existing.scope_id,
        Within {
            org: existing.within_org_id,
            project: existing.within_project_id,
        },
    )
    .await?;
    let org_id = chain.org;
    authorize_cap(&state, &principal, chain, cap!("rate_limit", Update)).await?;
    if body.rpm.is_none() && body.tpm.is_none() {
        // nothing asked for, so nothing written, bumped or audited
        return Ok(Json(existing));
    }
    validate_rate_limit_caps(body.rpm.flatten(), body.tpm.flatten())?;
    let edit = RateLimitRepo(pool(&state))
        .update(id, body.rpm, body.tpm)
        .await?;
    if !edit.changed {
        // both caps already read as sent: nothing written, bumped or audited
        return Ok(Json(edit.after));
    }
    publish_config_change(&state).await?;
    let (before, row) = (edit.before, edit.after);
    let mut changes = serde_json::Map::new();
    if before.rpm != row.rpm {
        changes.insert(
            "rpm".into(),
            serde_json::json!({"from": before.rpm, "to": row.rpm}),
        );
    }
    if before.tpm != row.tpm {
        changes.insert(
            "tpm".into(),
            serde_json::json!({"from": before.tpm, "to": row.tpm}),
        );
    }
    log_audit(
        &state,
        &principal,
        org_id,
        "rate_limit.update",
        "rate_limit",
        id,
        serde_json::json!({
            "scope_type": row.scope_type,
            "scope_id": row.scope_id,
            "changes": changes,
        }),
    )
    .await;
    Ok(Json(row))
}

async fn delete_rate_limit(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let limit = RateLimitRepo(pool(&state)).get(id).await?;
    let chain = ScopeChain::from_cap(
        pool(&state),
        &limit.scope_type,
        limit.scope_id,
        Within {
            org: limit.within_org_id,
            project: limit.within_project_id,
        },
    )
    .await?;
    let org_id = chain.org;
    authorize_cap(&state, &principal, chain, cap!("rate_limit", Delete)).await?;
    RateLimitRepo(pool(&state)).delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        org_id,
        "rate_limit.delete",
        "rate_limit",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
