//! Outbound observability connectors (#511).
//!
//! rolter keeps analytics in ClickHouse and, before this, had no export path at
//! all. A connector says where a deployment additionally ships its telemetry:
//! endpoint, auth, sampling rate, on or off.
//!
//! **Strictly opt-in and off by default.** An air-gapped deployment must keep
//! working, so a deployment with no connectors has no egress path, and a
//! connector row with `enabled = false` sends nothing. Creating one does not
//! turn it on.
//!
//! Two things are deliberately not left to trust:
//!
//! - **The endpoint goes through [`rolter_core::EgressPolicy`]**, the same check
//!   a provider `api_base` passes. A connector endpoint is an operator-supplied
//!   URL the deployment will POST to, which is the same SSRF surface, so it gets
//!   the same guard rather than a weaker one.
//! - **The credential is write-only.** A managed secret is sealed with the
//!   deployment KEK before it reaches Postgres and is never returned by the API
//!   or placed in an audit detail; the API reports only whether one is
//!   configured. An external secret-manager reference is kept as an opaque
//!   string, exactly as the provider path does.

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
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

/// Connector kinds this build can deliver to.
///
/// OTLP first, as the issue sequences it: it is the one format every other sink
/// can be reached through via a collector, so shipping it first makes Datadog,
/// Prometheus remote-write and Langfuse a configuration change rather than more
/// code. Widening this list also means widening the check constraint in
/// migration `0062`.
const KINDS: &[&str] = &["otlp_http"];

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/connectors", get(list).post(create))
        .route(
            "/api/v1/connectors/{id}",
            axum::routing::put(update).delete(remove),
        )
        .route("/api/v1/connectors/{id}/test", post(test_delivery))
}

#[derive(Serialize, sqlx::FromRow)]
struct Connector {
    id: Uuid,
    name: String,
    kind: String,
    endpoint: String,
    enabled: bool,
    sampling_rate: f64,
    auth_secret_ref: Option<String>,
    /// whether a managed secret is stored — never the secret itself
    auth_secret_configured: bool,
    health_status: String,
    health_checked_at: Option<DateTime<Utc>>,
    health_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ConnectorInput {
    name: String,
    kind: String,
    endpoint: String,
    enabled: bool,
    sampling_rate: f64,
    #[serde(default)]
    auth_secret_ref: Option<String>,
    /// write-only bearer token; sealed before it reaches Postgres
    #[serde(default)]
    managed_auth_secret: Option<String>,
}

fn columns() -> &'static str {
    "id, name, kind, endpoint, enabled, sampling_rate, auth_secret_ref, \
     auth_ciphertext is not null as auth_secret_configured, health_status, \
     health_checked_at, health_error, created_at, updated_at"
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate(input: &ConnectorInput, egress: &rolter_core::EgressPolicy) -> ApiResult<()> {
    if input.name.trim().is_empty() {
        return Err(invalid("connector name must not be empty"));
    }
    if !KINDS.contains(&input.kind.as_str()) {
        return Err(invalid(format!("kind must be one of {KINDS:?}")));
    }
    let endpoint = input.endpoint.trim();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err(invalid("endpoint must be an http(s) URL"));
    }
    // the same guard a provider api_base passes: this is a URL the deployment
    // will POST telemetry to, so it is the same SSRF surface
    if let Err(problem) = egress.check_url(endpoint, "connector endpoint") {
        return Err(invalid(problem));
    }
    if !input.sampling_rate.is_finite() || !(0.0..=1.0).contains(&input.sampling_rate) {
        return Err(invalid("sampling_rate must be between 0.0 and 1.0"));
    }
    Ok(())
}

fn seal(secret: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    if secret.trim().is_empty() {
        return Err(invalid("managed_auth_secret must not be empty"));
    }
    let Some(kek) = Kek::from_env() else {
        return Err(invalid(format!(
            "storing connector credentials requires {KEK_ENV}"
        )));
    };
    kek.encrypt(secret).map_err(|_| {
        ApiError::Core(Error::Store(
            "failed to encrypt connector credential".into(),
        ))
    })
}

async fn list(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Connector>>> {
    authorize_superadmin(&principal, superadmin_cap!("connector", Read))?;
    Ok(Json(
        sqlx::query_as(&format!(
            "select {} from observability_connectors order by name",
            columns()
        ))
        .fetch_all(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?,
    ))
}

async fn create(
    principal: Principal,
    State(state): State<ControlState>,
    Json(input): Json<ConnectorInput>,
) -> ApiResult<Json<Connector>> {
    authorize_superadmin(&principal, superadmin_cap!("connector", Create))?;
    validate(&input, &state.egress)?;
    let secret = input.managed_auth_secret.as_deref().map(seal).transpose()?;
    let connector: Connector = sqlx::query_as(&format!(
        "insert into observability_connectors \
         (name, kind, endpoint, enabled, sampling_rate, auth_secret_ref, auth_ciphertext, auth_nonce) \
         values ($1, $2, $3, $4, $5, $6, $7, $8) returning {}",
        columns()
    ))
    .bind(input.name.trim())
    .bind(input.kind.trim())
    .bind(input.endpoint.trim())
    .bind(input.enabled)
    .bind(input.sampling_rate)
    .bind(input.auth_secret_ref.as_deref())
    .bind(secret.as_ref().map(|(c, _)| c.as_slice()))
    .bind(secret.as_ref().map(|(_, n)| n.as_slice()))
    .fetch_one(pool(&state))
    .await
    .map_err(|e| Error::Store(e.to_string()))?;
    audit(&state, &principal, "connector.create", &connector).await;
    Ok(Json(connector))
}

async fn update(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ConnectorInput>,
) -> ApiResult<Json<Connector>> {
    authorize_superadmin(&principal, superadmin_cap!("connector", Update))?;
    validate(&input, &state.egress)?;
    let secret = input.managed_auth_secret.as_deref().map(seal).transpose()?;
    // coalesce leaves an existing secret in place when the caller does not
    // resend it, so editing the sampling rate cannot silently drop the
    // credential the connector needs
    let connector: Connector = sqlx::query_as(&format!(
        "update observability_connectors set name=$2, kind=$3, endpoint=$4, enabled=$5, \
         sampling_rate=$6, auth_secret_ref=$7, \
         auth_ciphertext=coalesce($8, auth_ciphertext), auth_nonce=coalesce($9, auth_nonce), \
         updated_at=now() where id=$1 returning {}",
        columns()
    ))
    .bind(id)
    .bind(input.name.trim())
    .bind(input.kind.trim())
    .bind(input.endpoint.trim())
    .bind(input.enabled)
    .bind(input.sampling_rate)
    .bind(input.auth_secret_ref.as_deref())
    .bind(secret.as_ref().map(|(c, _)| c.as_slice()))
    .bind(secret.as_ref().map(|(_, n)| n.as_slice()))
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| Error::Store(e.to_string()))?
    .ok_or_else(|| Error::NotFound(format!("connector {id}")))?;
    audit(&state, &principal, "connector.update", &connector).await;
    Ok(Json(connector))
}

async fn remove(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("connector", Delete))?;
    if sqlx::query("delete from observability_connectors where id=$1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?
        .rows_affected()
        == 0
    {
        return Err(ApiError::Core(Error::NotFound(format!("connector {id}"))));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct TestResult {
    delivered: bool,
    health_status: String,
    health_checked_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_error: Option<String>,
}

/// Deliver one probe record and record the outcome as the connector's health.
///
/// This is what stops a connector being the same class of silent no-op as
/// `security_settings.allowed_headers` was (#813): an operator can find out
/// whether the endpoint and credential actually work, at the moment they
/// configure them, rather than discovering it from missing data later.
///
/// Runs regardless of `enabled`, because the point is to check a connector
/// before turning it on.
async fn test_delivery(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<TestResult>> {
    authorize_superadmin(&principal, superadmin_cap!("connector", Update))?;

    let row: (String, String, Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
        "select kind, endpoint, auth_ciphertext, auth_nonce from observability_connectors where id=$1",
    )
    .bind(id)
    .fetch_optional(pool(&state))
    .await
    .map_err(|e| Error::Store(e.to_string()))?
    .ok_or_else(|| Error::NotFound(format!("connector {id}")))?;
    let (kind, endpoint, ciphertext, nonce) = row;

    // the endpoint was checked when it was stored, but the policy may have been
    // tightened since; a stored row is not a standing permission to egress
    if let Err(problem) = state.egress.check_url(&endpoint, "connector endpoint") {
        return Err(invalid(problem));
    }

    let token = match (ciphertext, nonce) {
        (Some(c), Some(n)) => Kek::from_env()
            .and_then(|kek| kek.decrypt(&c, &n).ok())
            .map(|secret| format!("Bearer {secret}")),
        _ => None,
    };

    let outcome = deliver_probe(&state.http, &kind, &endpoint, token.as_deref()).await;
    let checked_at = Utc::now();
    let (status, error) = match &outcome {
        Ok(()) => ("healthy", None),
        Err(message) => ("unhealthy", Some(message.clone())),
    };

    sqlx::query(
        "update observability_connectors set health_status=$2, health_checked_at=$3, \
         health_error=$4, updated_at=now() where id=$1",
    )
    .bind(id)
    .bind(status)
    .bind(checked_at)
    .bind(error.as_deref())
    .execute(pool(&state))
    .await
    .map_err(|e| Error::Store(e.to_string()))?;

    Ok(Json(TestResult {
        delivered: outcome.is_ok(),
        health_status: status.to_string(),
        health_checked_at: checked_at,
        health_error: error,
    }))
}

/// POST one empty OTLP logs payload and report whether the sink accepted it.
///
/// An empty `resourceLogs` is a well-formed OTLP request that delivers no data,
/// so a test cannot pollute the operator's backend with synthetic records while
/// still exercising the whole path: DNS, TLS, the URL, and the credential.
async fn deliver_probe(
    http: &reqwest::Client,
    kind: &str,
    endpoint: &str,
    authorization: Option<&str>,
) -> Result<(), String> {
    if kind != "otlp_http" {
        return Err(format!("unsupported connector kind '{kind}'"));
    }
    let mut request = http
        .post(endpoint)
        .timeout(std::time::Duration::from_secs(10))
        .json(&serde_json::json!({ "resourceLogs": [] }));
    if let Some(value) = authorization {
        request = request.header(axum::http::header::AUTHORIZATION, value);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => Ok(()),
        // the status alone, never the body: a sink's error body can echo the
        // credential it just rejected
        Ok(response) => Err(format!("sink returned HTTP {}", response.status().as_u16())),
        Err(err) if err.is_timeout() => Err("timed out reaching the sink".to_string()),
        Err(err) if err.is_connect() => Err("could not connect to the sink".to_string()),
        Err(_) => Err("delivery failed".to_string()),
    }
}

/// Audit a connector write. Endpoint and credential are deliberately absent:
/// the endpoint can carry a token in its query string, and the audit log is a
/// wider-read surface than the connector table.
async fn audit(state: &ControlState, principal: &Principal, action: &str, connector: &Connector) {
    let actor = match principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    if let Err(err) = AuditLogRepo(pool(state))
        .create(
            None,
            actor,
            action,
            Some("connector"),
            Some(connector.id),
            Some(serde_json::json!({
                "name": connector.name,
                "kind": connector.kind,
                "enabled": connector.enabled,
                "sampling_rate": connector.sampling_rate,
                "auth_secret_configured": connector.auth_secret_configured,
            })),
        )
        .await
    {
        tracing::warn!(error = %err, "failed to write connector audit entry");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(endpoint: &str, sampling_rate: f64) -> ConnectorInput {
        ConnectorInput {
            name: "signoz".to_string(),
            kind: "otlp_http".to_string(),
            endpoint: endpoint.to_string(),
            enabled: false,
            sampling_rate,
            auth_secret_ref: None,
            managed_auth_secret: None,
        }
    }

    #[test]
    fn a_well_formed_connector_validates() {
        let egress = rolter_core::EgressPolicy::default();
        assert!(validate(
            &input("https://collector.example.com/v1/logs", 1.0),
            &egress
        )
        .is_ok());
    }

    #[test]
    fn a_non_http_endpoint_is_rejected() {
        // the endpoint is a URL the deployment will POST to; anything that is
        // not http(s) has no business here
        let egress = rolter_core::EgressPolicy::default();
        for endpoint in ["file:///etc/passwd", "collector.example.com", ""] {
            assert!(
                validate(&input(endpoint, 1.0), &egress).is_err(),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn the_endpoint_goes_through_the_egress_policy() {
        // the SSRF guard a provider api_base passes must apply here too — a
        // connector endpoint is the same primitive
        let egress = rolter_core::EgressPolicy::default();
        let blocked = validate(
            &input("http://169.254.169.254/latest/meta-data", 1.0),
            &egress,
        );
        assert!(
            blocked.is_err(),
            "the cloud metadata endpoint must not be reachable as a connector"
        );
    }

    #[test]
    fn the_sampling_rate_is_bounded() {
        let egress = rolter_core::EgressPolicy::default();
        for rate in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert!(
                validate(&input("https://collector.example.com", rate), &egress).is_err(),
                "{rate}"
            );
        }
        for rate in [0.0, 0.5, 1.0] {
            assert!(
                validate(&input("https://collector.example.com", rate), &egress).is_ok(),
                "{rate}"
            );
        }
    }

    #[test]
    fn an_unknown_kind_is_rejected() {
        let egress = rolter_core::EgressPolicy::default();
        let mut bad = input("https://collector.example.com", 1.0);
        bad.kind = "datadog".to_string();
        assert!(validate(&bad, &egress).is_err());
    }
}
