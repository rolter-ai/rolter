//! Opt-in observability connector configuration and OTLP/HTTP test delivery.
//!
//! Continuous export workers intentionally do not live in this request API.
//! The control plane persists connector intent, verifies it with a bounded
//! OTLP/HTTP test, and records a health result without ever returning a secret.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::repo::AuditLogRepo;

use crate::crud::{pool, ApiError, ApiResult};
use crate::rbac::{require_superadmin, Principal};
use crate::ControlState;

const OTLP_HTTP: &str = "otlp_http";
const TEST_DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/connectors",
            get(list_connectors).post(create_connector),
        )
        .route(
            "/api/v1/connectors/{id}",
            get(get_connector)
                .put(update_connector)
                .delete(delete_connector),
        )
        .route("/api/v1/connectors/{id}/test", post(test_delivery))
}

/// Connector configuration safe to return to dashboard clients.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
struct Connector {
    id: Uuid,
    name: String,
    kind: String,
    endpoint: String,
    enabled: bool,
    sampling_rate: f64,
    auth_secret_ref: Option<String>,
    auth_secret_configured: bool,
    health_status: String,
    health_checked_at: Option<DateTime<Utc>>,
    health_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ConnectorSecret {
    endpoint: String,
    auth_secret_ref: Option<String>,
    auth_secret_ciphertext: Option<Vec<u8>>,
    auth_secret_nonce: Option<Vec<u8>>,
}

#[derive(Debug, Deserialize)]
struct ConnectorInput {
    name: String,
    kind: String,
    endpoint: String,
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_sampling_rate")]
    sampling_rate: f64,
    /// opaque external secret-manager reference; never dereferenced here
    auth_secret_ref: Option<String>,
    /// write-only bearer token, sealed before it reaches Postgres
    managed_auth_secret: Option<String>,
}

fn default_sampling_rate() -> f64 {
    1.0
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_input(input: &ConnectorInput) -> ApiResult<()> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(invalid("connector name must be 1-128 visible characters"));
    }
    if input.kind != OTLP_HTTP {
        return Err(invalid("connector kind must be otlp_http"));
    }
    let endpoint = reqwest::Url::parse(input.endpoint.trim())
        .map_err(|_| invalid("connector endpoint must be a valid http(s) URL"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(invalid(
            "connector endpoint must be an http(s) URL without userinfo",
        ));
    }
    if !input.sampling_rate.is_finite() || !(0.0..=1.0).contains(&input.sampling_rate) {
        return Err(invalid("sampling_rate must be between 0 and 1"));
    }
    if input.auth_secret_ref.as_ref().is_some_and(|reference| {
        reference.trim().is_empty()
            || reference.len() > 1024
            || reference.chars().any(char::is_control)
    }) {
        return Err(invalid(
            "auth_secret_ref must be 1-1024 visible characters when supplied",
        ));
    }
    if input
        .managed_auth_secret
        .as_ref()
        .is_some_and(|secret| secret.trim().is_empty())
    {
        return Err(invalid("managed_auth_secret must not be empty"));
    }
    Ok(())
}

fn seal_secret(secret: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    let Some(kek) = Kek::from_env() else {
        return Err(invalid(format!(
            "storing connector credentials requires the {KEK_ENV} environment variable"
        )));
    };
    kek.encrypt(secret).map_err(|_| {
        ApiError::Core(Error::Store(
            "failed to encrypt connector credential".into(),
        ))
    })
}

fn connector_columns() -> &'static str {
    "id, name, kind, endpoint, enabled, sampling_rate, auth_secret_ref, \
     auth_secret_ciphertext is not null as auth_secret_configured, health_status, health_checked_at, \
     health_error, created_at, updated_at"
}

async fn list_connectors(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Connector>>> {
    require_superadmin(&principal)?;
    let query = format!(
        "select {} from observability_connectors order by name",
        connector_columns()
    );
    let connectors = sqlx::query_as(&query)
        .fetch_all(pool(&state))
        .await
        .map_err(|err| Error::Store(err.to_string()))?;
    Ok(Json(connectors))
}

async fn get_connector(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Connector>> {
    require_superadmin(&principal)?;
    Ok(Json(fetch_connector(pool(&state), id).await?))
}

async fn fetch_connector(db: &sqlx::PgPool, id: Uuid) -> Result<Connector, Error> {
    let query = format!(
        "select {} from observability_connectors where id = $1",
        connector_columns()
    );
    sqlx::query_as(&query)
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(|err| Error::Store(err.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("connector {id}")))
}

async fn create_connector(
    principal: Principal,
    State(state): State<ControlState>,
    Json(input): Json<ConnectorInput>,
) -> ApiResult<Json<Connector>> {
    require_superadmin(&principal)?;
    validate_input(&input)?;
    let secret = input
        .managed_auth_secret
        .as_deref()
        .map(seal_secret)
        .transpose()?;
    let connector: Connector = sqlx::query_as(&format!(
        "insert into observability_connectors \
         (name, kind, endpoint, enabled, sampling_rate, auth_secret_ref, auth_secret_ciphertext, auth_secret_nonce) \
         values ($1, $2, $3, $4, $5, $6, $7, $8) returning {}",
        connector_columns()
    ))
    .bind(input.name.trim())
    .bind(OTLP_HTTP)
    .bind(input.endpoint.trim())
    .bind(input.enabled)
    .bind(input.sampling_rate)
    .bind(input.auth_secret_ref.as_deref().map(str::trim))
    .bind(secret.as_ref().map(|(ciphertext, _)| ciphertext.as_slice()))
    .bind(secret.as_ref().map(|(_, nonce)| nonce.as_slice()))
    .fetch_one(pool(&state))
    .await
    .map_err(|err| Error::Store(err.to_string()))?;
    audit(
        &state,
        &principal,
        "connector.create",
        connector.id,
        &connector,
    )
    .await;
    Ok(Json(connector))
}

async fn update_connector(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ConnectorInput>,
) -> ApiResult<Json<Connector>> {
    require_superadmin(&principal)?;
    validate_input(&input)?;
    let secret = input
        .managed_auth_secret
        .as_deref()
        .map(seal_secret)
        .transpose()?;
    let connector: Connector = sqlx::query_as(&format!(
        "update observability_connectors set \
         name = $2, endpoint = $3, enabled = $4, sampling_rate = $5, auth_secret_ref = $6, \
         auth_secret_ciphertext = coalesce($7, auth_secret_ciphertext), \
         auth_secret_nonce = coalesce($8, auth_secret_nonce), updated_at = now() \
         where id = $1 returning {}",
        connector_columns()
    ))
    .bind(id)
    .bind(input.name.trim())
    .bind(input.endpoint.trim())
    .bind(input.enabled)
    .bind(input.sampling_rate)
    .bind(input.auth_secret_ref.as_deref().map(str::trim))
    .bind(secret.as_ref().map(|(ciphertext, _)| ciphertext.as_slice()))
    .bind(secret.as_ref().map(|(_, nonce)| nonce.as_slice()))
    .fetch_optional(pool(&state))
    .await
    .map_err(|err| Error::Store(err.to_string()))?
    .ok_or_else(|| Error::NotFound(format!("connector {id}")))?;
    audit(
        &state,
        &principal,
        "connector.update",
        connector.id,
        &connector,
    )
    .await;
    Ok(Json(connector))
}

async fn delete_connector(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
    let affected = sqlx::query("delete from observability_connectors where id = $1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|err| Error::Store(err.to_string()))?
        .rows_affected();
    if affected == 0 {
        return Err(ApiError::Core(Error::NotFound(format!("connector {id}"))));
    }
    audit_delete(&state, &principal, id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct TestDelivery {
    delivered: bool,
    health_status: String,
    health_checked_at: DateTime<Utc>,
}

async fn test_delivery(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TestDelivery>> {
    require_superadmin(&principal)?;
    let secret = fetch_secret(pool(&state), id).await?;
    let auth = decrypt_secret(&secret)?;
    let result = send_test(&state, &secret.endpoint, auth.as_deref()).await;
    let (delivered, health_status, health_error) = match result {
        Ok(()) => (true, "healthy", None),
        Err(error) => (false, "unhealthy", Some(error)),
    };
    let checked_at: (DateTime<Utc>,) = sqlx::query_as(
        "update observability_connectors set health_status = $2, health_checked_at = now(), \
         health_error = $3, updated_at = now() where id = $1 returning health_checked_at",
    )
    .bind(id)
    .bind(health_status)
    .bind(health_error)
    .fetch_one(pool(&state))
    .await
    .map_err(|err| Error::Store(err.to_string()))?;
    audit_test(&state, &principal, id, delivered, health_status).await;
    Ok(Json(TestDelivery {
        delivered,
        health_status: health_status.to_string(),
        health_checked_at: checked_at.0,
    }))
}

async fn fetch_secret(db: &sqlx::PgPool, id: Uuid) -> Result<ConnectorSecret, Error> {
    sqlx::query_as(
        "select endpoint, auth_secret_ref, auth_secret_ciphertext, auth_secret_nonce \
         from observability_connectors where id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(|err| Error::Store(err.to_string()))?
    .ok_or_else(|| Error::NotFound(format!("connector {id}")))
}

fn decrypt_secret(secret: &ConnectorSecret) -> ApiResult<Option<String>> {
    if secret.auth_secret_ref.is_some() && secret.auth_secret_ciphertext.is_none() {
        return Err(invalid(
            "test delivery cannot resolve auth_secret_ref; configure managed_auth_secret instead",
        ));
    }
    let (Some(ciphertext), Some(nonce)) = (
        secret.auth_secret_ciphertext.as_deref(),
        secret.auth_secret_nonce.as_deref(),
    ) else {
        return Ok(None);
    };
    let Some(kek) = Kek::from_env() else {
        return Err(invalid(format!(
            "test delivery requires the {KEK_ENV} environment variable to decrypt the connector credential"
        )));
    };
    kek.decrypt(ciphertext, nonce).map(Some).map_err(|_| {
        ApiError::Core(Error::Store(
            "failed to decrypt connector credential; check ROLTER_KEK".into(),
        ))
    })
}

async fn send_test(state: &ControlState, endpoint: &str, auth: Option<&str>) -> Result<(), String> {
    let record = serde_json::json!({
        "timeUnixNano": Utc::now().timestamp_nanos_opt().unwrap_or_default().to_string(),
        "severityText": "INFO",
        "body": {"stringValue": "rolter connector test delivery"},
        "attributes": [{"key": "rolter.connector_test", "value": {"boolValue": true}}]
    });
    let payload = serde_json::json!({
        "resourceLogs": [{
            "resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "rolter-control"}}]},
            "scopeLogs": [{"scope": {"name": "rolter-control"}, "logRecords": [record]}]
        }]
    });
    let mut request = state
        .http
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&payload);
    if let Some(auth) = auth {
        request = request.bearer_auth(auth);
    }
    let response = tokio::time::timeout(TEST_DELIVERY_TIMEOUT, request.send())
        .await
        .map_err(|_| "timeout".to_string())
        .and_then(|result| result.map_err(|_| "transport_error".to_string()))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("http_{}", response.status().as_u16()))
    }
}

async fn audit(
    state: &ControlState,
    principal: &Principal,
    action: &str,
    id: Uuid,
    connector: &Connector,
) {
    let actor = match principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(error) = AuditLogRepo(pool(state))
        .create(
            None,
            actor,
            action,
            Some("observability_connector"),
            Some(id),
            Some(serde_json::json!({
                "name": connector.name,
                "kind": connector.kind,
                "enabled": connector.enabled,
                "sampling_rate": connector.sampling_rate,
                "managed_auth_secret_configured": connector.auth_secret_configured,
            })),
        )
        .await
    {
        tracing::warn!(error = %error, action, "failed to write connector audit log");
    }
}

async fn audit_delete(state: &ControlState, principal: &Principal, id: Uuid) {
    let actor = match principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(error) = AuditLogRepo(pool(state))
        .create(
            None,
            actor,
            "connector.delete",
            Some("observability_connector"),
            Some(id),
            None,
        )
        .await
    {
        tracing::warn!(error = %error, "failed to write connector audit log");
    }
}

async fn audit_test(
    state: &ControlState,
    principal: &Principal,
    id: Uuid,
    delivered: bool,
    health_status: &str,
) {
    let actor = match principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(error) = AuditLogRepo(pool(state))
        .create(
            None,
            actor,
            "connector.test_delivery",
            Some("observability_connector"),
            Some(id),
            Some(serde_json::json!({"delivered": delivered, "health_status": health_status})),
        )
        .await
    {
        tracing::warn!(error = %error, "failed to write connector audit log");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(endpoint: &str) -> ConnectorInput {
        ConnectorInput {
            name: "primary otel".to_string(),
            kind: OTLP_HTTP.to_string(),
            endpoint: endpoint.to_string(),
            enabled: false,
            sampling_rate: 1.0,
            auth_secret_ref: None,
            managed_auth_secret: None,
        }
    }

    #[test]
    fn connector_validation_accepts_opt_in_otlp_http() {
        assert!(validate_input(&input("https://otel.example/v1/logs")).is_ok());
    }

    #[test]
    fn connector_validation_rejects_unusable_endpoints_and_sampling() {
        assert!(validate_input(&input("file:///tmp/collector")).is_err());
        assert!(validate_input(&input("https://token@example.com/v1/logs")).is_err());
        let mut invalid_sampling = input("https://otel.example/v1/logs");
        invalid_sampling.sampling_rate = 1.1;
        assert!(validate_input(&invalid_sampling).is_err());
    }

    #[test]
    fn connector_secrets_are_never_serialized() {
        let connector = Connector {
            id: Uuid::nil(),
            name: "otlp".to_string(),
            kind: OTLP_HTTP.to_string(),
            endpoint: "https://otel.example/v1/logs".to_string(),
            enabled: true,
            sampling_rate: 0.5,
            auth_secret_ref: None,
            auth_secret_configured: true,
            health_status: "unknown".to_string(),
            health_checked_at: None,
            health_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let serialized = serde_json::to_string(&connector).unwrap();
        assert!(!serialized.contains("managed_auth_secret"));
        assert!(!serialized.contains("ciphertext"));
    }

    #[tokio::test]
    async fn test_delivery_uses_otlp_json_and_bearer_auth() {
        use axum::http::{HeaderMap, StatusCode};

        let app = Router::new().route(
            "/v1/logs",
            post(
                |headers: HeaderMap, Json(payload): Json<serde_json::Value>| async move {
                    assert_eq!(headers["authorization"], "Bearer managed-token");
                    assert_eq!(
                        payload["resourceLogs"][0]["scopeLogs"][0]["scope"]["name"],
                        "rolter-control"
                    );
                    StatusCode::ACCEPTED
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = ControlState {
            store: std::sync::Arc::new(rolter_store::InMemoryConfigStore::new(
                rolter_core::GatewayConfig::default(),
            )),
            config_owned: std::sync::Arc::new(crate::ConfigOwned::default()),
            redis: None,
            clickhouse: None,
            admin_token: None,
            http: reqwest::Client::new(),
            gateway_url: std::sync::Arc::new("http://localhost:4000".to_string()),
            pool: None,
        };
        send_test(
            &state,
            &format!("http://{addr}/v1/logs"),
            Some("managed-token"),
        )
        .await
        .unwrap();
    }
}
