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

use rolter_store::postgres::models::ClientSettings;
use rolter_store::postgres::repo::{AuditLogRepo, ClientSettingsRepo};

use crate::crud::{invalid_field, pool, publish_config_change, ApiResult, SafeJson};
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
    /// header names refused as the request id header: the reserved names plus
    /// the other credential, session and framing headers (#2970)
    request_id_refused: Vec<&'static str>,
}

impl From<ClientSettings> for ClientSettingsView {
    fn from(settings: ClientSettings) -> Self {
        Self {
            settings,
            always_propagated: ALWAYS_PROPAGATED,
            reserved: RESERVED_HEADERS,
            request_id_refused: rolter_core::request_id_refused_headers(),
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

/// Reject anything that would not survive the wire: a header name that is not a
/// valid token, a reserved name, or a value with control characters in it.
/// `field` is the path of the offending entry (`forwarded_headers[2]`,
/// `injected_headers.x-team`), so a client can mark that row.
fn validate_header_name(name: &str, field: &str) -> ApiResult<String> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return Err(invalid_field(field, "header names must not be empty"));
    }
    if HeaderName::try_from(lower.as_str()).is_err() {
        return Err(invalid_field(
            field,
            format!("'{name}' is not a valid header name"),
        ));
    }
    if RESERVED_HEADERS.contains(&lower.as_str()) {
        return Err(invalid_field(
            field,
            format!("'{lower}' is managed by the gateway and cannot be forwarded or injected"),
        ));
    }
    Ok(lower)
}

fn validate_settings(body: &UpdateClientSettings) -> ApiResult<()> {
    if let Some(url) = body.public_base_url.as_deref() {
        let url = url.trim();
        if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
            return Err(invalid_field(
                "public_base_url",
                "public_base_url must be an http(s) URL",
            ));
        }
        if url.len() > 2_048 {
            return Err(invalid_field(
                "public_base_url",
                "public_base_url must be at most 2048 characters",
            ));
        }
    }
    if body.forwarded_headers.len() > 64 {
        return Err(invalid_field(
            "forwarded_headers",
            "at most 64 forwarded headers are allowed",
        ));
    }
    if body.injected_headers.len() > 64 {
        return Err(invalid_field(
            "injected_headers",
            "at most 64 injected headers are allowed",
        ));
    }
    for (idx, name) in body.forwarded_headers.iter().enumerate() {
        validate_header_name(name, &format!("forwarded_headers[{idx}]"))?;
    }
    for (name, value) in &body.injected_headers {
        // the map is keyed by name, so the name is the row's path
        let row = format!("injected_headers.{}", name.trim().to_ascii_lowercase());
        validate_header_name(name, &row)?;
        if value.len() > 4_096 {
            return Err(invalid_field(
                row,
                format!("value for '{name}' exceeds 4096 bytes"),
            ));
        }
        if value.chars().any(|c| c.is_control()) {
            return Err(invalid_field(
                row,
                format!("value for '{name}' must not contain control characters"),
            ));
        }
    }
    let request_id_header = validate_header_name(&body.request_id_header, "request_id_header")?;
    // the gateway reads, echoes and logs this header, so it refuses a few
    // names the upstream lists allow; checking the same rule here keeps a save
    // from succeeding while the gateway keeps using the default (#2925)
    if let Some(problem) = rolter_core::request_id_header_problem(&request_id_header) {
        return Err(invalid_field(
            "request_id_header",
            format!("request_id_header: {problem}"),
        ));
    }
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
    use crate::crud::ApiError;

    fn body() -> UpdateClientSettings {
        UpdateClientSettings {
            public_base_url: Some("https://gateway.example.com".to_string()),
            forwarded_headers: vec!["X-Tenant-Id".to_string()],
            injected_headers: BTreeMap::from([("x-org".to_string(), "acme".to_string())]),
            request_id_header: "x-request-id".to_string(),
        }
    }

    fn refused_field(body: &UpdateClientSettings) -> String {
        match validate_settings(body) {
            Err(ApiError::InvalidField { field, .. }) => field,
            other => panic!("expected an invalid_field refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_bad_header_names_its_row_rather_than_the_message_prefix() {
        let mut bad = body();
        bad.injected_headers
            .insert("bad header".to_string(), "v".to_string());
        assert_eq!(refused_field(&bad), "injected_headers.bad header");

        let mut bad = body();
        bad.forwarded_headers.push("authorization".to_string());
        assert_eq!(refused_field(&bad), "forwarded_headers[1]");

        let mut bad = body();
        bad.injected_headers
            .insert("x-big".to_string(), "v".repeat(5_000));
        assert_eq!(refused_field(&bad), "injected_headers.x-big");

        let mut bad = body();
        bad.public_base_url = Some("ftp://x".to_string());
        assert_eq!(refused_field(&bad), "public_base_url");
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
    fn the_refused_list_matches_what_validation_refuses() {
        let view = ClientSettingsView::from(ClientSettings {
            public_base_url: None,
            forwarded_headers: vec![],
            injected_headers: serde_json::json!({}),
            request_id_header: "x-request-id".to_string(),
            updated_at: chrono::Utc::now(),
        });
        let json = serde_json::to_value(&view).unwrap();
        let listed = json["request_id_refused"].as_array().unwrap();
        assert!(listed.iter().any(|n| n == "x-goog-api-key"));
        assert!(listed.iter().any(|n| n == "set-cookie"));
        for name in listed {
            let mut refused = body();
            refused.request_id_header = name.as_str().unwrap().to_string();
            assert!(validate_settings(&refused).is_err(), "{name}");
        }
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

    /// The gateway would refuse these as the request id header and keep using
    /// `x-request-id`, so a save that names one must fail rather than appear
    /// to work.
    #[test]
    fn rejects_a_request_id_header_the_gateway_would_not_use() {
        for name in ["Proxy-Authorization", "x-goog-api-key", "set-cookie", "te"] {
            let mut reserved = body();
            reserved.request_id_header = name.to_string();
            assert!(validate_settings(&reserved).is_err(), "{name}");
        }

        let mut custom = body();
        custom.request_id_header = "X-Correlation-Id".to_string();
        assert!(validate_settings(&custom).is_ok());
    }
}
