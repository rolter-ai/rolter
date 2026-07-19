//! Global security-policy API for the gateway ingress and dashboard.
//!
//! Dashboard credentials are write-only: a managed secret is sealed with the
//! deployment KEK before persistence, while external secret-manager references
//! are retained as opaque strings. Neither form is placed in audit details or
//! gateway snapshots.

use axum::extract::State;
use axum::http::header::HeaderName;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rolter_core::Error;
use rolter_store::postgres::models::SecuritySettings;
use rolter_store::postgres::repo::{AuditLogRepo, SecuritySettingsRepo};

use crate::crud::{pool, publish_config_change, ApiError, ApiResult};
use crate::rbac::{require_superadmin, Principal};
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new().route(
        "/api/v1/security-settings",
        get(get_security_settings).put(update_security_settings),
    )
}

async fn get_security_settings(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<SecuritySettings>> {
    require_superadmin(&principal)?;
    Ok(Json(SecuritySettingsRepo(pool(&state)).get().await?))
}

#[derive(Deserialize)]
struct UpdateSecuritySettings {
    virtual_key_required: bool,
    allow_direct_provider_keys: bool,
    #[serde(default)]
    allowed_origins: Vec<String>,
    #[serde(default)]
    allowed_headers: Vec<String>,
    #[serde(default)]
    required_headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    auth_bypass_routes: Vec<String>,
    dashboard_auth_enabled: bool,
    dashboard_credential_ref: Option<String>,
    /// write-only secret; it is encrypted before it reaches Postgres
    managed_dashboard_secret: Option<String>,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_origin(origin: &str) -> ApiResult<()> {
    let origin = origin.trim();
    if origin.is_empty()
        || origin.contains('*')
        || !(origin.starts_with("https://") || origin.starts_with("http://"))
        || origin.contains(['?', '#', '@'])
    {
        return Err(invalid(format!(
            "allowed origin '{origin}' must be an exact http(s) origin without wildcards"
        )));
    }
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or_default();
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.contains('/') || authority.chars().any(char::is_whitespace)
    {
        return Err(invalid(format!(
            "allowed origin '{origin}' must not include a path"
        )));
    }
    Ok(())
}

fn validate_header_name(name: &str, field: &str) -> ApiResult<()> {
    if HeaderName::from_bytes(name.trim().as_bytes()).is_err() {
        return Err(invalid(format!("{field} header '{name}' is invalid")));
    }
    Ok(())
}

fn validate_bypass_route(route: &str) -> ApiResult<()> {
    let route = route.trim();
    if !route.starts_with("/v1/")
        || route.contains(['*', '{', '}', '?', '#'])
        || route.chars().any(char::is_whitespace)
    {
        return Err(invalid(format!(
            "auth bypass route '{route}' must be an exact /v1 path without wildcards"
        )));
    }
    Ok(())
}

fn validate_settings(body: &UpdateSecuritySettings) -> ApiResult<()> {
    for origin in &body.allowed_origins {
        validate_origin(origin)?;
    }
    for header in &body.allowed_headers {
        validate_header_name(header, "allowed_headers")?;
    }
    for (header, value) in &body.required_headers {
        validate_header_name(header, "required_headers")?;
        if value.trim().is_empty() || value.contains(['\r', '\n']) {
            return Err(invalid(format!(
                "required header '{header}' must have a non-empty single-line value"
            )));
        }
    }
    for route in &body.auth_bypass_routes {
        validate_bypass_route(route)?;
    }
    if body.dashboard_auth_enabled
        && body
            .dashboard_credential_ref
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        && body
            .managed_dashboard_secret
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(invalid(
            "dashboard authentication requires dashboard_credential_ref or managed_dashboard_secret",
        ));
    }
    Ok(())
}

fn seal_dashboard_secret(secret: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    use rolter_store::postgres::crypto::{Kek, KEK_ENV};
    if secret.trim().is_empty() {
        return Err(invalid("managed_dashboard_secret must not be empty"));
    }
    let Some(kek) = Kek::from_env() else {
        return Err(invalid(format!(
            "storing dashboard credentials requires the {KEK_ENV} environment variable"
        )));
    };
    Ok(kek.encrypt(secret)?)
}

async fn update_security_settings(
    principal: Principal,
    State(state): State<ControlState>,
    Json(body): Json<UpdateSecuritySettings>,
) -> ApiResult<Json<SecuritySettings>> {
    require_superadmin(&principal)?;
    validate_settings(&body)?;
    let dashboard_secret = body
        .managed_dashboard_secret
        .as_deref()
        .map(seal_dashboard_secret)
        .transpose()?;
    let dashboard_secret = dashboard_secret
        .as_ref()
        .map(|(ciphertext, nonce)| (ciphertext.as_slice(), nonce.as_slice()));
    let row = SecuritySettingsRepo(pool(&state))
        .update(
            body.virtual_key_required,
            body.allow_direct_provider_keys,
            &body.allowed_origins,
            &body.allowed_headers,
            serde_json::to_value(&body.required_headers).map_err(|err| invalid(err.to_string()))?,
            &body.auth_bypass_routes,
            body.dashboard_auth_enabled,
            body.dashboard_credential_ref.as_deref(),
            dashboard_secret,
        )
        .await?;
    publish_config_change(&state).await?;
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "security.settings.update",
            Some("security_settings"),
            None,
            Some(serde_json::json!({
                "virtual_key_required": row.virtual_key_required,
                "allow_direct_provider_keys": row.allow_direct_provider_keys,
                "origin_count": row.allowed_origins.len(),
                "required_header_count": body.required_headers.len(),
                "bypass_route_count": row.auth_bypass_routes.len(),
                "dashboard_auth_enabled": row.dashboard_auth_enabled,
                "managed_dashboard_secret_configured": row.dashboard_secret_configured,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write security settings audit log");
    }
    Ok(Json(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wildcards_and_control_plane_bypass_paths() {
        assert!(validate_origin("https://*.example.com").is_err());
        assert!(validate_bypass_route("/admin/providers").is_err());
        assert!(validate_bypass_route("/v1/*").is_err());
    }

    #[test]
    fn accepts_exact_origin_headers_and_openai_path() {
        assert!(validate_origin("https://console.example.com").is_ok());
        assert!(validate_header_name("x-tenant-id", "required_headers").is_ok());
        assert!(validate_bypass_route("/v1/models").is_ok());
    }

    #[test]
    fn rejects_multiline_required_header_value() {
        let body = UpdateSecuritySettings {
            virtual_key_required: false,
            allow_direct_provider_keys: false,
            allowed_origins: Vec::new(),
            allowed_headers: Vec::new(),
            required_headers: [("x-tenant".to_string(), "a\nb".to_string())]
                .into_iter()
                .collect(),
            auth_bypass_routes: Vec::new(),
            dashboard_auth_enabled: false,
            dashboard_credential_ref: None,
            managed_dashboard_secret: None,
        };
        assert!(validate_settings(&body).is_err());
    }
}
