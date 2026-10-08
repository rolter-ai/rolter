//! Global model-defaults API: the inference parameters the gateway fills in
//! when a client omits them (#564).
//!
//! Defaults only ever fill a gap. A value the client sent is never overwritten,
//! so turning this on cannot change the meaning of a request that was already
//! explicit — which is what makes it safe to enable on a live deployment.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rolter_core::Error;
use rolter_store::postgres::models::ModelDefaults;
use rolter_store::postgres::repo::{AuditLogRepo, ModelDefaultsRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/model-defaults",
        get(get_model_defaults).put(update_model_defaults),
    )
}

async fn get_model_defaults(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<ModelDefaults>> {
    authorize_superadmin(&principal, superadmin_cap!("model_defaults", Read))?;
    Ok(Json(ModelDefaultsRepo(pool(&state)).get().await?))
}

#[derive(Debug, Deserialize)]
struct UpdateModelDefaults {
    enabled: bool,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    default_temperature: Option<f64>,
    #[serde(default)]
    default_top_p: Option<f64>,
    #[serde(default)]
    default_max_tokens: Option<i32>,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_defaults(body: &UpdateModelDefaults) -> ApiResult<()> {
    if let Some(model) = body.default_model.as_deref() {
        let model = model.trim();
        if model.is_empty() {
            return Err(invalid(
                "default_model must be a model name or omitted entirely",
            ));
        }
        if model.len() > 256 {
            return Err(invalid("default_model must be at most 256 characters"));
        }
    }
    // NaN compares false against every bound, so check it explicitly rather
    // than letting it slip through the range test and reach a provider
    if let Some(temperature) = body.default_temperature {
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err(invalid("default_temperature must be between 0 and 2"));
        }
    }
    if let Some(top_p) = body.default_top_p {
        if !top_p.is_finite() || !(0.0..=1.0).contains(&top_p) {
            return Err(invalid("default_top_p must be between 0 and 1"));
        }
    }
    if let Some(max_tokens) = body.default_max_tokens {
        if !(1..=1_000_000).contains(&max_tokens) {
            return Err(invalid("default_max_tokens must be between 1 and 1000000"));
        }
    }
    Ok(())
}

async fn update_model_defaults(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateModelDefaults>,
) -> ApiResult<Json<ModelDefaults>> {
    authorize_superadmin(&principal, superadmin_cap!("model_defaults", Update))?;
    validate_defaults(&body)?;
    let model = body
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let row = ModelDefaultsRepo(pool(&state))
        .update(
            body.enabled,
            model,
            body.default_temperature,
            body.default_top_p,
            body.default_max_tokens,
        )
        .await?;
    publish_config_change(&state).await?;
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin { .. } => None,
    };
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "model_defaults.update",
            Some("model_defaults"),
            None,
            Some(serde_json::json!({
                "enabled": row.enabled,
                "default_model": row.default_model,
                "default_temperature": row.default_temperature,
                "default_top_p": row.default_top_p,
                "default_max_tokens": row.default_max_tokens,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write model defaults audit log");
    }
    Ok(Json(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> UpdateModelDefaults {
        UpdateModelDefaults {
            enabled: true,
            default_model: Some("gpt-4o-mini".to_string()),
            default_temperature: Some(0.7),
            default_top_p: Some(0.9),
            default_max_tokens: Some(1024),
        }
    }

    #[test]
    fn accepts_in_range_defaults_and_an_all_empty_body() {
        assert!(validate_defaults(&body()).is_ok());
        assert!(validate_defaults(&UpdateModelDefaults {
            enabled: false,
            default_model: None,
            default_temperature: None,
            default_top_p: None,
            default_max_tokens: None,
        })
        .is_ok());
    }

    #[test]
    fn rejects_out_of_range_and_non_finite_parameters() {
        for temperature in [-0.1, 2.1, f64::NAN, f64::INFINITY] {
            let mut b = body();
            b.default_temperature = Some(temperature);
            assert!(validate_defaults(&b).is_err(), "accepted {temperature}");
        }
        let mut top_p = body();
        top_p.default_top_p = Some(1.5);
        assert!(validate_defaults(&top_p).is_err());

        let mut tokens = body();
        tokens.default_max_tokens = Some(0);
        assert!(validate_defaults(&tokens).is_err());
    }

    #[test]
    fn rejects_a_blank_model_name() {
        let mut b = body();
        b.default_model = Some("   ".to_string());
        assert!(validate_defaults(&b).is_err());
    }
}
