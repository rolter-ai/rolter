//! Global cross-dialect compatibility API: the values applied when a request
//! is translated between the OpenAI and Anthropic wire formats (#546).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rolter_core::Error;
use rolter_store::postgres::models::CompatibilityPolicy;
use rolter_store::postgres::repo::{AuditLogRepo, CompatibilityPolicyRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/compatibility-policy",
        get(get_compatibility_policy).put(update_compatibility_policy),
    )
}

/// The stored policy plus whether applying it needs a gateway restart. Both
/// fields reach the data plane through the snapshot, so neither does.
#[derive(Debug, Serialize)]
struct CompatibilityPolicyView {
    #[serde(flatten)]
    policy: CompatibilityPolicy,
    /// fields that only take effect after a gateway restart; empty today
    restart_required: Vec<&'static str>,
}

impl From<CompatibilityPolicy> for CompatibilityPolicyView {
    fn from(policy: CompatibilityPolicy) -> Self {
        Self {
            policy,
            restart_required: Vec::new(),
        }
    }
}

async fn get_compatibility_policy(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<CompatibilityPolicyView>> {
    authorize_superadmin(&principal, superadmin_cap!("compatibility_policy", Read))?;
    Ok(Json(
        CompatibilityPolicyRepo(pool(&state)).get().await?.into(),
    ))
}

#[derive(Debug, Deserialize)]
struct UpdateCompatibilityPolicy {
    anthropic_version: String,
    default_max_tokens: i32,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

/// Reject anything the gateway would send upstream verbatim. The version is a
/// dated Anthropic release string; anything else would make every Anthropic
/// call fail at the edge with an opaque upstream error.
fn validate_policy(body: &UpdateCompatibilityPolicy) -> ApiResult<()> {
    let version = body.anthropic_version.as_bytes();
    let dated = version.len() == 10
        && version.iter().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if !dated {
        return Err(invalid(
            "anthropic_version must be a dated release like '2023-06-01'",
        ));
    }
    if !(1..=1_000_000).contains(&body.default_max_tokens) {
        return Err(invalid("default_max_tokens must be between 1 and 1000000"));
    }
    Ok(())
}

async fn update_compatibility_policy(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateCompatibilityPolicy>,
) -> ApiResult<Json<CompatibilityPolicyView>> {
    authorize_superadmin(&principal, superadmin_cap!("compatibility_policy", Update))?;
    validate_policy(&body)?;
    let row = CompatibilityPolicyRepo(pool(&state))
        .update(&body.anthropic_version, body.default_max_tokens)
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
            "compatibility_policy.update",
            Some("compatibility_policy"),
            None,
            Some(serde_json::json!({
                "anthropic_version": row.anthropic_version,
                "default_max_tokens": row.default_max_tokens,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write compatibility policy audit log");
    }
    Ok(Json(row.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_dated_anthropic_version() {
        let body = UpdateCompatibilityPolicy {
            anthropic_version: "2023-06-01".to_string(),
            default_max_tokens: 1024,
        };
        assert!(validate_policy(&body).is_ok());
    }

    #[test]
    fn rejects_free_form_version_or_out_of_range_tokens() {
        for version in ["latest", "2023-6-1", "2023-06-01T00", ""] {
            let body = UpdateCompatibilityPolicy {
                anthropic_version: version.to_string(),
                default_max_tokens: 1024,
            };
            assert!(validate_policy(&body).is_err(), "accepted {version:?}");
        }
        let body = UpdateCompatibilityPolicy {
            anthropic_version: "2023-06-01".to_string(),
            default_max_tokens: 0,
        };
        assert!(validate_policy(&body).is_err());
    }
}
