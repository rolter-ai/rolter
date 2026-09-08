//! Labels on providers, provider groups, routes and models (#985).
//!
//! Two surfaces over one table, because the subjects sit at different scopes:
//! `/api/v1/orgs/{org_id}/labels` covers the things an org owns, and
//! `/api/v1/model-labels` covers models, which belong to the deployment-wide
//! pricing catalog and not to any org. Both refuse to write anything but a
//! `custom` label — an auto label is an observation, and the only way to
//! change one is for the subsystem that made it to observe again.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::models::Label;
use rolter_store::postgres::repo::{CustomLabelInput, LabelFilter, LabelRepo};

use crate::crud::{log_audit, pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/labels",
            get(list_org_labels).post(create_org_label),
        )
        .route(
            "/api/v1/orgs/{org_id}/labels/{id}",
            axum::routing::put(update_org_label).delete(delete_org_label),
        )
        .route(
            "/api/v1/model-labels",
            get(list_model_labels).post(create_model_label),
        )
        .route(
            "/api/v1/model-labels/{id}",
            axum::routing::put(update_model_label).delete(delete_model_label),
        )
}

/// The narrowing a list request may apply. Absent fields mean "any", so the
/// bare path returns everything in scope.
#[derive(Debug, Deserialize)]
struct LabelQuery {
    subject_type: Option<String>,
    subject_id: Option<String>,
    key: Option<String>,
    value: Option<String>,
    source: Option<String>,
}

impl LabelQuery {
    fn as_filter(&self) -> LabelFilter<'_> {
        LabelFilter {
            subject_type: self.subject_type.as_deref(),
            subject_id: self.subject_id.as_deref(),
            key: self.key.as_deref(),
            value: self.value.as_deref(),
            source: self.source.as_deref(),
        }
    }

    /// A `source` the caller does not recognise would otherwise return an
    /// empty list and read as "no labels" rather than "bad request".
    fn validate(&self) -> ApiResult<()> {
        if let Some(source) = self.source.as_deref() {
            if !matches!(source, "auto" | "custom") {
                return Err(invalid("source must be auto or custom"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateOrgLabel {
    subject_type: String,
    subject_id: Uuid,
    key: String,
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateModelLabel {
    /// the model's name, which is how the pricing catalog addresses it
    model: String,
    key: String,
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateLabel {
    value: Option<String>,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

/// The same charset the `labels_key_charset` constraint enforces, checked here
/// so a bad key is a 400 naming the rule rather than a 500 from the database.
fn validate_key(key: &str) -> ApiResult<()> {
    if key.is_empty() || key.len() > 63 {
        return Err(invalid("key must be between 1 and 63 characters"));
    }
    let mut chars = key.chars();
    let first = chars.next().unwrap_or('-');
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(invalid("key must start with a lowercase letter or a digit"));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | ':' | '-'))
    {
        return Err(invalid(
            "key may contain lowercase letters, digits, and . _ : - only",
        ));
    }
    Ok(())
}

fn validate_value(value: Option<&str>) -> ApiResult<()> {
    match value {
        Some(value) if value.is_empty() || value.len() > 128 => Err(invalid(
            "value must be between 1 and 128 characters, or omitted entirely",
        )),
        _ => Ok(()),
    }
}

/// `Ok(None)` from the repository is the unique-constraint conflict; the
/// operator is being told the key is taken, not that the write failed.
fn created(row: Option<Label>, key: &str) -> ApiResult<(StatusCode, Json<Label>)> {
    match row {
        Some(row) => Ok((StatusCode::CREATED, Json(row))),
        None => Err(ApiError::Conflict(format!(
            "label '{key}' is already set on this subject"
        ))),
    }
}

async fn list_org_labels(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    Query(query): Query<LabelQuery>,
) -> ApiResult<Json<Vec<Label>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("label", Read),
    )
    .await?;
    query.validate()?;
    Ok(Json(
        LabelRepo(pool(&state))
            .list_in_org(org_id, query.as_filter())
            .await?,
    ))
}

async fn create_org_label(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateOrgLabel>,
) -> ApiResult<(StatusCode, Json<Label>)> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("label", Create),
    )
    .await?;
    if !matches!(
        body.subject_type.as_str(),
        "provider" | "provider_group" | "route"
    ) {
        return Err(invalid(
            "subject_type must be provider, provider_group or route; \
             label a model through /api/v1/model-labels",
        ));
    }
    validate_key(&body.key)?;
    validate_value(body.value.as_deref())?;
    // the authorization above is for the org in the path, so the subject has
    // to be shown to live in that org or the path becomes a way to label
    // another tenant's provider
    let repo = LabelRepo(pool(&state));
    if !repo
        .subject_in_org(&body.subject_type, body.subject_id, org_id)
        .await?
    {
        return Err(ApiError::Core(Error::NotFound(format!(
            "{} {} in org {org_id}",
            body.subject_type, body.subject_id
        ))));
    }
    let row = repo
        .create_custom(CustomLabelInput {
            subject_type: &body.subject_type,
            subject_id: &body.subject_id.to_string(),
            key: &body.key,
            value: body.value.as_deref(),
        })
        .await?;
    if let Some(ref row) = row {
        publish_config_change(&state).await?;
        log_audit(
            &state,
            &principal,
            Some(org_id),
            "label.create",
            "label",
            row.id,
            serde_json::json!({
                "subject_type": row.subject_type,
                "subject_id": row.subject_id,
                "key": row.key,
            }),
        )
        .await;
    }
    created(row, &body.key)
}

/// Load a label and confirm it belongs to `org_id`, so an id from another
/// tenant is a 404 rather than an edit.
async fn org_label(state: &ControlState, org_id: Uuid, id: Uuid) -> ApiResult<Label> {
    let repo = LabelRepo(pool(state));
    let label = repo.get(id).await?;
    let subject_id = Uuid::parse_str(&label.subject_id)
        .map_err(|_| Error::NotFound(format!("label {id} in org {org_id}")))?;
    if !repo
        .subject_in_org(&label.subject_type, subject_id, org_id)
        .await?
    {
        return Err(ApiError::Core(Error::NotFound(format!(
            "label {id} in org {org_id}"
        ))));
    }
    Ok(label)
}

async fn update_org_label(
    principal: Principal,
    State(state): State<ControlState>,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    SafeJson(body): SafeJson<UpdateLabel>,
) -> ApiResult<Json<Label>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("label", Update),
    )
    .await?;
    validate_value(body.value.as_deref())?;
    org_label(&state, org_id, id).await?;
    let row = LabelRepo(pool(&state))
        .update_custom(id, body.value.as_deref())
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "label.update",
        "label",
        id,
        serde_json::json!({"key": row.key, "value": row.value}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_org_label(
    principal: Principal,
    State(state): State<ControlState>,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<StatusCode> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("label", Delete),
    )
    .await?;
    org_label(&state, org_id, id).await?;
    LabelRepo(pool(&state)).delete_custom(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "label.delete",
        "label",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_model_labels(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<LabelQuery>,
) -> ApiResult<Json<Vec<Label>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_label", Read),
    )
    .await?;
    query.validate()?;
    let filter = LabelFilter {
        subject_type: Some("model"),
        ..query.as_filter()
    };
    Ok(Json(LabelRepo(pool(&state)).list(filter).await?))
}

async fn create_model_label(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<CreateModelLabel>,
) -> ApiResult<(StatusCode, Json<Label>)> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_label", Create),
    )
    .await?;
    if body.model.trim().is_empty() {
        return Err(invalid("model must not be empty"));
    }
    validate_key(&body.key)?;
    validate_value(body.value.as_deref())?;
    let row = LabelRepo(pool(&state))
        .create_custom(CustomLabelInput {
            subject_type: "model",
            subject_id: &body.model,
            key: &body.key,
            value: body.value.as_deref(),
        })
        .await?;
    if let Some(ref row) = row {
        publish_config_change(&state).await?;
        log_audit(
            &state,
            &principal,
            None,
            "model_label.create",
            "label",
            row.id,
            serde_json::json!({"model": row.subject_id, "key": row.key}),
        )
        .await;
    }
    created(row, &body.key)
}

/// Confirm the label is a model's before a model-scoped handler edits it: the
/// two surfaces share a table, and superadmin authority over the catalog is
/// not authority over an org's providers.
async fn model_label(state: &ControlState, id: Uuid) -> ApiResult<Label> {
    let label = LabelRepo(pool(state)).get(id).await?;
    if label.subject_type != "model" {
        return Err(ApiError::Core(Error::NotFound(format!("model label {id}"))));
    }
    Ok(label)
}

async fn update_model_label(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateLabel>,
) -> ApiResult<Json<Label>> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_label", Update),
    )
    .await?;
    validate_value(body.value.as_deref())?;
    model_label(&state, id).await?;
    let row = LabelRepo(pool(&state))
        .update_custom(id, body.value.as_deref())
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "model_label.update",
        "label",
        id,
        serde_json::json!({"model": row.subject_id, "key": row.key, "value": row.value}),
    )
    .await;
    Ok(Json(row))
}

async fn delete_model_label(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize(
        &state,
        &principal,
        ScopeChain::default(),
        cap!("model_label", Delete),
    )
    .await?;
    model_label(&state, id).await?;
    LabelRepo(pool(&state)).delete_custom(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "model_label.delete",
        "label",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
