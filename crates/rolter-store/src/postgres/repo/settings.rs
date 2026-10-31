//! Org-wide and deployment-wide settings rows: policies, feature flags, logging, security.

use sqlx::PgPool;

use rolter_core::Result;

use super::super::models::{
    AdaptiveRoutingPolicy, ClientSettings, CompatibilityPolicy, FeatureFlags, LoggingSettings,
    ModelDefaults, RuntimePolicy, SecuritySettings,
};
use super::support::{fetch_optional_or_not_found, store_err};

impl CompatibilityPolicyRepo<'_> {
    pub async fn get(&self) -> Result<CompatibilityPolicy> {
        sqlx::query_as(
            "select anthropic_version, default_max_tokens, updated_at \
             from compatibility_policy where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        anthropic_version: &str,
        default_max_tokens: i32,
    ) -> Result<CompatibilityPolicy> {
        fetch_optional_or_not_found(
            sqlx::query_as(
                "update compatibility_policy set \
                anthropic_version = $1, default_max_tokens = $2, updated_at = now() \
             where id = true \
             returning anthropic_version, default_max_tokens, updated_at",
            )
            .bind(anthropic_version)
            .bind(default_max_tokens),
            self.0,
            || "compatibility policy".to_string(),
        )
        .await
    }
}

const CLIENT_SETTINGS_COLUMNS: &str = "public_base_url, forwarded_headers, injected_headers, \
     request_id_header, updated_at";

impl ClientSettingsRepo<'_> {
    pub async fn get(&self) -> Result<ClientSettings> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {CLIENT_SETTINGS_COLUMNS} from client_settings where id = true"
        )))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        public_base_url: Option<&str>,
        forwarded_headers: &[String],
        injected_headers: &serde_json::Value,
        request_id_header: &str,
    ) -> Result<ClientSettings> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update client_settings set \
                public_base_url = $1, forwarded_headers = $2, injected_headers = $3, \
                request_id_header = $4, updated_at = now() \
             where id = true \
             returning {CLIENT_SETTINGS_COLUMNS}"
            )))
            .bind(public_base_url)
            .bind(forwarded_headers)
            .bind(injected_headers)
            .bind(request_id_header),
            self.0,
            || "client settings".to_string(),
        )
        .await
    }
}

const MODEL_DEFAULTS_COLUMNS: &str = "enabled, default_model, default_temperature, \
     default_top_p, default_max_tokens, updated_at";

impl ModelDefaultsRepo<'_> {
    pub async fn get(&self) -> Result<ModelDefaults> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {MODEL_DEFAULTS_COLUMNS} from model_defaults where id = true"
        )))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        enabled: bool,
        default_model: Option<&str>,
        default_temperature: Option<f64>,
        default_top_p: Option<f64>,
        default_max_tokens: Option<i32>,
    ) -> Result<ModelDefaults> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update model_defaults set \
                enabled = $1, default_model = $2, default_temperature = $3, \
                default_top_p = $4, default_max_tokens = $5, updated_at = now() \
             where id = true \
             returning {MODEL_DEFAULTS_COLUMNS}"
            )))
            .bind(enabled)
            .bind(default_model)
            .bind(default_temperature)
            .bind(default_top_p)
            .bind(default_max_tokens),
            self.0,
            || "model defaults".to_string(),
        )
        .await
    }
}

const ADAPTIVE_POLICY_COLUMNS: &str = "enabled, latency_weight, cost_weight, load_weight, \
     exploration_ratio, min_samples, updated_at";

impl AdaptiveRoutingPolicyRepo<'_> {
    pub async fn get(&self) -> Result<AdaptiveRoutingPolicy> {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select {ADAPTIVE_POLICY_COLUMNS} from adaptive_routing_policy where id = true"
        )))
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        enabled: bool,
        latency_weight: f32,
        cost_weight: f32,
        load_weight: f32,
        exploration_ratio: f32,
        min_samples: i32,
    ) -> Result<AdaptiveRoutingPolicy> {
        fetch_optional_or_not_found(
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "update adaptive_routing_policy set \
                enabled = $1, latency_weight = $2, cost_weight = $3, load_weight = $4, \
                exploration_ratio = $5, min_samples = $6, updated_at = now() \
             where id = true returning {ADAPTIVE_POLICY_COLUMNS}"
            )))
            .bind(enabled)
            .bind(latency_weight)
            .bind(cost_weight)
            .bind(load_weight)
            .bind(exploration_ratio)
            .bind(min_samples),
            self.0,
            || "adaptive routing policy".to_string(),
        )
        .await
    }
}

/// The retry, timeout and queue settings the runtime policy is replaced with.
#[derive(Debug, Clone, Copy)]
pub struct RuntimePolicyUpdate<'a> {
    pub retry_max_retries: i32,
    pub retry_base_ms: i32,
    pub retry_max_ms: i32,
    pub timeout_connect_s: i32,
    pub timeout_request_s: i32,
    pub queue_enabled: bool,
    pub queue_capacity: i32,
    pub queue_workers: i32,
    pub queue_backpressure: &'a str,
    pub queue_block_ms: i32,
}

impl RuntimePolicyRepo<'_> {
    pub async fn get(&self) -> Result<RuntimePolicy> {
        sqlx::query_as(
            "select retry_max_retries, retry_base_ms, retry_max_ms, timeout_connect_s, \
                    timeout_request_s, queue_enabled, queue_capacity, queue_workers, \
                    queue_backpressure, queue_block_ms, updated_at \
             from runtime_policy where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(&self, values: RuntimePolicyUpdate<'_>) -> Result<RuntimePolicy> {
        let RuntimePolicyUpdate {
            retry_max_retries,
            retry_base_ms,
            retry_max_ms,
            timeout_connect_s,
            timeout_request_s,
            queue_enabled,
            queue_capacity,
            queue_workers,
            queue_backpressure,
            queue_block_ms,
        } = values;
        sqlx::query_as(
            "update runtime_policy set \
                retry_max_retries = $1, retry_base_ms = $2, retry_max_ms = $3, \
                timeout_connect_s = $4, timeout_request_s = $5, queue_enabled = $6, \
                queue_capacity = $7, queue_workers = $8, queue_backpressure = $9, \
                queue_block_ms = $10, updated_at = now() \
             where id = true \
             returning retry_max_retries, retry_base_ms, retry_max_ms, timeout_connect_s, \
                       timeout_request_s, queue_enabled, queue_capacity, queue_workers, \
                       queue_backpressure, queue_block_ms, updated_at",
        )
        .bind(retry_max_retries)
        .bind(retry_base_ms)
        .bind(retry_max_ms)
        .bind(timeout_connect_s)
        .bind(timeout_request_s)
        .bind(queue_enabled)
        .bind(queue_capacity)
        .bind(queue_workers)
        .bind(queue_backpressure)
        .bind(queue_block_ms)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

/// Global ingress policy. The encrypted secret remains write-only; callers
/// receive only whether a managed dashboard credential is configured.
pub struct SecuritySettingsRepo<'a>(pub &'a PgPool);
pub struct FeatureFlagsRepo<'a>(pub &'a PgPool);
pub struct LoggingSettingsRepo<'a>(pub &'a PgPool);
pub struct RuntimePolicyRepo<'a>(pub &'a PgPool);
pub struct CompatibilityPolicyRepo<'a>(pub &'a PgPool);
pub struct ClientSettingsRepo<'a>(pub &'a PgPool);
pub struct ModelDefaultsRepo<'a>(pub &'a PgPool);
pub struct AdaptiveRoutingPolicyRepo<'a>(pub &'a PgPool);

impl SecuritySettingsRepo<'_> {
    pub async fn get(&self) -> Result<SecuritySettings> {
        sqlx::query_as(
            "select allowed_origins, allowed_headers, \
                    required_headers, auth_bypass_routes, updated_at \
             from security_settings where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    /// `allow_direct_provider_keys` is pinned to `false` in the statement
    /// rather than taken as an argument: the gateway has no
    /// direct-provider-key passthrough, so the column never controlled
    /// anything and is no longer offered by the API (#1162). The dashboard
    /// password columns are likewise left untouched (#2356): they stay in the
    /// table, unread and unwritten, because migrations are append-only. So is
    /// `virtual_key_required` (#2357): no gateway decision ever read it, since
    /// every gateway that received it was managed and already closed.
    pub async fn update(
        &self,
        allowed_origins: &[String],
        allowed_headers: &[String],
        required_headers: serde_json::Value,
        auth_bypass_routes: &[String],
    ) -> Result<SecuritySettings> {
        sqlx::query_as(
            "update security_settings set \
                allowed_origins = $1, \
                allowed_headers = $2, required_headers = $3, auth_bypass_routes = $4, \
                allow_direct_provider_keys = false, updated_at = now() \
             where id = true \
             returning allowed_origins, allowed_headers, \
                       required_headers, auth_bypass_routes, updated_at",
        )
        .bind(allowed_origins)
        .bind(allowed_headers)
        .bind(required_headers)
        .bind(auth_bypass_routes)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

impl FeatureFlagsRepo<'_> {
    pub async fn get(&self) -> Result<FeatureFlags> {
        sqlx::query_as(
            "select response_cache, cache_aware_routing, circuit_breaker, active_health_checks, \
                    complexity_routing, guardrails, updated_at \
             from feature_flags where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(
        &self,
        response_cache: bool,
        cache_aware_routing: bool,
        circuit_breaker: bool,
        active_health_checks: bool,
        complexity_routing: bool,
        guardrails: bool,
    ) -> Result<FeatureFlags> {
        sqlx::query_as(
            "update feature_flags set \
                response_cache = $1, cache_aware_routing = $2, circuit_breaker = $3, \
                active_health_checks = $4, complexity_routing = $5, guardrails = $6, updated_at = now() \
             where id = true \
             returning response_cache, cache_aware_routing, circuit_breaker, active_health_checks, \
                       complexity_routing, guardrails, updated_at",
        )
        .bind(response_cache)
        .bind(cache_aware_routing)
        .bind(circuit_breaker)
        .bind(active_health_checks)
        .bind(complexity_routing)
        .bind(guardrails)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}

/// The logging settings that replace the stored row. `ui_events` is `None` to
/// keep the stored value.
#[derive(Debug, Clone, Copy)]
pub struct LoggingSettingsUpdate<'a> {
    pub sample_rate: f64,
    pub payload_capture_enabled: bool,
    pub payload_capture_max_bytes: i32,
    pub payload_capture_redact_fields: &'a [String],
    /// `None` keeps the stored detectors, like `ui_events`
    pub payload_capture_redact_detectors: Option<&'a [String]>,
    /// `None` keeps the stored patterns
    pub payload_capture_redact_patterns:
        Option<&'a [rolter_core::payload_redaction::RedactPattern]>,
    pub payload_capture_models: &'a [String],
    pub payload_capture_virtual_key_ids: &'a [String],
    pub retention_days: i32,
    pub payload_retention_hours: i32,
    pub ui_events: Option<bool>,
}

impl LoggingSettingsRepo<'_> {
    pub async fn get(&self) -> Result<LoggingSettings> {
        sqlx::query_as(
            "select sample_rate, payload_capture_enabled, payload_capture_max_bytes, \
                    payload_capture_redact_fields, payload_capture_redact_detectors, \
                    payload_capture_redact_patterns, payload_capture_models, payload_capture_virtual_key_ids, \
                    retention_days, payload_retention_hours, ui_events, updated_at \
             from logging_settings where id = true",
        )
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }

    pub async fn update(&self, values: LoggingSettingsUpdate<'_>) -> Result<LoggingSettings> {
        let LoggingSettingsUpdate {
            sample_rate,
            payload_capture_enabled,
            payload_capture_max_bytes,
            payload_capture_redact_fields,
            payload_capture_redact_detectors,
            payload_capture_redact_patterns,
            payload_capture_models,
            payload_capture_virtual_key_ids,
            retention_days,
            payload_retention_hours,
            ui_events,
        } = values;
        let patterns = payload_capture_redact_patterns.map(sqlx::types::Json);
        // `None` keeps the stored ui_events: a caller that has no opinion on it
        // (the seed importer, a client written before the column) must not
        // silently re-enable a stream an operator switched off
        sqlx::query_as(
            "update logging_settings set \
                sample_rate = $1, payload_capture_enabled = $2, payload_capture_max_bytes = $3, \
                payload_capture_redact_fields = $4, payload_capture_models = $5, \
                payload_capture_virtual_key_ids = $6, retention_days = $7, \
                payload_retention_hours = $8, ui_events = coalesce($9, ui_events), \
                payload_capture_redact_detectors = coalesce($10, payload_capture_redact_detectors), \
                payload_capture_redact_patterns = coalesce($11, payload_capture_redact_patterns), \
                updated_at = now() \
             where id = true \
             returning sample_rate, payload_capture_enabled, payload_capture_max_bytes, \
                       payload_capture_redact_fields, payload_capture_redact_detectors, \
                       payload_capture_redact_patterns, payload_capture_models, \
                       payload_capture_virtual_key_ids, retention_days, \
                       payload_retention_hours, ui_events, updated_at",
        )
        .bind(sample_rate)
        .bind(payload_capture_enabled)
        .bind(payload_capture_max_bytes)
        .bind(payload_capture_redact_fields)
        .bind(payload_capture_models)
        .bind(payload_capture_virtual_key_ids)
        .bind(retention_days)
        .bind(payload_retention_hours)
        .bind(ui_events)
        .bind(payload_capture_redact_detectors)
        .bind(patterns)
        .fetch_one(self.0)
        .await
        .map_err(store_err)
    }
}
