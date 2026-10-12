//! Global security-policy API for the gateway ingress and dashboard.
//!
//! There is no shared dashboard password: the dashboard is protected by
//! per-user sessions (#2356). A client that still sends the retired
//! `dashboard_*` fields is not rejected; the request struct ignores unknown
//! fields, so they are dropped.
//!
//! There is no "enforce virtual keys" switch either (#2357). It only ever
//! reached managed gateways, which refuse a keyless request anyway, so it
//! changed nothing; `virtual_key_required` from an older client is dropped the
//! same way.

use axum::extract::State;
use axum::http::header::HeaderName;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use rolter_store::postgres::models::SecuritySettings;
use rolter_store::postgres::repo::{AuditLogRepo, SecuritySettingsRepo};

use crate::crud::{invalid_field, pool, publish_config_change, ApiError, ApiResult, SafeJson};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
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
    authorize_superadmin(&principal, superadmin_cap!("security_settings", Read))?;
    Ok(Json(SecuritySettingsRepo(pool(&state)).get().await?))
}

#[derive(Deserialize)]
struct UpdateSecuritySettings {
    #[serde(default)]
    allowed_origins: Vec<String>,
    #[serde(default)]
    allowed_headers: Vec<String>,
    #[serde(default)]
    required_headers: std::collections::HashMap<String, String>,
    #[serde(default)]
    auth_bypass_routes: Vec<String>,
}

fn validate_origin(origin: &str, field: &str) -> ApiResult<()> {
    let origin = origin.trim();
    if origin.is_empty()
        || origin.contains('*')
        || !(origin.starts_with("https://") || origin.starts_with("http://"))
        || origin.contains(['?', '#', '@'])
    {
        return Err(invalid_field(
            field,
            format!("allowed origin '{origin}' must be an exact http(s) origin without wildcards"),
        ));
    }
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or_default();
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty() || authority.contains('/') || authority.chars().any(char::is_whitespace)
    {
        return Err(invalid_field(
            field,
            format!("allowed origin '{origin}' must not include a path"),
        ));
    }
    Ok(())
}

fn validate_header_name(name: &str, list: &str, field: &str) -> ApiResult<()> {
    if HeaderName::from_bytes(name.trim().as_bytes()).is_err() {
        return Err(invalid_field(
            field,
            format!("{list} header '{name}' is invalid"),
        ));
    }
    Ok(())
}

fn validate_bypass_route(route: &str, field: &str) -> ApiResult<()> {
    let route = route.trim();
    if !route.starts_with("/v1/")
        || route.contains(['*', '{', '}', '?', '#'])
        || route.chars().any(char::is_whitespace)
    {
        return Err(invalid_field(
            field,
            format!("auth bypass route '{route}' must be an exact /v1 path without wildcards"),
        ));
    }
    Ok(())
}

fn validate_settings(body: &UpdateSecuritySettings) -> ApiResult<()> {
    for (idx, origin) in body.allowed_origins.iter().enumerate() {
        validate_origin(origin, &format!("allowed_origins[{idx}]"))?;
    }
    for (idx, header) in body.allowed_headers.iter().enumerate() {
        validate_header_name(
            header,
            "allowed_headers",
            &format!("allowed_headers[{idx}]"),
        )?;
    }
    for (header, value) in &body.required_headers {
        let row = format!("required_headers.{header}");
        validate_header_name(header, "required_headers", &row)?;
        if value.trim().is_empty() || value.contains(['\r', '\n']) {
            return Err(invalid_field(
                row,
                format!("required header '{header}' must have a non-empty single-line value"),
            ));
        }
    }
    for (idx, route) in body.auth_bypass_routes.iter().enumerate() {
        validate_bypass_route(route, &format!("auth_bypass_routes[{idx}]"))?;
    }
    Ok(())
}

async fn update_security_settings(
    principal: Principal,
    State(state): State<ControlState>,
    SafeJson(body): SafeJson<UpdateSecuritySettings>,
) -> ApiResult<Json<SecuritySettings>> {
    authorize_superadmin(&principal, superadmin_cap!("security_settings", Update))?;
    validate_settings(&body)?;
    let row = SecuritySettingsRepo(pool(&state))
        .update(
            // #1162: `allow_direct_provider_keys` is gone from this call.
            // The gateway has no direct-provider-key passthrough, so the
            // column never controlled anything; the store pins it to its
            // default rather than leave a toggle that reads like a security
            // control and is not one. `virtual_key_required` went the same
            // way for the same reason (#2357)
            &body.allowed_origins,
            &body.allowed_headers,
            serde_json::to_value(&body.required_headers)
                .map_err(|_| ApiError::Curated("required headers could not be stored".into()))?,
            &body.auth_bypass_routes,
        )
        .await?;
    publish_config_change(&state).await?;
    // the stored origins and headers are the live cross-origin policy, so
    // reload it here rather than at the next restart — an operator who saves an
    // origin expects it to work, which is the whole of #813
    crate::cors::refresh(&state).await;
    let actor = principal.account_id();
    if let Err(err) = AuditLogRepo(pool(&state))
        .create(
            None,
            actor,
            "security.settings.update",
            Some("security_settings"),
            None,
            Some(serde_json::json!({
                "origin_count": row.allowed_origins.len(),
                "required_header_count": body.required_headers.len(),
                "bypass_route_count": row.auth_bypass_routes.len(),
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
        assert!(validate_origin("https://*.example.com", "allowed_origins[0]").is_err());
        assert!(validate_bypass_route("/admin/providers", "auth_bypass_routes[0]").is_err());
        assert!(validate_bypass_route("/v1/*", "auth_bypass_routes[1]").is_err());
    }

    #[test]
    fn a_refusal_names_the_list_row_it_is_about() {
        let body = UpdateSecuritySettings {
            allowed_origins: vec!["https://ok.example.com".into(), "https://*.bad".into()],
            allowed_headers: vec![],
            required_headers: Default::default(),
            auth_bypass_routes: vec![],
        };
        let Err(ApiError::InvalidField { field, .. }) = validate_settings(&body) else {
            panic!("expected an invalid_field refusal");
        };
        assert_eq!(field, "allowed_origins[1]");
    }

    #[test]
    fn accepts_exact_origin_headers_and_openai_path() {
        assert!(validate_origin("https://console.example.com", "allowed_origins[0]").is_ok());
        assert!(validate_header_name(
            "x-tenant-id",
            "required_headers",
            "required_headers.x-tenant-id"
        )
        .is_ok());
        assert!(validate_bypass_route("/v1/models", "auth_bypass_routes[0]").is_ok());
    }

    #[test]
    fn the_retired_dashboard_fields_are_ignored_not_rejected() {
        let body: UpdateSecuritySettings = serde_json::from_value(serde_json::json!({
            "virtual_key_required": true,
            "allowed_origins": [],
            "dashboard_auth_enabled": true,
            "dashboard_credential_ref": "X",
            "managed_dashboard_secret": uuid::Uuid::new_v4().to_string(),
        }))
        .expect("old clients must still parse");
        assert!(validate_settings(&body).is_ok());
    }

    #[test]
    fn rejects_multiline_required_header_value() {
        let body = UpdateSecuritySettings {
            allowed_origins: Vec::new(),
            allowed_headers: Vec::new(),
            required_headers: [("x-tenant".to_string(), "a\nb".to_string())]
                .into_iter()
                .collect(),
            auth_bypass_routes: Vec::new(),
        };
        assert!(validate_settings(&body).is_err());
    }
}
