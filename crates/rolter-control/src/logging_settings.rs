//! Global logging-policy API for request-log sampling, payload capture,
//! retention and the dashboard UX event opt-out.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rolter_core::Error;
use rolter_store::postgres::models::LoggingSettings;
use rolter_store::postgres::repo::{AuditLogRepo, LoggingSettingsRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/logging-settings",
        get(get_logging_settings).put(update_logging_settings),
    )
}

async fn get_logging_settings(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<LoggingSettings>> {
    authorize_superadmin(&principal, superadmin_cap!("logging_settings", Read))?;
    Ok(Json(LoggingSettingsRepo(pool(&state)).get().await?))
}

#[derive(Deserialize)]
struct UpdateLoggingSettings {
    sample_rate: f64,
    payload_capture_enabled: bool,
    payload_capture_max_bytes: i32,
    #[serde(default)]
    payload_capture_redact_fields: Vec<String>,
    #[serde(default)]
    payload_capture_models: Vec<String>,
    #[serde(default)]
    payload_capture_virtual_key_ids: Vec<String>,
    /// how long request-log metadata is kept, in days
    #[serde(default = "default_retention_days")]
    retention_days: i32,
    /// how long captured raw payloads are kept, in hours
    #[serde(default = "default_payload_retention_hours")]
    payload_retention_hours: i32,
    /// whether dashboard UX events are accepted; absent keeps the stored value
    /// so a client that predates the field cannot re-enable a stream an
    /// operator switched off
    #[serde(default)]
    ui_events: Option<bool>,
}

fn default_retention_days() -> i32 {
    90
}

fn default_payload_retention_hours() -> i32 {
    168
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_settings(body: &UpdateLoggingSettings) -> ApiResult<()> {
    if !body.sample_rate.is_finite() || !(0.0..=1.0).contains(&body.sample_rate) {
        return Err(invalid("sample_rate must be in [0, 1]"));
    }
    if !(0..=1_048_576).contains(&body.payload_capture_max_bytes) {
        return Err(invalid(
            "payload_capture_max_bytes must be between 0 and 1048576",
        ));
    }
    if body.payload_capture_redact_fields.iter().any(|field| {
        field.trim().is_empty() || field.len() > 128 || field.chars().any(char::is_control)
    }) {
        return Err(invalid(
            "payload_capture_redact_fields must contain 1-128 visible-character keys",
        ));
    }
    if body.payload_capture_models.iter().any(|model| {
        model.trim().is_empty() || model.len() > 255 || model.chars().any(char::is_control)
    }) {
        return Err(invalid(
            "payload_capture_models must contain 1-255 visible-character model names",
        ));
    }
    if body
        .payload_capture_virtual_key_ids
        .iter()
        .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(invalid(
            "payload_capture_virtual_key_ids must contain valid UUIDs",
        ));
    }
    if !(1..=3650).contains(&body.retention_days) {
        return Err(invalid("retention_days must be between 1 and 3650"));
    }
    if !(1..=8760).contains(&body.payload_retention_hours) {
        return Err(invalid(
            "payload_retention_hours must be between 1 and 8760",
        ));
    }
    // raw bodies are the sensitive half; keeping them past the metadata they
    // belong to would leak prompt content the operator meant to expire
    if i64::from(body.payload_retention_hours) > i64::from(body.retention_days) * 24 {
        return Err(invalid(
            "payload_retention_hours must not exceed retention_days",
        ));
    }
    Ok(())
}

async fn update_logging_settings(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateLoggingSettings>,
) -> ApiResult<Json<LoggingSettings>> {
    authorize_superadmin(&principal, superadmin_cap!("logging_settings", Update))?;
    validate_settings(&body)?;
    let row = LoggingSettingsRepo(pool(&state))
        .update(
            body.sample_rate,
            body.payload_capture_enabled,
            body.payload_capture_max_bytes,
            &body.payload_capture_redact_fields,
            &body.payload_capture_models,
            &body.payload_capture_virtual_key_ids,
            body.retention_days,
            body.payload_retention_hours,
            body.ui_events,
        )
        .await?;
    publish_config_change(&state).await?;
    // clickhouse owns expiry; a failure here leaves the stored policy in place
    // and is surfaced in logs rather than failing the admin write
    if let Some(clickhouse) = &state.clickhouse {
        if let Err(err) = clickhouse
            .apply_log_retention(
                row.retention_days as u32,
                row.payload_retention_hours as u32,
            )
            .await
        {
            tracing::warn!(error = %err, "failed to apply log retention to clickhouse");
        }
    }
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "logging_settings.update",
            Some("logging_settings"),
            None,
            Some(serde_json::json!({
                "sample_rate": row.sample_rate,
                "payload_capture_enabled": row.payload_capture_enabled,
                "payload_capture_max_bytes": row.payload_capture_max_bytes,
                "payload_capture_redact_field_count": row.payload_capture_redact_fields.len(),
                "payload_capture_model_count": row.payload_capture_models.len(),
                "payload_capture_virtual_key_id_count": row.payload_capture_virtual_key_ids.len(),
                "retention_days": row.retention_days,
                "payload_retention_hours": row.payload_retention_hours,
                "ui_events": row.ui_events,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write logging settings audit log");
    }
    Ok(Json(row))
}

/// Push the stored retention policy at ClickHouse once, at startup.
///
/// The TTL is otherwise only written by [`update_logging_settings`], and that
/// call deliberately does not fail the admin write when ClickHouse is
/// unreachable — losing the policy because the log store was briefly down is
/// worse than a stale TTL. The cost is that the two can diverge permanently:
/// once the PUT has returned, nothing ever retries. A ClickHouse restored from
/// a backup, or provisioned after the policy was set, keeps the schema default
/// (90 days / 7 days) while the dashboard reports whatever the operator chose.
///
/// Reconciling at boot converges that. `alter table … modify ttl` is
/// idempotent, so re-applying an unchanged policy is a no-op.
#[cfg(feature = "postgres")]
pub(crate) async fn reconcile_retention(state: &ControlState) {
    let Some(clickhouse) = state.clickhouse.clone() else {
        return;
    };
    let settings = match LoggingSettingsRepo(pool(state)).get().await {
        Ok(settings) => settings,
        Err(err) => {
            tracing::warn!(error = %err, "could not read logging settings to reconcile log retention");
            return;
        }
    };
    match clickhouse
        .apply_log_retention(
            settings.retention_days as u32,
            settings.payload_retention_hours as u32,
        )
        .await
    {
        Ok(()) => tracing::info!(
            retention_days = settings.retention_days,
            payload_retention_hours = settings.payload_retention_hours,
            "reconciled clickhouse log retention with the stored policy"
        ),
        // startup must not depend on clickhouse being up; the next successful
        // settings write reconciles, as does the next restart
        Err(err) => {
            tracing::warn!(error = %err, "could not reconcile clickhouse log retention at startup")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(retention_days: i32, payload_retention_hours: i32) -> UpdateLoggingSettings {
        UpdateLoggingSettings {
            sample_rate: 1.0,
            payload_capture_enabled: false,
            payload_capture_max_bytes: 32768,
            payload_capture_redact_fields: vec!["api_key".to_string()],
            payload_capture_models: Vec::new(),
            payload_capture_virtual_key_ids: Vec::new(),
            retention_days,
            payload_retention_hours,
            ui_events: None,
        }
    }

    #[test]
    fn accepts_retention_within_bounds() {
        assert!(validate_settings(&settings(90, 168)).is_ok());
        assert!(validate_settings(&settings(1, 24)).is_ok());
    }

    #[test]
    fn rejects_out_of_range_or_outliving_retention() {
        assert!(validate_settings(&settings(0, 24)).is_err());
        assert!(validate_settings(&settings(4000, 24)).is_err());
        assert!(validate_settings(&settings(90, 0)).is_err());
        // payloads must never outlive the metadata they belong to
        assert!(validate_settings(&settings(1, 48)).is_err());
    }
}
