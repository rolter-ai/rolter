//! Global logging-policy API for request-log sampling, payload capture,
//! retention and the dashboard UX event opt-out.
//!
//! The policy itself is a superadmin's to read and write. Its sample rate is
//! the one value every analytics reader also needs, since below 100 % the
//! figures the analytics screens show are scaled up from a sample, so
//! `GET /api/v1/analytics/sampling` hands out that one number to anyone the
//! analytics reads serve (#3057).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rolter_core::payload_redaction::{PayloadRedactor, RedactPattern, RedactionReport};
use rolter_store::postgres::models::LoggingSettings;
use rolter_store::postgres::repo::{AuditLogRepo, LoggingSettingsRepo, LoggingSettingsUpdate};

use crate::analytics_access::AnalyticsAccess;
use crate::crud::{invalid_field, pool, publish_config_change, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/logging-settings",
            get(get_logging_settings).put(update_logging_settings),
        )
        .route(
            "/api/v1/logging-settings/redaction-preview",
            post(preview_redaction),
        )
        .route("/api/v1/analytics/sampling", get(get_log_sampling))
}

/// What `GET /api/v1/analytics/sampling` answers: the request log's sample
/// rate and nothing else from the policy.
#[derive(Debug, Serialize, PartialEq)]
struct LogSampling {
    sample_rate: f64,
}

impl LogSampling {
    /// The rate the snapshot serves: the bootstrap file's `[logging]` while the
    /// stored row still holds its seeded defaults, the row's otherwise (#3171).
    fn served(settings: &LoggingSettings, file: &rolter_core::LoggingConfig) -> Self {
        Self {
            sample_rate: settings.served(file).sample_rate,
        }
    }
}

/// The sample rate, for every caller the analytics reads serve (#3057).
///
/// The rest of the policy (payload capture, its model and key lists, retention)
/// stays a superadmin's, so this answers with the one number the dashboard
/// needs to say that the figures it shows are estimates. The rate is
/// deployment-wide and names no tenant, so a viewer may learn it like anyone
/// else who can open the analytics screens.
async fn get_log_sampling(
    // authenticates the caller exactly as the analytics reads do; the filter it
    // resolves is not needed, since the answer holds no tenant's rows
    _access: AnalyticsAccess,
    State(state): State<ControlState>,
) -> ApiResult<Json<LogSampling>> {
    let settings = LoggingSettingsRepo(pool(&state)).get().await?;
    Ok(Json(LogSampling::served(&settings, &state.file_logging)))
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
    /// built-in value detectors; absent keeps the stored set, so a client that
    /// predates the field cannot switch value redaction off (#1835)
    #[serde(default)]
    payload_capture_redact_detectors: Option<Vec<String>>,
    /// operator value patterns; absent keeps the stored set
    #[serde(default)]
    payload_capture_redact_patterns: Option<Vec<RedactPattern>>,
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

fn validate_settings(body: &UpdateLoggingSettings) -> ApiResult<()> {
    if !body.sample_rate.is_finite() || !(0.0..=1.0).contains(&body.sample_rate) {
        return Err(invalid_field(
            "sample_rate",
            "sample_rate must be in [0, 1]",
        ));
    }
    if !(0..=1_048_576).contains(&body.payload_capture_max_bytes) {
        return Err(invalid_field(
            "payload_capture_max_bytes",
            "payload_capture_max_bytes must be between 0 and 1048576",
        ));
    }
    if body.payload_capture_redact_fields.iter().any(|field| {
        field.trim().is_empty() || field.len() > 128 || field.chars().any(char::is_control)
    }) {
        return Err(invalid_field(
            "payload_capture_redact_fields",
            "payload_capture_redact_fields must contain 1-128 visible-character keys",
        ));
    }
    // compile the value policy now, so a pattern that cannot run is a 422 here
    // and never a silent gap in what the gateway redacts
    compile_value_policy(
        &body.payload_capture_redact_fields,
        body.payload_capture_redact_detectors
            .as_deref()
            .unwrap_or(&[]),
        body.payload_capture_redact_patterns
            .as_deref()
            .unwrap_or(&[]),
    )?;
    if body.payload_capture_models.iter().any(|model| {
        model.trim().is_empty() || model.len() > 255 || model.chars().any(char::is_control)
    }) {
        return Err(invalid_field(
            "payload_capture_models",
            "payload_capture_models must contain 1-255 visible-character model names",
        ));
    }
    if body
        .payload_capture_virtual_key_ids
        .iter()
        .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(invalid_field(
            "payload_capture_virtual_key_ids",
            "payload_capture_virtual_key_ids must contain valid UUIDs",
        ));
    }
    if !(1..=3650).contains(&body.retention_days) {
        return Err(invalid_field(
            "retention_days",
            "retention_days must be between 1 and 3650",
        ));
    }
    if !(1..=8760).contains(&body.payload_retention_hours) {
        return Err(invalid_field(
            "payload_retention_hours",
            "payload_retention_hours must be between 1 and 8760",
        ));
    }
    // raw bodies are the sensitive half; keeping them past the metadata they
    // belong to would leak prompt content the operator meant to expire
    if i64::from(body.payload_retention_hours) > i64::from(body.retention_days) * 24 {
        return Err(invalid_field(
            "payload_retention_hours",
            "payload_retention_hours must not exceed retention_days",
        ));
    }
    Ok(())
}

fn compile_value_policy(
    fields: &[String],
    detectors: &[String],
    patterns: &[RedactPattern],
) -> ApiResult<PayloadRedactor> {
    PayloadRedactor::compile(fields, detectors, patterns)
        .map_err(|err| invalid_field(err.field(), err.to_string()))
}

/// The largest sample the preview accepts.
const MAX_PREVIEW_SAMPLE_BYTES: usize = 64 * 1024;

/// A dry run of the redaction policy against a pasted sample. Any part of the
/// policy left out is read from the stored settings, so the dashboard can
/// preview unsaved edits alongside the saved remainder.
#[derive(Deserialize)]
struct RedactionPreviewRequest {
    sample: String,
    #[serde(default)]
    redact_fields: Option<Vec<String>>,
    #[serde(default)]
    redact_detectors: Option<Vec<String>>,
    #[serde(default)]
    redact_patterns: Option<Vec<RedactPattern>>,
}

#[derive(Serialize)]
struct RedactionPreview {
    /// what capture would store for this body
    output: String,
    /// what was found, by kind
    report: RedactionReport,
}

/// Run the redaction policy over a pasted sample and show the result.
///
/// Nothing is stored: the handler touches no table, and the sample is not
/// written to the audit log or to any log line, since a sample pasted to test a
/// detector is by construction a body that contains the secret it tests for.
async fn preview_redaction(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<RedactionPreviewRequest>,
) -> ApiResult<Json<RedactionPreview>> {
    authorize_superadmin(&principal, superadmin_cap!("logging_settings", Update))?;
    if body.sample.len() > MAX_PREVIEW_SAMPLE_BYTES {
        return Err(invalid_field(
            "sample",
            format!("sample must be at most {MAX_PREVIEW_SAMPLE_BYTES} bytes"),
        ));
    }
    let stored = LoggingSettingsRepo(pool(&state)).get().await?;
    let fields = body
        .redact_fields
        .unwrap_or(stored.payload_capture_redact_fields);
    let detectors = body
        .redact_detectors
        .unwrap_or(stored.payload_capture_redact_detectors);
    let patterns = body
        .redact_patterns
        .unwrap_or(stored.payload_capture_redact_patterns.0);
    let redactor = compile_value_policy(&fields, &detectors, &patterns)?;
    let mut report = RedactionReport::default();
    let output = redactor.capture(body.sample.as_bytes(), usize::MAX, &mut report);
    Ok(Json(RedactionPreview { output, report }))
}

async fn update_logging_settings(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateLoggingSettings>,
) -> ApiResult<Json<LoggingSettings>> {
    authorize_superadmin(&principal, superadmin_cap!("logging_settings", Update))?;
    validate_settings(&body)?;
    let row = LoggingSettingsRepo(pool(&state))
        .update(LoggingSettingsUpdate {
            sample_rate: body.sample_rate,
            payload_capture_enabled: body.payload_capture_enabled,
            payload_capture_max_bytes: body.payload_capture_max_bytes,
            payload_capture_redact_fields: &body.payload_capture_redact_fields,
            payload_capture_redact_detectors: body.payload_capture_redact_detectors.as_deref(),
            payload_capture_redact_patterns: body.payload_capture_redact_patterns.as_deref(),
            payload_capture_models: &body.payload_capture_models,
            payload_capture_virtual_key_ids: &body.payload_capture_virtual_key_ids,
            retention_days: body.retention_days,
            payload_retention_hours: body.payload_retention_hours,
            ui_events: body.ui_events,
        })
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
    let actor = principal.account_id();
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
                "payload_capture_redact_detectors": row.payload_capture_redact_detectors,
                "payload_capture_redact_pattern_names": row
                    .payload_capture_redact_patterns
                    .0
                    .iter()
                    .map(|pattern| pattern.name.as_str())
                    .collect::<Vec<_>>(),
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
            payload_capture_redact_detectors: None,
            payload_capture_redact_patterns: None,
            payload_capture_models: Vec::new(),
            payload_capture_virtual_key_ids: Vec::new(),
            retention_days,
            payload_retention_hours,
            ui_events: None,
        }
    }

    #[test]
    fn the_sampling_answer_carries_the_rate_and_nothing_else() {
        let stored = LoggingSettings {
            sample_rate: 0.25,
            payload_capture_enabled: true,
            payload_capture_max_bytes: 4096,
            payload_capture_redact_fields: vec!["authorization".to_string()],
            payload_capture_redact_detectors: Vec::new(),
            payload_capture_redact_patterns: Default::default(),
            payload_capture_models: vec!["gpt-4o".to_string()],
            payload_capture_virtual_key_ids: vec![uuid::Uuid::nil().to_string()],
            retention_days: 30,
            payload_retention_hours: 24,
            ui_events: false,
            updated_at: chrono::Utc::now(),
        };
        let file = rolter_core::LoggingConfig {
            sample_rate: 0.5,
            ..Default::default()
        };
        let body = serde_json::to_value(LogSampling::served(&stored, &file)).unwrap();
        assert_eq!(body, serde_json::json!({"sample_rate": 0.25}));
    }

    /// A control plane started with `--config` whose `[logging]` samples, with
    /// nobody having saved Logs Settings, serves the file's rate, so the
    /// dashboard must report that one (#3171).
    #[test]
    fn an_untouched_row_reports_the_file_rate() {
        let seeded = rolter_core::LoggingConfig::default();
        let untouched = LoggingSettings {
            sample_rate: seeded.sample_rate,
            payload_capture_enabled: seeded.payload_capture.enabled,
            payload_capture_max_bytes: seeded.payload_capture.max_bytes as i32,
            payload_capture_redact_fields: seeded.payload_capture.redact_fields.clone(),
            payload_capture_redact_detectors: seeded.payload_capture.redact_detectors.clone(),
            payload_capture_redact_patterns: Default::default(),
            payload_capture_models: Vec::new(),
            payload_capture_virtual_key_ids: Vec::new(),
            retention_days: 90,
            payload_retention_hours: 168,
            ui_events: seeded.ui_events,
            updated_at: chrono::Utc::now(),
        };
        let file = rolter_core::LoggingConfig {
            sample_rate: 0.25,
            ..Default::default()
        };
        assert_eq!(LogSampling::served(&untouched, &file).sample_rate, 0.25);
        // with no file the seeded row is all there is
        assert_eq!(LogSampling::served(&untouched, &seeded).sample_rate, 1.0);
    }

    #[test]
    fn a_pattern_that_cannot_run_is_refused_with_its_field() {
        let mut body = settings(90, 168);
        body.payload_capture_redact_patterns = Some(vec![RedactPattern {
            name: "bad".into(),
            pattern: "(unclosed".into(),
        }]);
        assert!(validate_settings(&body).is_err());
        body.payload_capture_redact_patterns = None;
        body.payload_capture_redact_detectors = Some(vec!["nope".into()]);
        assert!(validate_settings(&body).is_err());
        body.payload_capture_redact_detectors =
            Some(rolter_core::payload_redaction::detector_names());
        assert!(validate_settings(&body).is_ok());
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
