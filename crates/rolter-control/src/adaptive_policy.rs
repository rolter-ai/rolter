//! Global adaptive-routing API: the kill switch and blend weights every route
//! using the `adaptive` strategy is governed by (#544).

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rolter_core::{Error, MAX_EXPLORATION_RATIO};
use rolter_store::postgres::models::AdaptiveRoutingPolicy;
use rolter_store::postgres::repo::{AdaptiveRoutingPolicyRepo, AuditLogRepo, RouteRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

/// Upper bound on a blend weight. The blend is a weighted sum of signals each
/// scored in `[0, 1]`, so only the ratio between weights matters; the bound
/// exists to keep a typo from making one signal numerically dominant.
const MAX_WEIGHT: f32 = 100.0;
/// Upper bound on the warm-up window. Anything larger is indistinguishable
/// from "never engage", which the kill switch already expresses.
const MAX_MIN_SAMPLES: i32 = 1_000_000;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/adaptive-routing-policy",
        get(get_policy).put(update_policy),
    )
}

/// The stored policy plus the routes it currently governs, so an operator can
/// see the blast radius of the kill switch before flipping it.
#[derive(Debug, Serialize)]
struct AdaptiveRoutingPolicyView {
    #[serde(flatten)]
    policy: AdaptiveRoutingPolicy,
    /// public model names of routes on the `adaptive` strategy
    affected_routes: Vec<String>,
}

async fn get_policy(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<AdaptiveRoutingPolicyView>> {
    authorize_superadmin(&principal, superadmin_cap!("adaptive_routing_policy", Read))?;
    let policy = AdaptiveRoutingPolicyRepo(pool(&state)).get().await?;
    Ok(Json(AdaptiveRoutingPolicyView {
        policy,
        affected_routes: adaptive_routes(&state).await?,
    }))
}

#[derive(Debug, Deserialize)]
struct UpdateAdaptiveRoutingPolicy {
    enabled: bool,
    latency_weight: f32,
    cost_weight: f32,
    load_weight: f32,
    exploration_ratio: f32,
    min_samples: i32,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

/// Reject anything the gateway would have to clamp or reinterpret. The kill
/// switch is the intended way to stop adaptive routing; an all-zero blend or a
/// runaway exploration ratio would stop it by turning the strategy into a
/// random balancer, which is a much less obvious thing to read off a dashboard.
fn validate(body: &UpdateAdaptiveRoutingPolicy) -> ApiResult<()> {
    for (name, value) in [
        ("latency_weight", body.latency_weight),
        ("cost_weight", body.cost_weight),
        ("load_weight", body.load_weight),
    ] {
        if !value.is_finite() || !(0.0..=MAX_WEIGHT).contains(&value) {
            return Err(invalid(format!(
                "{name} must be between 0 and {MAX_WEIGHT}"
            )));
        }
    }
    if body.latency_weight <= 0.0 && body.cost_weight <= 0.0 && body.load_weight <= 0.0 {
        return Err(invalid(
            "at least one of latency_weight, cost_weight or load_weight must be positive; \
             use enabled = false to stop adaptive routing",
        ));
    }
    if !body.exploration_ratio.is_finite()
        || !(0.0..=MAX_EXPLORATION_RATIO).contains(&body.exploration_ratio)
    {
        return Err(invalid(format!(
            "exploration_ratio must be between 0 and {MAX_EXPLORATION_RATIO}"
        )));
    }
    if !(0..=MAX_MIN_SAMPLES).contains(&body.min_samples) {
        return Err(invalid(format!(
            "min_samples must be between 0 and {MAX_MIN_SAMPLES}"
        )));
    }
    Ok(())
}

async fn update_policy(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateAdaptiveRoutingPolicy>,
) -> ApiResult<Json<AdaptiveRoutingPolicyView>> {
    authorize_superadmin(
        &principal,
        superadmin_cap!("adaptive_routing_policy", Update),
    )?;
    validate(&body)?;
    let policy = AdaptiveRoutingPolicyRepo(pool(&state))
        .update(
            body.enabled,
            body.latency_weight,
            body.cost_weight,
            body.load_weight,
            body.exploration_ratio,
            body.min_samples,
        )
        .await?;
    publish_config_change(&state).await?;
    let affected_routes = adaptive_routes(&state).await?;
    let actor = principal.account_id();
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "adaptive_routing_policy.update",
            Some("adaptive_routing_policy"),
            None,
            Some(serde_json::json!({
                "enabled": policy.enabled,
                "latency_weight": policy.latency_weight,
                "cost_weight": policy.cost_weight,
                "load_weight": policy.load_weight,
                "exploration_ratio": policy.exploration_ratio,
                "min_samples": policy.min_samples,
                "affected_routes": affected_routes,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write adaptive routing policy audit log");
    }
    Ok(Json(AdaptiveRoutingPolicyView {
        policy,
        affected_routes,
    }))
}

/// Public model names of every enabled route on the `adaptive` strategy.
async fn adaptive_routes(state: &ControlState) -> ApiResult<Vec<String>> {
    Ok(RouteRepo(pool(state))
        .list_by_strategy("adaptive")
        .await?
        .into_iter()
        .map(|route| route.model)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> UpdateAdaptiveRoutingPolicy {
        UpdateAdaptiveRoutingPolicy {
            enabled: true,
            latency_weight: 1.0,
            cost_weight: 0.5,
            load_weight: 0.25,
            exploration_ratio: 0.05,
            min_samples: 50,
        }
    }

    #[test]
    fn accepts_the_default_blend() {
        assert!(validate(&body()).is_ok());
    }

    #[test]
    fn rejects_an_all_zero_blend() {
        let flat = UpdateAdaptiveRoutingPolicy {
            latency_weight: 0.0,
            cost_weight: 0.0,
            load_weight: 0.0,
            ..body()
        };
        assert!(validate(&flat).is_err());
        // a single positive weight is enough
        let one = UpdateAdaptiveRoutingPolicy {
            cost_weight: 1.0,
            ..flat
        };
        assert!(validate(&one).is_ok());
    }

    #[test]
    fn rejects_out_of_range_values() {
        for bad in [
            UpdateAdaptiveRoutingPolicy {
                latency_weight: -1.0,
                ..body()
            },
            UpdateAdaptiveRoutingPolicy {
                latency_weight: f32::NAN,
                ..body()
            },
            UpdateAdaptiveRoutingPolicy {
                cost_weight: 1_000.0,
                ..body()
            },
            UpdateAdaptiveRoutingPolicy {
                exploration_ratio: 0.9,
                ..body()
            },
            UpdateAdaptiveRoutingPolicy {
                exploration_ratio: -0.1,
                ..body()
            },
            UpdateAdaptiveRoutingPolicy {
                min_samples: -1,
                ..body()
            },
            UpdateAdaptiveRoutingPolicy {
                min_samples: MAX_MIN_SAMPLES + 1,
                ..body()
            },
        ] {
            assert!(validate(&bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn a_disabled_policy_still_has_to_be_well_formed() {
        // switching the kill switch off does not license nonsense weights that
        // would take effect the moment it is switched back on
        let off = UpdateAdaptiveRoutingPolicy {
            enabled: false,
            exploration_ratio: 5.0,
            ..body()
        };
        assert!(validate(&off).is_err());
    }
}
