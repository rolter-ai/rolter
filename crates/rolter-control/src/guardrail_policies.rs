//! Scoped semantic guardrail policies (#459).
//!
//! A policy scoped to the deployment, an organization, a project, a route or a
//! virtual key names the guardrail providers that judge the requests it covers,
//! in order, and the thresholds their scored categories are held to. The
//! gateway applies the most specific enabled policy (key, route, project, org,
//! deployment); see `rolter_core::semantic_guards`. Every write is validated
//! here, stored with its provider list in one transaction, and reaches the
//! gateways through the snapshot.

use std::collections::{BTreeMap, HashSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use rolter_core::semantic_guards::policy_field_problems;
use rolter_core::PolicyScope;
use rolter_store::postgres::models::GuardrailPolicyRow;
use rolter_store::postgres::repo::{GuardrailPolicyInput, GuardrailRepo};

use crate::crud::{
    invalid_field, log_audit, pool, publish_config_change, ApiError, ApiResult, SafeJson,
};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

/// The longest description a policy may carry.
const MAX_DESCRIPTION_CHARS: usize = 500;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/guardrails/policies",
            get(list_policies).post(create_policy),
        )
        .route(
            "/api/v1/guardrails/policies/{id}",
            axum::routing::put(update_policy).delete(delete_policy),
        )
}

#[derive(Debug, Deserialize)]
struct PolicyBody {
    name: String,
    enabled: bool,
    scope_kind: String,
    #[serde(default)]
    scope_id: Option<String>,
    #[serde(default)]
    provider_ids: Vec<Uuid>,
    #[serde(default)]
    thresholds: BTreeMap<String, f64>,
    #[serde(default = "default_on_violation")]
    on_violation: String,
    #[serde(default = "default_streaming")]
    streaming: String,
    #[serde(default)]
    description: Option<String>,
}

fn default_on_violation() -> String {
    "block".to_string()
}

fn default_streaming() -> String {
    "reject".to_string()
}

impl PolicyBody {
    /// The scope id as stored: trimmed, and absent when empty.
    fn scope_id(&self) -> Option<&str> {
        self.scope_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
    }

    fn description(&self) -> Option<&str> {
        self.description
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    }
}

/// Check a body against everything the store's constraints cannot say,
/// returning the thresholds as the JSON the row keeps.
fn validate(
    body: &PolicyBody,
    known_providers: &HashSet<Uuid>,
    file_policies: &HashSet<String>,
) -> ApiResult<serde_json::Value> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(invalid_field("name", "a policy needs a name"));
    }
    if file_policies.contains(name) {
        return Err(ApiError::Conflict(format!(
            "the guardrail policy '{name}' is owned by the config file"
        )));
    }
    let Some(scope) = PolicyScope::parse(&body.scope_kind) else {
        return Err(invalid_field(
            "scope_kind",
            "scope_kind must be deployment, org, project, route or key",
        ));
    };
    if let Some((field, problem)) = policy_field_problems(scope, body.scope_id(), &body.thresholds)
        .into_iter()
        .next()
    {
        return Err(invalid_field(field, problem));
    }
    let mut seen = HashSet::new();
    for id in &body.provider_ids {
        if !seen.insert(id) {
            return Err(invalid_field(
                "provider_ids",
                format!("provider {id} is listed twice"),
            ));
        }
        if !known_providers.contains(id) {
            return Err(invalid_field(
                "provider_ids",
                format!("no guardrail provider has the id {id}"),
            ));
        }
    }
    if !matches!(body.on_violation.as_str(), "block" | "annotate") {
        return Err(invalid_field(
            "on_violation",
            "on_violation must be block or annotate",
        ));
    }
    if !matches!(body.streaming.as_str(), "reject" | "buffer") {
        return Err(invalid_field(
            "streaming",
            "streaming must be reject or buffer",
        ));
    }
    if body
        .description()
        .is_some_and(|text| text.chars().count() > MAX_DESCRIPTION_CHARS)
    {
        return Err(invalid_field(
            "description",
            format!("a description is at most {MAX_DESCRIPTION_CHARS} characters"),
        ));
    }
    serde_json::to_value(&body.thresholds)
        .map_err(|_| invalid_field("thresholds", "thresholds must be numbers between 0 and 1"))
}

impl PolicyBody {
    fn as_input<'a>(&'a self, thresholds: &'a serde_json::Value) -> GuardrailPolicyInput<'a> {
        GuardrailPolicyInput {
            name: self.name.trim(),
            enabled: self.enabled,
            scope_kind: &self.scope_kind,
            scope_id: self.scope_id(),
            provider_ids: &self.provider_ids,
            thresholds,
            on_violation: &self.on_violation,
            streaming: &self.streaming,
            description: self.description(),
        }
    }
}

async fn checked(state: &ControlState, body: &PolicyBody) -> ApiResult<serde_json::Value> {
    let known: HashSet<Uuid> = GuardrailRepo(pool(state))
        .list_providers()
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    validate(body, &known, &state.config_owned.semantic_policies)
}

fn audit_detail(row: &GuardrailPolicyRow) -> serde_json::Value {
    serde_json::json!({
        "name": row.name,
        "enabled": row.enabled,
        "scope_kind": row.scope_kind,
        "scope_id": row.scope_id,
        "providers": row.provider_ids.len(),
    })
}

async fn list_policies(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<GuardrailPolicyRow>>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_policy", Read))?;
    Ok(Json(GuardrailRepo(pool(&state)).list_policies().await?))
}

async fn create_policy(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<PolicyBody>,
) -> ApiResult<(StatusCode, Json<GuardrailPolicyRow>)> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_policy", Create))?;
    let thresholds = checked(&state, &body).await?;
    let row = GuardrailRepo(pool(&state))
        .create_policy(body.as_input(&thresholds))
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_policy.create",
        "guardrail_policy",
        row.id,
        audit_detail(&row),
    )
    .await;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_policy(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<PolicyBody>,
) -> ApiResult<Json<GuardrailPolicyRow>> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_policy", Update))?;
    let thresholds = checked(&state, &body).await?;
    let row = GuardrailRepo(pool(&state))
        .update_policy(id, body.as_input(&thresholds))
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_policy.update",
        "guardrail_policy",
        id,
        audit_detail(&row),
    )
    .await;
    Ok(Json(row))
}

async fn delete_policy(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("guardrail_policy", Delete))?;
    GuardrailRepo(pool(&state)).delete_policy(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        None,
        "guardrail_policy.delete",
        "guardrail_policy",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> PolicyBody {
        PolicyBody {
            name: "support".into(),
            enabled: true,
            scope_kind: "project".into(),
            scope_id: Some(" p-1 ".into()),
            provider_ids: Vec::new(),
            thresholds: BTreeMap::from([("violence".to_string(), 0.8)]),
            on_violation: "block".into(),
            streaming: "reject".into(),
            description: Some("  ".into()),
        }
    }

    fn field_of(result: ApiResult<serde_json::Value>) -> String {
        match result {
            Err(ApiError::InvalidField { field, .. }) => field,
            other => panic!("expected an invalid field, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_body_stores_trimmed_values() {
        let body = body();
        let thresholds = validate(&body, &HashSet::new(), &HashSet::new()).unwrap();
        let input = body.as_input(&thresholds);
        assert_eq!(input.scope_id, Some("p-1"));
        assert_eq!(input.description, None);
        assert_eq!(thresholds, serde_json::json!({"violence": 0.8}));
    }

    #[test]
    fn every_field_is_checked() {
        let none: HashSet<Uuid> = HashSet::new();
        let names: HashSet<String> = HashSet::new();
        let mut b = body();
        b.scope_kind = "team".into();
        assert_eq!(field_of(validate(&b, &none, &names)), "scope_kind");
        let mut b = body();
        b.scope_id = None;
        assert_eq!(field_of(validate(&b, &none, &names)), "scope_id");
        let mut b = body();
        b.scope_kind = "deployment".into();
        assert_eq!(field_of(validate(&b, &none, &names)), "scope_id");
        let mut b = body();
        b.thresholds.insert("hate".into(), 1.2);
        assert_eq!(field_of(validate(&b, &none, &names)), "thresholds");
        let mut b = body();
        b.provider_ids = vec![Uuid::new_v4()];
        assert_eq!(field_of(validate(&b, &none, &names)), "provider_ids");
        let known = Uuid::new_v4();
        let mut b = body();
        b.provider_ids = vec![known, known];
        assert_eq!(
            field_of(validate(&b, &HashSet::from([known]), &names)),
            "provider_ids"
        );
        let mut b = body();
        b.on_violation = "redact".into();
        assert_eq!(field_of(validate(&b, &none, &names)), "on_violation");
        let mut b = body();
        b.streaming = "passthrough".into();
        assert_eq!(field_of(validate(&b, &none, &names)), "streaming");
        let mut b = body();
        b.name = " ".into();
        assert_eq!(field_of(validate(&b, &none, &names)), "name");
    }

    #[test]
    fn a_name_the_file_owns_is_refused() {
        let owned = HashSet::from(["support".to_string()]);
        assert!(matches!(
            validate(&body(), &HashSet::new(), &owned),
            Err(ApiError::Conflict(_))
        ));
    }
}
