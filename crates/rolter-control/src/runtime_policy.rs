//! Global runtime-policy API for retry/timeout/queue controls.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rolter_core::{BackpressurePolicy, Error};
use rolter_store::postgres::models::RuntimePolicy;
use rolter_store::postgres::repo::{AuditLogRepo, RuntimePolicyRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/runtime-policy",
        get(get_runtime_policy).put(update_runtime_policy),
    )
}

async fn get_runtime_policy(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<RuntimePolicy>> {
    authorize_superadmin(&principal, superadmin_cap!("runtime_policy", Read))?;
    Ok(Json(RuntimePolicyRepo(pool(&state)).get().await?))
}

#[derive(Deserialize)]
struct UpdateRuntimePolicy {
    retry_max_retries: i32,
    retry_base_ms: i32,
    retry_max_ms: i32,
    timeout_connect_s: i32,
    timeout_request_s: i32,
    queue_enabled: bool,
    queue_capacity: i32,
    queue_workers: i32,
    queue_backpressure: BackpressurePolicy,
    queue_block_ms: i32,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_policy(body: &UpdateRuntimePolicy) -> ApiResult<()> {
    if !(0..=10).contains(&body.retry_max_retries) {
        return Err(invalid("retry_max_retries must be between 0 and 10"));
    }
    if !(0..=60_000).contains(&body.retry_base_ms) {
        return Err(invalid("retry_base_ms must be between 0 and 60000"));
    }
    if !(0..=600_000).contains(&body.retry_max_ms) {
        return Err(invalid("retry_max_ms must be between 0 and 600000"));
    }
    if body.retry_max_ms < body.retry_base_ms {
        return Err(invalid(
            "retry_max_ms must be greater than or equal to retry_base_ms",
        ));
    }
    if !(0..=300).contains(&body.timeout_connect_s) {
        return Err(invalid("timeout_connect_s must be between 0 and 300"));
    }
    if !(0..=3_600).contains(&body.timeout_request_s) {
        return Err(invalid("timeout_request_s must be between 0 and 3600"));
    }
    if !(1..=100_000).contains(&body.queue_capacity) {
        return Err(invalid("queue_capacity must be between 1 and 100000"));
    }
    if !(1..=2_048).contains(&body.queue_workers) {
        return Err(invalid("queue_workers must be between 1 and 2048"));
    }
    if !(0..=120_000).contains(&body.queue_block_ms) {
        return Err(invalid("queue_block_ms must be between 0 and 120000"));
    }
    if body.queue_backpressure == BackpressurePolicy::Block && body.queue_block_ms == 0 {
        return Err(invalid(
            "queue_block_ms must be greater than zero when queue_backpressure is block",
        ));
    }
    Ok(())
}

async fn update_runtime_policy(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateRuntimePolicy>,
) -> ApiResult<Json<RuntimePolicy>> {
    authorize_superadmin(&principal, superadmin_cap!("runtime_policy", Update))?;
    validate_policy(&body)?;
    let row = RuntimePolicyRepo(pool(&state))
        .update(
            body.retry_max_retries,
            body.retry_base_ms,
            body.retry_max_ms,
            body.timeout_connect_s,
            body.timeout_request_s,
            body.queue_enabled,
            body.queue_capacity,
            body.queue_workers,
            match body.queue_backpressure {
                BackpressurePolicy::Drop => "drop",
                BackpressurePolicy::Block => "block",
                BackpressurePolicy::Error => "error",
            },
            body.queue_block_ms,
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
            "runtime_policy.update",
            Some("runtime_policy"),
            None,
            Some(serde_json::json!({
                "retry_max_retries": row.retry_max_retries,
                "retry_base_ms": row.retry_base_ms,
                "retry_max_ms": row.retry_max_ms,
                "timeout_connect_s": row.timeout_connect_s,
                "timeout_request_s": row.timeout_request_s,
                "queue_enabled": row.queue_enabled,
                "queue_capacity": row.queue_capacity,
                "queue_workers": row.queue_workers,
                "queue_backpressure": row.queue_backpressure,
                "queue_block_ms": row.queue_block_ms,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write runtime policy audit log");
    }
    Ok(Json(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_runtime_limits() {
        let body = UpdateRuntimePolicy {
            retry_max_retries: -1,
            retry_base_ms: 100,
            retry_max_ms: 50,
            timeout_connect_s: 10,
            timeout_request_s: 60,
            queue_enabled: true,
            queue_capacity: 0,
            queue_workers: 0,
            queue_backpressure: BackpressurePolicy::Block,
            queue_block_ms: 0,
        };
        assert!(validate_policy(&body).is_err());
    }
}
