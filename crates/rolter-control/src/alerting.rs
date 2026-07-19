//! Opt-in alert-rule management and bounded ClickHouse evaluation.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::repo::AuditLogRepo;

use crate::analytics::client_or_503;
use crate::crud::{pool, ApiError, ApiResult};
use crate::rbac::{require_superadmin, Principal};
use crate::ControlState;

const SIGNALS: &[&str] = &[
    "error_rate",
    "p95_latency_ms",
    "spend_velocity",
    "request_volume",
    "provider_health_flaps",
];

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/alert-channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/api/v1/alert-channels/{id}",
            axum::routing::put(update_channel).delete(delete_channel),
        )
        .route("/api/v1/alert-rules", get(list_rules).post(create_rule))
        .route(
            "/api/v1/alert-rules/{id}",
            axum::routing::put(update_rule).delete(delete_rule),
        )
        .route("/api/v1/alert-rules/{id}/evaluate", post(evaluate))
        .route("/api/v1/alert-notifications", get(list_history))
}

/// Start the control-plane evaluator. It is deliberately absent when no DB is
/// configured and does no network work until at least one rule is enabled.
pub(crate) fn start_evaluator(state: ControlState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = evaluate_enabled(&state).await {
                tracing::warn!(error = %error, "alert rule evaluation pass failed");
            }
        }
    });
}

#[derive(Serialize, sqlx::FromRow)]
struct Channel {
    id: Uuid,
    name: String,
    kind: String,
    endpoint: String,
    enabled: bool,
    secret_configured: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Rule {
    id: Uuid,
    name: String,
    signal: String,
    threshold: f64,
    window_secs: i32,
    channel_id: Option<Uuid>,
    enabled: bool,
    state: String,
    last_value: Option<f64>,
    last_evaluated_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
struct Notification {
    id: Uuid,
    rule_id: Uuid,
    channel_id: Option<Uuid>,
    state: String,
    delivery_status: String,
    detail: Option<String>,
    sent_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ChannelInput {
    name: String,
    endpoint: String,
    #[serde(default)]
    enabled: bool,
    managed_secret: Option<String>,
}

#[derive(Deserialize)]
struct RuleInput {
    name: String,
    signal: String,
    threshold: f64,
    window_secs: i32,
    channel_id: Option<Uuid>,
    #[serde(default)]
    enabled: bool,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_name(value: &str, field: &str) -> ApiResult<()> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(invalid(format!("{field} must be 1-128 visible characters")));
    }
    Ok(())
}

fn validate_channel(input: &ChannelInput) -> ApiResult<()> {
    validate_name(&input.name, "channel name")?;
    let endpoint = reqwest::Url::parse(input.endpoint.trim())
        .map_err(|_| invalid("channel endpoint must be a valid http(s) URL"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(invalid(
            "channel endpoint must be an http(s) URL without userinfo",
        ));
    }
    if input
        .managed_secret
        .as_ref()
        .is_some_and(|secret| secret.trim().is_empty())
    {
        return Err(invalid("managed_secret must not be empty"));
    }
    Ok(())
}

fn validate_rule(input: &RuleInput) -> ApiResult<()> {
    validate_name(&input.name, "rule name")?;
    if !SIGNALS.contains(&input.signal.as_str()) {
        return Err(invalid(format!("signal must be one of {SIGNALS:?}")));
    }
    if !input.threshold.is_finite() || input.threshold < 0.0 {
        return Err(invalid("threshold must be a finite non-negative number"));
    }
    if !(60..=86_400).contains(&input.window_secs) {
        return Err(invalid("window_secs must be between 60 and 86400"));
    }
    Ok(())
}

fn seal(secret: &str) -> ApiResult<(Vec<u8>, Vec<u8>)> {
    let Some(kek) = Kek::from_env() else {
        return Err(invalid(format!(
            "storing channel credentials requires {KEK_ENV}"
        )));
    };
    kek.encrypt(secret)
        .map_err(|_| ApiError::Core(Error::Store("failed to encrypt channel credential".into())))
}

fn channel_columns() -> &'static str {
    "id, name, kind, endpoint, enabled, secret_ciphertext is not null as secret_configured, created_at, updated_at"
}

fn rule_columns() -> &'static str {
    "id, name, signal, threshold, window_secs, channel_id, enabled, state, last_value, last_evaluated_at, last_error, created_at, updated_at"
}

async fn list_channels(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Channel>>> {
    require_superadmin(&principal)?;
    Ok(Json(
        sqlx::query_as(&format!(
            "select {} from alert_channels order by name",
            channel_columns()
        ))
        .fetch_all(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?,
    ))
}

async fn create_channel(
    principal: Principal,
    State(state): State<ControlState>,
    Json(input): Json<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    require_superadmin(&principal)?;
    validate_channel(&input)?;
    let secret = input.managed_secret.as_deref().map(seal).transpose()?;
    let channel: Channel = sqlx::query_as(&format!(
        "insert into alert_channels (name, kind, endpoint, enabled, secret_ciphertext, secret_nonce) values ($1, 'webhook', $2, $3, $4, $5) returning {}", channel_columns()))
        .bind(input.name.trim()).bind(input.endpoint.trim()).bind(input.enabled)
        .bind(secret.as_ref().map(|(c, _)| c.as_slice())).bind(secret.as_ref().map(|(_, n)| n.as_slice()))
        .fetch_one(pool(&state)).await.map_err(|e| Error::Store(e.to_string()))?;
    audit(
        &state,
        &principal,
        "alert.channel.create",
        channel.id,
        serde_json::json!({"name": channel.name, "enabled": channel.enabled}),
    )
    .await;
    Ok(Json(channel))
}

async fn update_channel(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(input): Json<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    require_superadmin(&principal)?;
    validate_channel(&input)?;
    let secret = input.managed_secret.as_deref().map(seal).transpose()?;
    let channel: Channel = sqlx::query_as(&format!(
        "update alert_channels set name=$2, endpoint=$3, enabled=$4, secret_ciphertext=coalesce($5, secret_ciphertext), secret_nonce=coalesce($6, secret_nonce), updated_at=now() where id=$1 returning {}", channel_columns()))
        .bind(id).bind(input.name.trim()).bind(input.endpoint.trim()).bind(input.enabled)
        .bind(secret.as_ref().map(|(c, _)| c.as_slice())).bind(secret.as_ref().map(|(_, n)| n.as_slice()))
        .fetch_optional(pool(&state)).await.map_err(|e| Error::Store(e.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("alert channel {id}")))?;
    audit(
        &state,
        &principal,
        "alert.channel.update",
        id,
        serde_json::json!({"name": channel.name, "enabled": channel.enabled}),
    )
    .await;
    Ok(Json(channel))
}

async fn delete_channel(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
    if sqlx::query("delete from alert_channels where id=$1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?
        .rows_affected()
        == 0
    {
        return Err(ApiError::Core(Error::NotFound(format!(
            "alert channel {id}"
        ))));
    }
    audit(
        &state,
        &principal,
        "alert.channel.delete",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_rules(
    principal: Principal,
    State(state): State<ControlState>,
) -> ApiResult<Json<Vec<Rule>>> {
    require_superadmin(&principal)?;
    Ok(Json(
        sqlx::query_as(&format!(
            "select {} from alert_rules order by name",
            rule_columns()
        ))
        .fetch_all(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?,
    ))
}

async fn create_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Json<Rule>> {
    require_superadmin(&principal)?;
    validate_rule(&input)?;
    let rule: Rule = sqlx::query_as(&format!("insert into alert_rules (name, signal, threshold, window_secs, channel_id, enabled) values ($1,$2,$3,$4,$5,$6) returning {}", rule_columns()))
        .bind(input.name.trim()).bind(&input.signal).bind(input.threshold).bind(input.window_secs).bind(input.channel_id).bind(input.enabled)
        .fetch_one(pool(&state)).await.map_err(|e| Error::Store(e.to_string()))?;
    audit(
        &state,
        &principal,
        "alert.rule.create",
        rule.id,
        serde_json::json!({"name": rule.name, "signal": rule.signal, "enabled": rule.enabled}),
    )
    .await;
    Ok(Json(rule))
}

async fn update_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    Json(input): Json<RuleInput>,
) -> ApiResult<Json<Rule>> {
    require_superadmin(&principal)?;
    validate_rule(&input)?;
    let rule: Rule = sqlx::query_as(&format!("update alert_rules set name=$2, signal=$3, threshold=$4, window_secs=$5, channel_id=$6, enabled=$7, updated_at=now() where id=$1 returning {}", rule_columns()))
        .bind(id).bind(input.name.trim()).bind(&input.signal).bind(input.threshold).bind(input.window_secs).bind(input.channel_id).bind(input.enabled)
        .fetch_optional(pool(&state)).await.map_err(|e| Error::Store(e.to_string()))?
        .ok_or_else(|| Error::NotFound(format!("alert rule {id}")))?;
    audit(
        &state,
        &principal,
        "alert.rule.update",
        id,
        serde_json::json!({"name": rule.name, "signal": rule.signal, "enabled": rule.enabled}),
    )
    .await;
    Ok(Json(rule))
}

async fn delete_rule(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
    if sqlx::query("delete from alert_rules where id=$1")
        .bind(id)
        .execute(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?
        .rows_affected()
        == 0
    {
        return Err(ApiError::Core(Error::NotFound(format!("alert rule {id}"))));
    }
    audit(
        &state,
        &principal,
        "alert.rule.delete",
        id,
        serde_json::json!({}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<i64>,
    rule_id: Option<Uuid>,
}
async fn list_history(
    principal: Principal,
    State(state): State<ControlState>,
    Query(query): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<Notification>>> {
    require_superadmin(&principal)?;
    let history = sqlx::query_as("select id, rule_id, channel_id, state, delivery_status, detail, sent_at from alert_notification_history where ($1::uuid is null or rule_id=$1) order by sent_at desc limit $2")
        .bind(query.rule_id).bind(query.limit.unwrap_or(100).clamp(1, 500)).fetch_all(pool(&state)).await.map_err(|e| Error::Store(e.to_string()))?;
    Ok(Json(history))
}

#[derive(Serialize)]
struct Evaluation {
    rule: Rule,
    notified: bool,
}
async fn evaluate(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Evaluation>> {
    require_superadmin(&principal)?;
    Ok(Json(evaluate_rule(&state, id).await?))
}

async fn evaluate_enabled(state: &ControlState) -> Result<(), Error> {
    let ids: Vec<(Uuid,)> = sqlx::query_as("select id from alert_rules where enabled")
        .fetch_all(pool(state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?;
    for (id,) in ids {
        if let Err(error) = evaluate_rule(state, id).await {
            tracing::warn!(%id, error = ?error, "alert rule evaluation failed");
        }
    }
    Ok(())
}

async fn evaluate_rule(state: &ControlState, id: Uuid) -> ApiResult<Evaluation> {
    let previous: Rule = sqlx::query_as(&format!(
        "select {} from alert_rules where id=$1",
        rule_columns()
    ))
    .bind(id)
    .fetch_optional(pool(state))
    .await
    .map_err(|e| Error::Store(e.to_string()))?
    .ok_or_else(|| Error::NotFound(format!("alert rule {id}")))?;
    let ch = client_or_503(state).map_err(|_| {
        ApiError::Core(Error::Store(
            "alert evaluation requires CLICKHOUSE_URL".into(),
        ))
    })?;
    let sql = metric_sql(&previous.signal, previous.window_secs)?;
    let rows = ch
        .query(&sql, &[])
        .await
        .map_err(|e| Error::Store(e.to_string()))?;
    let value = rows
        .first()
        .and_then(|row| row.get("value"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let next_state = if value >= previous.threshold {
        "firing"
    } else {
        "ok"
    };
    let rule: Rule = sqlx::query_as(&format!("update alert_rules set state=$2,last_value=$3,last_evaluated_at=now(),last_error=null,updated_at=now() where id=$1 returning {}", rule_columns())).bind(id).bind(next_state).bind(value).fetch_one(pool(state)).await.map_err(|e| Error::Store(e.to_string()))?;
    let notified = previous.state != next_state
        && (next_state == "firing" || previous.state == "firing")
        && deliver_transition(state, &rule).await?;
    Ok(Evaluation { rule, notified })
}

fn metric_sql(signal: &str, window_secs: i32) -> ApiResult<String> {
    let window = window_secs.clamp(60, 86_400);
    let expression = match signal {
        "error_rate" => "if(count() = 0, 0, countIf(status >= 500) / count())".to_string(),
        "p95_latency_ms" => "if(count() = 0, 0, quantile(0.95)(latency_ms))".to_string(),
        "spend_velocity" => format!("sum(cost_usd) * 3600 / {window}"),
        "request_volume" => "count()".to_string(),
        "provider_health_flaps" => "countIf(outcome != 'ok')".to_string(),
        _ => return Err(invalid("unknown alert signal")),
    };
    let table = if signal == "provider_health_flaps" {
        "provider_health_events"
    } else {
        "request_logs"
    };
    Ok(format!("select toFloat64({expression}) as value from {table} where ts >= now64(3) - interval {window} second format JSON"))
}

async fn deliver_transition(state: &ControlState, rule: &Rule) -> ApiResult<bool> {
    let delivery = if let Some(channel_id) = rule.channel_id {
        let row: Option<(String, bool)> =
            sqlx::query_as("select endpoint, enabled from alert_channels where id=$1")
                .bind(channel_id)
                .fetch_optional(pool(state))
                .await
                .map_err(|e| Error::Store(e.to_string()))?;
        match row {
            Some((_endpoint, true)) => (
                "skipped",
                Some("webhook delivery worker not configured".to_string()),
            ),
            Some(_) => ("skipped", Some("channel disabled".to_string())),
            None => ("skipped", Some("channel deleted".to_string())),
        }
    } else {
        ("skipped", Some("no channel configured".to_string()))
    };
    sqlx::query("insert into alert_notification_history (rule_id, channel_id, state, delivery_status, detail) values ($1,$2,$3,$4,$5)")
        .bind(rule.id).bind(rule.channel_id).bind(if rule.state == "firing" { "firing" } else { "resolved" }).bind(delivery.0).bind(delivery.1).execute(pool(state)).await.map_err(|e| Error::Store(e.to_string()))?;
    Ok(false)
}

async fn audit(
    state: &ControlState,
    principal: &Principal,
    action: &str,
    target: Uuid,
    detail: serde_json::Value,
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
            Some("alerting"),
            Some(target),
            Some(detail),
        )
        .await
    {
        tracing::warn!(error = %error, action, "failed to write alert audit log");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_reject_unsafe_bounds() {
        let bad = RuleInput {
            name: "x".into(),
            signal: "error_rate".into(),
            threshold: -1.0,
            window_secs: 30,
            channel_id: None,
            enabled: false,
        };
        assert!(validate_rule(&bad).is_err());
        assert!(metric_sql("error_rate", 60)
            .unwrap()
            .contains("request_logs"));
        assert!(metric_sql("provider_health_flaps", 60)
            .unwrap()
            .contains("provider_health_events"));
    }
    #[test]
    fn channels_accept_exact_http_endpoints() {
        let channel = ChannelInput {
            name: "ops".into(),
            endpoint: "https://hooks.example/rolter".into(),
            enabled: false,
            managed_secret: None,
        };
        assert!(validate_channel(&channel).is_ok());
    }
}
