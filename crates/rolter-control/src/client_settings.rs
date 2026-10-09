//! Global client-settings API: the base URL the dashboard advertises and what
//! the gateway does with headers on the upstream leg (#564).
//!
//! Injected header *values* can carry deployment secrets (a partner tenant id,
//! a shared token), so they are never written to the audit log — the log
//! records which header names changed, not what they were set to.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::header::HeaderName;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use rolter_core::Error;
use rolter_store::postgres::models::ClientSettings;
use rolter_store::postgres::repo::{AuditLogRepo, ClientSettingsRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/client-settings",
        get(get_client_settings).put(update_client_settings),
    )
}

/// Headers the gateway owns end to end. Forwarding or overriding one of these
/// would either break the upstream call or let a caller impersonate the
/// gateway's own credentials, so they are rejected on both lists.
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "api-key",
    "host",
    "content-length",
    "content-type",
    "transfer-encoding",
    "connection",
    "cookie",
];

/// Trace-context headers propagate unconditionally, so listing them adds
/// nothing and only invites the belief that removing them stops propagation.
const ALWAYS_PROPAGATED: &[&str] = &["traceparent", "tracestate", "b3"];

#[derive(Debug, Serialize)]
struct ClientSettingsView {
    #[serde(flatten)]
    settings: ClientSettings,
    /// header names the gateway propagates whether or not they are listed
    always_propagated: &'static [&'static str],
    /// header names that can never be forwarded or injected
    reserved: &'static [&'static str],
}

impl From<ClientSettings> for ClientSettingsView {
    fn from(settings: ClientSettings) -> Self {
        Self {
            settings,
            always_propagated: ALWAYS_PROPAGATED,
            reserved: RESERVED_HEADERS,
        }
    }
}

async fn get_client_settings(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<ClientSettingsView>> {
    authorize_superadmin(&principal, superadmin_cap!("client_settings", Read))?;
    Ok(Json(ClientSettingsRepo(pool(&state)).get().await?.into()))
}

#[derive(Debug, Deserialize)]
struct UpdateClientSettings {
    #[serde(default)]
    public_base_url: Option<String>,
    #[serde(default)]
    forwarded_headers: Vec<String>,
    #[serde(default)]
    injected_headers: BTreeMap<String, String>,
    request_id_header: String,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

/// Reject anything that would not survive the wire: a header name that is not a
/// valid token, a reserved name, or a value with control characters in it.
fn validate_header_name(name: &str) -> ApiResult<String> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return Err(invalid("header names must not be empty"));
    }
    if HeaderName::try_from(lower.as_str()).is_err() {
        return Err(invalid(format!("'{name}' is not a valid header name")));
    }
    if RESERVED_HEADERS.contains(&lower.as_str()) {
        return Err(invalid(format!(
            "'{lower}' is managed by the gateway and cannot be forwarded or injected"
        )));
    }
    Ok(lower)
}

fn validate_settings(body: &UpdateClientSettings) -> ApiResult<()> {
    if let Some(url) = body.public_base_url.as_deref() {
        let url = url.trim();
        if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
            return Err(invalid("public_base_url must be an http(s) URL"));
        }
        if url.len() > 2_048 {
            return Err(invalid("public_base_url must be at most 2048 characters"));
        }
    }
    if body.forwarded_headers.len() > 64 {
        return Err(invalid("at most 64 forwarded headers are allowed"));
    }
    if body.injected_headers.len() > 64 {
        return Err(invalid("at most 64 injected headers are allowed"));
    }
    for name in &body.forwarded_headers {
        validate_header_name(name)?;
    }
    for (name, value) in &body.injected_headers {
        validate_header_name(name)?;
        if value.len() > 4_096 {
            return Err(invalid(format!("value for '{name}' exceeds 4096 bytes")));
        }
        if value.chars().any(|c| c.is_control()) {
            return Err(invalid(format!(
                "value for '{name}' must not contain control characters"
            )));
        }
    }
    validate_header_name(&body.request_id_header)?;
    Ok(())
}

async fn update_client_settings(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateClientSettings>,
) -> ApiResult<Json<ClientSettingsView>> {
    authorize_superadmin(&principal, superadmin_cap!("client_settings", Update))?;
    validate_settings(&body)?;

    // normalize once so the stored row, the snapshot and the audit trail all
    // agree on casing and never carry duplicates
    let mut forwarded: Vec<String> = body
        .forwarded_headers
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect();
    forwarded.sort();
    forwarded.dedup();
    let injected: serde_json::Map<String, serde_json::Value> = body
        .injected_headers
        .iter()
        .map(|(name, value)| {
            (
                name.trim().to_ascii_lowercase(),
                serde_json::Value::String(value.clone()),
            )
        })
        .collect();
    let base_url = body
        .public_base_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());

    let row = ClientSettingsRepo(pool(&state))
        .update(
            base_url,
            &forwarded,
            &serde_json::Value::Object(injected.clone()),
            &body.request_id_header.trim().to_ascii_lowercase(),
        )
        .await?;
    publish_config_change(&state).await?;

    let actor = principal.account_id();
    // names only — an injected header value is deployment credential material
    let injected_names: Vec<&String> = injected.keys().collect();
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "client_settings.update",
            Some("client_settings"),
            None,
            Some(serde_json::json!({
                "public_base_url": row.public_base_url,
                "forwarded_headers": row.forwarded_headers,
                "injected_header_names": injected_names,
                "request_id_header": row.request_id_header,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write client settings audit log");
    }
    Ok(Json(row.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> UpdateClientSettings {
        UpdateClientSettings {
            public_base_url: Some("https://gateway.example.com".to_string()),
            forwarded_headers: vec!["X-Tenant-Id".to_string()],
            injected_headers: BTreeMap::from([("x-org".to_string(), "acme".to_string())]),
            request_id_header: "x-request-id".to_string(),
        }
    }

    #[test]
    fn accepts_ordinary_headers_and_an_https_base_url() {
        assert!(validate_settings(&body()).is_ok());
    }

    #[test]
    fn rejects_reserved_headers_on_either_list() {
        let mut forwarded = body();
        forwarded.forwarded_headers = vec!["Authorization".to_string()];
        assert!(validate_settings(&forwarded).is_err());

        let mut injected = body();
        injected.injected_headers = BTreeMap::from([("x-api-key".to_string(), "k".to_string())]);
        assert!(validate_settings(&injected).is_err());
    }

    #[test]
    fn rejects_malformed_names_values_and_urls() {
        let mut name = body();
        name.forwarded_headers = vec!["not a header".to_string()];
        assert!(validate_settings(&name).is_err());

        let mut value = body();
        value.injected_headers = BTreeMap::from([("x-org".to_string(), "a\nb".to_string())]);
        assert!(validate_settings(&value).is_err());

        let mut url = body();
        url.public_base_url = Some("gateway.example.com".to_string());
        assert!(validate_settings(&url).is_err());
    }

    #[test]
    fn rejects_an_invalid_request_id_header() {
        let mut invalid_header = body();
        invalid_header.request_id_header = "x request id".to_string();
        assert!(validate_settings(&invalid_header).is_err());
    }
}
