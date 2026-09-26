//! Opt-in alert-rule management, bounded ClickHouse evaluation and webhook
//! delivery of state transitions.
//!
//! A rule reads one signal over a trailing window and is `firing` when the
//! value reaches its threshold, `ok` otherwise. A change between the two is a
//! transition: it is POSTed to the rule's channel when that channel is enabled,
//! and recorded in `alert_notification_history` as `delivered`, `failed` or
//! `skipped` either way.
//!
//! Three things are deliberately not left to trust:
//!
//! - **The endpoint goes through [`rolter_core::EgressPolicy`]** when the
//!   channel is saved and again at every delivery, the same check a connector
//!   endpoint gets, and redirects are not followed.
//! - **The credential is write-only.** A channel secret is sealed with the
//!   deployment KEK, unsealed only to build the `Authorization` header, and
//!   never returned by the API or placed in an audit entry or history row.
//! - **A receiver's response body is never stored.** The history keeps the HTTP
//!   status or the class of transport failure, since an error body can echo the
//!   credential it just rejected.

use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::{EgressPolicy, Error};
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::repo::AuditLogRepo;

use crate::crud::{pool, ApiError, ApiResult};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::ControlState;

/// Longest one webhook delivery may take, connect included. The evaluator waits
/// on it while holding the rule's row lock, so a slow receiver must not hold
/// that lock for long.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a signal query may run. The ClickHouse client has no timeout of its
/// own, and a pass that hangs on one rule never reaches the rest.
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest `last_error` stored, in bytes.
const MAX_ERROR_LEN: usize = 512;

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

fn validate_channel(input: &ChannelInput, egress: &EgressPolicy) -> ApiResult<()> {
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
    // the same guard a connector endpoint gets: an operator-supplied URL the
    // control plane will POST to is an SSRF surface
    if let Err(problem) = egress.check_url(input.endpoint.trim(), "channel endpoint") {
        return Err(invalid(problem));
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
    authorize_superadmin(&principal, superadmin_cap!("alert_channel", Read))?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_channel", Create))?;
    validate_channel(&input, &state.egress)?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_channel", Update))?;
    validate_channel(&input, &state.egress)?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_channel", Delete))?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_rule", Read))?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_rule", Create))?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_rule", Update))?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_rule", Delete))?;
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
    authorize_superadmin(&principal, superadmin_cap!("alert_history", Read))?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    let history = if let Some(rule_id) = query.rule_id {
        sqlx::query_as(
            "select id, rule_id, channel_id, state, delivery_status, detail, sent_at \
             from alert_notification_history \
             where rule_id=$1 \
             order by sent_at desc \
             limit $2",
        )
        .bind(rule_id)
        .bind(limit)
        .fetch_all(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?
    } else {
        sqlx::query_as(
            "select id, rule_id, channel_id, state, delivery_status, detail, sent_at \
             from alert_notification_history \
             order by sent_at desc \
             limit $1",
        )
        .bind(limit)
        .fetch_all(pool(&state))
        .await
        .map_err(|e| Error::Store(e.to_string()))?
    };
    Ok(Json(history))
}

#[derive(Serialize)]
struct Evaluation {
    rule: Rule,
    /// Whether this evaluation delivered a transition to a channel. False for a
    /// transition recorded as `failed` or `skipped`.
    notified: bool,
    /// The history row this evaluation wrote, when the reading was a transition.
    notification: Option<Notification>,
}

async fn evaluate(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Evaluation>> {
    authorize_superadmin(&principal, superadmin_cap!("alert_history", Create))?;
    evaluate_one(&state, id, Lock::Wait, Kek::from_env().as_ref())
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::Core(Error::NotFound(format!("alert rule {id}"))))
}

async fn evaluate_enabled(state: &ControlState) -> Result<(), Error> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("select id from alert_rules where enabled order by name")
            .fetch_all(pool(state))
            .await
            .map_err(store_error)?;
    let kek = Kek::from_env();
    for id in ids {
        if let Err(error) = evaluate_one(state, id, Lock::SkipLocked, kek.as_ref()).await {
            tracing::warn!(%id, error = ?error, "alert rule evaluation failed");
        }
    }
    Ok(())
}

fn store_error(error: sqlx::Error) -> Error {
    Error::Store(error.to_string())
}

/// How [`evaluate_one`] takes the rule's row lock.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lock {
    /// Wait for it. **Evaluate now** runs after whichever pass holds the rule,
    /// and evaluates it whether or not it is enabled.
    Wait,
    /// Leave a rule another pass holds to that pass, and skip one that is no
    /// longer enabled. The scheduled evaluator runs this way.
    SkipLocked,
}

/// Evaluate one rule, then deliver and record whatever changed.
///
/// `None` when the rule is gone, or, under [`Lock::SkipLocked`], when it is
/// disabled or another evaluation holds it.
///
/// The ClickHouse read happens before any lock is taken, because it is the slow
/// part. Everything after it runs in one transaction holding the rule's row
/// lock: deciding whether the reading is a transition, delivering it, and
/// writing the rule and its history row. Every control-plane replica runs an
/// evaluator, and the lock is what stops two of them that read the same state
/// from reporting the same change twice.
async fn evaluate_one(
    state: &ControlState,
    id: Uuid,
    lock: Lock,
    kek: Option<&Kek>,
) -> ApiResult<Option<Evaluation>> {
    let select = format!(
        "select {} from alert_rules where id=$1{}",
        rule_columns(),
        if lock == Lock::SkipLocked {
            " and enabled"
        } else {
            ""
        }
    );
    let Some(unlocked): Option<Rule> = sqlx::query_as(&select)
        .bind(id)
        .fetch_optional(pool(state))
        .await
        .map_err(store_error)?
    else {
        return Ok(None);
    };
    let reading = read_signal(state, &unlocked.signal, unlocked.window_secs).await;

    let mut tx = pool(state).begin().await.map_err(store_error)?;
    let locking = match lock {
        Lock::Wait => "for update",
        Lock::SkipLocked => "for update skip locked",
    };
    let Some(rule): Option<Rule> = sqlx::query_as(&format!("{select} {locking}"))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_error)?
    else {
        return Ok(None);
    };
    // an edit landed between the read and the lock, so the reading measures a
    // query the rule no longer asks for; the next pass reads the new one
    if rule.signal != unlocked.signal || rule.window_secs != unlocked.window_secs {
        return Ok(Some(Evaluation {
            rule,
            notified: false,
            notification: None,
        }));
    }

    let value = match reading {
        Ok(value) => value,
        Err(reason) => {
            // last_value and last_evaluated_at stay as the last successful
            // reading left them, so the card still says what was last known
            sqlx::query(
                "update alert_rules set state='error', last_error=$2, updated_at=now() where id=$1",
            )
            .bind(id)
            .bind(&reason)
            .execute(&mut *tx)
            .await
            .map_err(store_error)?;
            tx.commit().await.map_err(store_error)?;
            return Err(ApiError::Core(Error::Upstream(reason)));
        }
    };
    let next_state = if value >= rule.threshold {
        "firing"
    } else {
        "ok"
    };
    let last_reported: Option<String> = sqlx::query_scalar(
        "select state from alert_notification_history where rule_id=$1 order by sent_at desc limit 1",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(store_error)?;
    let rule: Rule = sqlx::query_as(&format!(
        "update alert_rules set state=$2, last_value=$3, last_evaluated_at=now(), last_error=null, updated_at=now() where id=$1 returning {}",
        rule_columns()
    ))
    .bind(id)
    .bind(next_state)
    .bind(value)
    .fetch_one(&mut *tx)
    .await
    .map_err(store_error)?;
    let notification = match transition(last_reported.as_deref(), next_state) {
        Some(change) => Some(report(&state.egress, &mut tx, &rule, change, kek).await?),
        None => None,
    };
    tx.commit().await.map_err(store_error)?;
    let notified = notification
        .as_ref()
        .is_some_and(|n| n.delivery_status == "delivered");
    Ok(Some(Evaluation {
        rule,
        notified,
        notification,
    }))
}

/// What a reading reports, given the state the rule's history last recorded.
///
/// Decided against the history rather than the rule's `state` column, which
/// also holds `unknown` and `error`. A rule whose evaluation failed between two
/// firing readings is still the same alert and is not reported again, and one
/// that recovered while its evaluation was failing still owes a `resolved`. A
/// rule with no history that reads high reports `firing`, so a rule enabled
/// while its condition already holds says so on its first pass.
fn transition(last_reported: Option<&str>, next_state: &str) -> Option<&'static str> {
    match (last_reported, next_state) {
        (Some("firing"), "ok") => Some("resolved"),
        (Some("firing"), _) => None,
        (_, "firing") => Some("firing"),
        _ => None,
    }
}

/// Run the rule's signal query, bounded by [`QUERY_TIMEOUT`].
///
/// The error is what the rule's `last_error` shows, so it is kept short and
/// never carries the ClickHouse endpoint (see [`describe_query_error`]).
async fn read_signal(state: &ControlState, signal: &str, window_secs: i32) -> Result<f64, String> {
    let Some(ch) = state.clickhouse.as_ref() else {
        return Err("alert evaluation requires CLICKHOUSE_URL".to_string());
    };
    let sql = metric_sql(signal, window_secs).map_err(|_| format!("unknown signal '{signal}'"))?;
    let rows = match tokio::time::timeout(QUERY_TIMEOUT, ch.query(&sql, &[])).await {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, signal, "alert signal query failed");
            return Err(describe_query_error(&error));
        }
        Err(_) => {
            return Err(format!(
                "analytics query timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            ))
        }
    };
    Ok(rows
        .first()
        .and_then(|row| row.get("value"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0))
}

/// A bounded description of a failed signal query that is safe to store.
///
/// A transport error's message carries the request URL, and `CLICKHOUSE_URL`
/// may hold a password, so those are reduced to their class. A ClickHouse error
/// response is kept, truncated: its status and exception text are what an
/// operator needs to fix the query, and neither names the endpoint.
fn describe_query_error(error: &anyhow::Error) -> String {
    if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        let class = if error.is_timeout() {
            "analytics query timed out"
        } else if error.is_connect() {
            "could not connect to ClickHouse"
        } else if error.is_decode() {
            "ClickHouse returned a response that is not JSON"
        } else {
            "analytics query failed"
        };
        return class.to_string();
    }
    truncate(&error.to_string(), MAX_ERROR_LEN)
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
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

/// The channel columns delivery needs, secret included. Never serialized.
#[derive(sqlx::FromRow)]
struct ChannelTarget {
    id: Uuid,
    endpoint: String,
    enabled: bool,
    secret_ciphertext: Option<Vec<u8>>,
    secret_nonce: Option<Vec<u8>>,
}

/// One history row's `delivery_status` and `detail`.
struct Outcome {
    status: &'static str,
    detail: String,
}

impl Outcome {
    fn delivered(detail: impl Into<String>) -> Self {
        Self {
            status: "delivered",
            detail: detail.into(),
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            status: "failed",
            detail: detail.into(),
        }
    }

    fn skipped(detail: impl Into<String>) -> Self {
        Self {
            status: "skipped",
            detail: detail.into(),
        }
    }
}

/// Deliver `change` to the rule's channel and record the outcome as a history
/// row, inside the evaluation's transaction.
async fn report(
    egress: &EgressPolicy,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule: &Rule,
    change: &'static str,
    kek: Option<&Kek>,
) -> ApiResult<Notification> {
    let id = Uuid::new_v4();
    let channel: Option<ChannelTarget> = match rule.channel_id {
        Some(channel_id) => sqlx::query_as(
            "select id, endpoint, enabled, secret_ciphertext, secret_nonce from alert_channels where id=$1",
        )
        .bind(channel_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(store_error)?,
        None => None,
    };
    let outcome = match (rule.channel_id, &channel) {
        (None, _) => Outcome::skipped("no channel configured"),
        (Some(_), None) => Outcome::skipped("channel deleted"),
        (Some(_), Some(channel)) if !channel.enabled => Outcome::skipped("channel disabled"),
        (Some(_), Some(channel)) => deliver(egress, channel, &payload(id, rule, change), kek).await,
    };
    if outcome.status == "failed" {
        tracing::warn!(rule = %rule.id, change, detail = %outcome.detail, "alert delivery failed");
    }
    // clock_timestamp rather than the column's now() default: now() is when the
    // transaction began, and a transaction that waited for the row lock began
    // before the one it waited for, which would order the history backwards
    let notification: Notification = sqlx::query_as(
        "insert into alert_notification_history (id, rule_id, channel_id, state, delivery_status, detail, sent_at) \
         values ($1, $2, $3, $4, $5, $6, clock_timestamp()) \
         returning id, rule_id, channel_id, state, delivery_status, detail, sent_at",
    )
    .bind(id)
    .bind(rule.id)
    .bind(channel.as_ref().map(|c| c.id))
    .bind(change)
    .bind(outcome.status)
    .bind(&outcome.detail)
    .fetch_one(&mut **tx)
    .await
    .map_err(store_error)?;
    Ok(notification)
}

/// The JSON body a channel receives.
///
/// `id` is the history row's id, so a receiver can match a request to the row
/// the **History** screen shows. `text` is a one-line summary for a relay or a
/// chat tool to display as-is.
fn payload(id: Uuid, rule: &Rule, change: &str) -> serde_json::Value {
    let value = rule.last_value.unwrap_or_default();
    let text = if change == "firing" {
        format!(
            "{} is firing: {} is {value}, threshold {}, {}s window",
            rule.name, rule.signal, rule.threshold, rule.window_secs
        )
    } else {
        format!(
            "{} resolved: {} is {value}, threshold {}, {}s window",
            rule.name, rule.signal, rule.threshold, rule.window_secs
        )
    };
    serde_json::json!({
        "id": id,
        "state": change,
        "rule": { "id": rule.id, "name": rule.name },
        "signal": rule.signal,
        "value": value,
        "threshold": rule.threshold,
        "window_secs": rule.window_secs,
        "evaluated_at": rule.last_evaluated_at,
        "text": text,
    })
}

/// POST one transition to an enabled channel.
///
/// Infallible on purpose: every way delivery can go wrong is an outcome the
/// history records, never an evaluation error, so a dead endpoint cannot stop
/// the rule's state from moving.
async fn deliver(
    egress: &EgressPolicy,
    channel: &ChannelTarget,
    payload: &serde_json::Value,
    kek: Option<&Kek>,
) -> Outcome {
    // checked when the channel was saved, but the policy may have been
    // tightened since; a stored row is not a standing permission to egress
    if let Err(problem) = egress.check_url(&channel.endpoint, "alert channel endpoint") {
        tracing::warn!(channel = %channel.id, %problem, "alert delivery refused by the egress policy");
        return Outcome::failed("endpoint denied by the egress policy");
    }
    let authorization = match (&channel.secret_ciphertext, &channel.secret_nonce) {
        (Some(ciphertext), Some(nonce)) => {
            match kek.and_then(|kek| kek.decrypt(ciphertext, nonce).ok()) {
                Some(secret) => Some(format!("Bearer {secret}")),
                // sending without the credential would hand an unauthenticated
                // alert to a receiver that may well accept it
                None => {
                    return Outcome::failed(format!(
                        "channel secret could not be unsealed; check {KEK_ENV}"
                    ))
                }
            }
        }
        _ => None,
    };
    let Some(client) = delivery_client() else {
        return Outcome::failed("webhook client unavailable");
    };
    let mut request = client.post(&channel.endpoint).json(payload);
    if let Some(value) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            Outcome::delivered(format!("HTTP {}", response.status().as_u16()))
        }
        // the status alone, never the body: a receiver's error body can echo
        // the credential it just rejected
        Ok(response) => Outcome::failed(format!("HTTP {}", response.status().as_u16())),
        Err(error) if error.is_timeout() => {
            Outcome::failed(format!("timed out after {}s", DELIVERY_TIMEOUT.as_secs()))
        }
        Err(error) if error.is_connect() => Outcome::failed("could not connect to the endpoint"),
        Err(_) => Outcome::failed("delivery failed"),
    }
}

/// The client webhooks are sent with.
///
/// Separate from [`ControlState`]'s shared client because it does not follow
/// redirects: a `3xx` is how an endpoint that passed the egress check hands the
/// request to one that would not have, so it is recorded as a failure instead.
/// Bounded end to end, since the evaluator waits on it while holding the
/// rule's row lock.
fn delivery_client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(DELIVERY_TIMEOUT)
                .user_agent(concat!("rolter/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()
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

    fn rule(signal: &str, threshold: f64, window_secs: i32) -> RuleInput {
        RuleInput {
            name: "x".into(),
            signal: signal.into(),
            threshold,
            window_secs,
            channel_id: None,
            enabled: false,
        }
    }

    fn channel(endpoint: &str, managed_secret: Option<String>) -> ChannelInput {
        ChannelInput {
            name: "ops".into(),
            endpoint: endpoint.into(),
            enabled: false,
            managed_secret,
        }
    }

    fn egress() -> EgressPolicy {
        EgressPolicy::default()
    }

    #[test]
    fn rules_reject_negative_threshold() {
        assert!(validate_rule(&rule("error_rate", -1.0, 60)).is_err());
    }

    #[test]
    fn rules_reject_non_finite_threshold() {
        assert!(validate_rule(&rule("error_rate", f64::NAN, 60)).is_err());
    }

    #[test]
    fn rules_reject_out_of_bounds_window_secs() {
        assert!(validate_rule(&rule("error_rate", 0.5, 30)).is_err());
        assert!(validate_rule(&rule("error_rate", 0.5, 86_401)).is_err());
    }

    #[test]
    fn rules_reject_unknown_signal() {
        assert!(validate_rule(&rule("not_a_real_signal", 0.5, 60)).is_err());
    }

    #[test]
    fn rules_accept_every_supported_signal() {
        for signal in SIGNALS {
            assert!(validate_rule(&rule(signal, 0.5, 60)).is_ok());
        }
    }

    #[test]
    fn metric_sql_uses_expected_tables() {
        assert!(metric_sql("error_rate", 60)
            .unwrap()
            .contains("request_logs"));
        assert!(metric_sql("provider_health_flaps", 60)
            .unwrap()
            .contains("provider_health_events"));
        assert!(metric_sql("not_a_real_signal", 60).is_err());
    }

    #[test]
    fn channels_accept_exact_http_endpoints() {
        assert!(
            validate_channel(&channel("https://hooks.example/rolter", None), &egress()).is_ok()
        );
    }

    #[test]
    fn channels_reject_empty_managed_secret() {
        assert!(validate_channel(
            &channel("https://hooks.example/rolter", Some(String::new())),
            &egress()
        )
        .is_err());
    }

    #[test]
    fn channels_reject_endpoints_with_userinfo() {
        assert!(validate_channel(
            &channel("https://user:pass@hooks.example/rolter", None),
            &egress()
        )
        .is_err());
    }

    #[test]
    fn channels_reject_non_http_schemes() {
        assert!(validate_channel(&channel("ftp://hooks.example/rolter", None), &egress()).is_err());
    }

    #[test]
    fn channels_go_through_the_egress_policy() {
        // cloud instance metadata is denied by default
        assert!(validate_channel(
            &channel("http://169.254.169.254/latest/meta-data/", None),
            &egress()
        )
        .is_err());
        // and an operator can deny private ranges too
        let strict = EgressPolicy {
            block_private: true,
            ..EgressPolicy::default()
        };
        assert!(validate_channel(&channel("http://10.0.0.5/hook", None), &strict).is_err());
        assert!(validate_channel(&channel("http://10.0.0.5/hook", None), &egress()).is_ok());
    }

    #[test]
    fn a_transition_is_decided_against_the_last_reported_state() {
        // nothing reported yet: a high reading fires, a low one is quiet
        assert_eq!(transition(None, "firing"), Some("firing"));
        assert_eq!(transition(None, "ok"), None);
        // after a resolve the next high reading fires again
        assert_eq!(transition(Some("resolved"), "firing"), Some("firing"));
        assert_eq!(transition(Some("resolved"), "ok"), None);
        // a firing alert is reported once, and resolved once
        assert_eq!(transition(Some("firing"), "firing"), None);
        assert_eq!(transition(Some("firing"), "ok"), Some("resolved"));
    }

    #[test]
    fn a_long_query_error_is_truncated_on_a_char_boundary() {
        let long = "é".repeat(MAX_ERROR_LEN);
        let cut = truncate(&long, MAX_ERROR_LEN + 1);
        assert!(cut.len() <= MAX_ERROR_LEN + 1 + '…'.len_utf8());
        assert!(cut.ends_with('…'));
        assert_eq!(truncate("short", MAX_ERROR_LEN), "short");
    }

    #[test]
    fn the_payload_carries_the_rule_the_reading_and_a_summary() {
        let rule = Rule {
            id: Uuid::nil(),
            name: "high error rate".into(),
            signal: "error_rate".into(),
            threshold: 0.05,
            window_secs: 300,
            channel_id: None,
            enabled: true,
            state: "firing".into(),
            last_value: Some(0.07),
            last_evaluated_at: None,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let body = payload(Uuid::nil(), &rule, "firing");
        assert_eq!(body["state"], "firing");
        assert_eq!(body["rule"]["name"], "high error rate");
        assert_eq!(body["signal"], "error_rate");
        assert_eq!(body["value"], 0.07);
        assert_eq!(body["threshold"], 0.05);
        assert_eq!(body["window_secs"], 300);
        assert_eq!(
            body["text"],
            "high error rate is firing: error_rate is 0.07, threshold 0.05, 300s window"
        );
        let resolved = payload(Uuid::nil(), &rule, "resolved");
        assert!(resolved["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("high error rate resolved")));
    }

    /// Evaluation, delivery and history against a real schema, with a stub
    /// that answers as ClickHouse on `/` and as the webhook receiver on
    /// `/hook`.
    mod delivery {
        use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
        use std::sync::Arc;

        use axum::body::{to_bytes, Body};
        use axum::http::{HeaderMap, Request};
        use axum::response::{IntoResponse, Response};
        use rolter_store::postgres::test_schema::TestSchema;
        use serde_json::{json, Value};
        use tower::ServiceExt;

        use super::*;

        const SECRET: &str = "s3cret-bearer";

        /// One request the webhook receiver saw: its `Authorization` header and
        /// its JSON body.
        type Hook = (Option<String>, Value);

        #[derive(Clone)]
        struct Stub {
            /// The signal value the fake ClickHouse answers with, as `f64` bits.
            value: Arc<AtomicU64>,
            /// When non-zero, ClickHouse answers with this status instead.
            clickhouse_status: Arc<AtomicU16>,
            /// The status the webhook receiver answers with.
            hook_status: Arc<AtomicU16>,
            /// Every webhook request: its `Authorization` header and body.
            hooks: Arc<parking_lot::Mutex<Vec<Hook>>>,
            url: String,
        }

        impl Stub {
            async fn start() -> Self {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind a local port");
                let url = format!("http://{}", listener.local_addr().expect("a local address"));
                let stub = Self {
                    value: Arc::new(AtomicU64::new(0f64.to_bits())),
                    clickhouse_status: Arc::new(AtomicU16::new(0)),
                    hook_status: Arc::new(AtomicU16::new(200)),
                    hooks: Arc::default(),
                    url,
                };
                let app = Router::new()
                    .route("/", post(clickhouse))
                    .route("/hook", post(hook))
                    .with_state(stub.clone());
                tokio::spawn(async move { axum::serve(listener, app).await });
                stub
            }

            fn read(&self, value: f64) {
                self.value.store(value.to_bits(), Ordering::SeqCst);
            }

            fn hook_url(&self) -> String {
                format!("{}/hook", self.url)
            }

            fn hooks(&self) -> Vec<Hook> {
                self.hooks.lock().clone()
            }
        }

        async fn clickhouse(State(stub): State<Stub>) -> Response {
            let status = stub.clickhouse_status.load(Ordering::SeqCst);
            if status != 0 {
                return (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    "Code: 60. DB::Exception: Table default.request_logs does not exist",
                )
                    .into_response();
            }
            let value = f64::from_bits(stub.value.load(Ordering::SeqCst));
            Json(json!({ "data": [{ "value": value }] })).into_response()
        }

        async fn hook(
            State(stub): State<Stub>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Response {
            let authorization = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            stub.hooks.lock().push((authorization, body));
            let status = StatusCode::from_u16(stub.hook_status.load(Ordering::SeqCst))
                .unwrap_or(StatusCode::OK);
            // a receiver that echoes what it was sent, which the history must not keep
            (status, format!("rejected {SECRET}")).into_response()
        }

        /// A schema of its own per test; the guard drops it when the test ends.
        async fn scratch() -> Option<TestSchema> {
            let url = rolter_store::postgres::test_database::url()
                .await
                .or_else(|| {
                    eprintln!("skipping: ROLTER_TEST_DATABASE_URL not set");
                    None
                })?;
            Some(TestSchema::migrated(&url).await)
        }

        fn state_with(db: &TestSchema, stub: &Stub) -> ControlState {
            let mut state = crate::test_state(db.pool().clone(), None, None);
            state.clickhouse = Some(crate::analytics::ClickHouseClient::new(&stub.url));
            state
        }

        fn kek() -> Kek {
            Kek::from_secret("alerting-test-kek")
        }

        async fn add_channel(db: &TestSchema, endpoint: &str, enabled: bool, secret: bool) -> Uuid {
            let sealed = secret.then(|| kek().encrypt(SECRET).expect("seal the test secret"));
            sqlx::query_scalar(
                "insert into alert_channels (name, kind, endpoint, enabled, secret_ciphertext, secret_nonce) \
                 values ($1, 'webhook', $2, $3, $4, $5) returning id",
            )
            .bind(format!("channel-{}", Uuid::new_v4()))
            .bind(endpoint)
            .bind(enabled)
            .bind(sealed.as_ref().map(|(c, _)| c.clone()))
            .bind(sealed.as_ref().map(|(_, n)| n.clone()))
            .fetch_one(db.pool())
            .await
            .expect("insert a channel")
        }

        async fn add_rule(db: &TestSchema, channel_id: Option<Uuid>) -> Uuid {
            sqlx::query_scalar(
                "insert into alert_rules (name, signal, threshold, window_secs, channel_id, enabled) \
                 values ($1, 'error_rate', 0.5, 300, $2, true) returning id",
            )
            .bind(format!("rule-{}", Uuid::new_v4()))
            .bind(channel_id)
            .fetch_one(db.pool())
            .await
            .expect("insert a rule")
        }

        async fn history(db: &TestSchema, rule_id: Uuid) -> Vec<(String, String, Option<String>)> {
            sqlx::query_as(
                "select state, delivery_status, detail from alert_notification_history \
                 where rule_id = $1 order by sent_at",
            )
            .bind(rule_id)
            .fetch_all(db.pool())
            .await
            .expect("read the history")
        }

        async fn rule_row(db: &TestSchema, id: Uuid) -> Rule {
            sqlx::query_as(&format!(
                "select {} from alert_rules where id=$1",
                rule_columns()
            ))
            .bind(id)
            .fetch_one(db.pool())
            .await
            .expect("read the rule")
        }

        async fn run(state: &ControlState, id: Uuid, kek: Option<&Kek>) -> Evaluation {
            evaluate_one(state, id, Lock::Wait, kek)
                .await
                .expect("the evaluation succeeds")
                .expect("the rule exists")
        }

        #[tokio::test]
        async fn a_transition_is_posted_with_the_secret_and_recorded_delivered() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;
            let kek = kek();

            stub.read(0.9);
            let first = run(&state, id, Some(&kek)).await;
            assert!(first.notified);
            assert_eq!(first.rule.state, "firing");
            let notification = first.notification.expect("a history row");
            assert_eq!(notification.delivery_status, "delivered");
            assert_eq!(notification.detail.as_deref(), Some("HTTP 200"));
            assert_eq!(notification.channel_id, Some(channel));

            let hooks = stub.hooks();
            assert_eq!(hooks.len(), 1);
            let (authorization, body) = &hooks[0];
            assert_eq!(
                authorization.as_deref(),
                Some(format!("Bearer {SECRET}").as_str())
            );
            assert_eq!(body["id"], json!(notification.id));
            assert_eq!(body["state"], "firing");
            assert_eq!(body["rule"]["id"], json!(id));
            assert_eq!(body["signal"], "error_rate");
            assert_eq!(body["value"], 0.9);
            assert_eq!(body["threshold"], 0.5);
            assert_eq!(body["window_secs"], 300);

            // still firing: nothing new is sent or recorded
            let again = run(&state, id, Some(&kek)).await;
            assert!(!again.notified);
            assert!(again.notification.is_none());
            assert_eq!(stub.hooks().len(), 1);

            stub.read(0.1);
            let resolved = run(&state, id, Some(&kek)).await;
            assert!(resolved.notified);
            assert_eq!(resolved.rule.state, "ok");
            assert_eq!(stub.hooks()[1].1["state"], "resolved");
            assert_eq!(
                history(&db, id).await,
                vec![
                    ("firing".into(), "delivered".into(), Some("HTTP 200".into())),
                    (
                        "resolved".into(),
                        "delivered".into(),
                        Some("HTTP 200".into())
                    ),
                ]
            );
        }

        #[tokio::test]
        async fn a_rejected_delivery_is_recorded_failed_with_the_status_only() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;
            stub.hook_status.store(500, Ordering::SeqCst);

            stub.read(0.9);
            let evaluation = run(&state, id, Some(&kek())).await;
            assert!(!evaluation.notified);
            // the evaluation itself succeeded, so the rule's state still moves
            assert_eq!(evaluation.rule.state, "firing");
            assert_eq!(evaluation.rule.last_error, None);
            assert_eq!(
                history(&db, id).await,
                vec![("firing".into(), "failed".into(), Some("HTTP 500".into()))]
            );
            // a failed delivery is not retried while the state holds
            run(&state, id, Some(&kek())).await;
            assert_eq!(stub.hooks().len(), 1);
            assert_eq!(history(&db, id).await.len(), 1);
        }

        #[tokio::test]
        async fn an_unreachable_endpoint_is_recorded_failed() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            // a port that was listening a moment ago and no longer is
            let closed = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
                listener.local_addr().expect("an address")
            };
            let channel = add_channel(&db, &format!("http://{closed}/hook"), true, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            run(&state, id, None).await;
            assert_eq!(
                history(&db, id).await,
                vec![(
                    "firing".into(),
                    "failed".into(),
                    Some("could not connect to the endpoint".into())
                )]
            );
        }

        #[tokio::test]
        async fn an_endpoint_the_egress_policy_denies_is_never_contacted() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            // saved while loopback was allowed, delivered after it was denied
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;
            state.egress = Arc::new(EgressPolicy {
                block_loopback: true,
                ..EgressPolicy::default()
            });

            stub.read(0.9);
            run(&state, id, None).await;
            assert!(stub.hooks().is_empty());
            assert_eq!(
                history(&db, id).await,
                vec![(
                    "firing".into(),
                    "failed".into(),
                    Some("endpoint denied by the egress policy".into())
                )]
            );
        }

        #[tokio::test]
        async fn a_secret_that_cannot_be_unsealed_is_not_sent_without_it() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            // no key at all, then the wrong one
            run(&state, id, None).await;
            stub.read(0.1);
            run(&state, id, Some(&Kek::from_secret("not-the-key"))).await;
            assert!(stub.hooks().is_empty());
            let rows = history(&db, id).await;
            assert_eq!(rows.len(), 2);
            for (_, status, detail) in rows {
                assert_eq!(status, "failed");
                assert!(detail.is_some_and(|d| d.contains(KEK_ENV)));
            }
        }

        #[tokio::test]
        async fn transitions_without_a_live_channel_are_recorded_skipped() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let no_channel = add_rule(&db, None).await;
            let disabled = add_channel(&db, &stub.hook_url(), false, false).await;
            let to_disabled = add_rule(&db, Some(disabled)).await;

            stub.read(0.9);
            run(&state, no_channel, None).await;
            run(&state, to_disabled, None).await;
            assert!(stub.hooks().is_empty());
            assert_eq!(
                history(&db, no_channel).await,
                vec![(
                    "firing".into(),
                    "skipped".into(),
                    Some("no channel configured".into())
                )]
            );
            assert_eq!(
                history(&db, to_disabled).await,
                vec![(
                    "firing".into(),
                    "skipped".into(),
                    Some("channel disabled".into())
                )]
            );
        }

        #[tokio::test]
        async fn a_failed_evaluation_sets_error_and_the_alert_survives_it() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            run(&state, id, None).await;
            assert_eq!(stub.hooks().len(), 1);

            // clickhouse answers with an error: the rule says so, and keeps
            // the last value it read
            stub.clickhouse_status.store(500, Ordering::SeqCst);
            let error = evaluate_one(&state, id, Lock::Wait, None).await;
            assert!(error.is_err());
            let errored = rule_row(&db, id).await;
            assert_eq!(errored.state, "error");
            assert_eq!(errored.last_value, Some(0.9));
            let last_error = errored.last_error.expect("last_error is set");
            assert!(last_error.contains("500"), "{last_error}");
            assert!(last_error.contains("DB::Exception"), "{last_error}");
            assert!(!last_error.contains(&stub.url), "{last_error}");

            // no clickhouse configured at all
            state.clickhouse = None;
            assert!(evaluate_one(&state, id, Lock::Wait, None).await.is_err());
            let errored = rule_row(&db, id).await;
            assert_eq!(
                errored.last_error.as_deref(),
                Some("alert evaluation requires CLICKHOUSE_URL")
            );

            // back, and still high: the alert was already reported
            state.clickhouse = Some(crate::analytics::ClickHouseClient::new(&stub.url));
            stub.clickhouse_status.store(0, Ordering::SeqCst);
            let recovered = run(&state, id, None).await;
            assert_eq!(recovered.rule.state, "firing");
            assert_eq!(recovered.rule.last_error, None);
            assert!(recovered.notification.is_none());
            assert_eq!(stub.hooks().len(), 1);

            // an error between firing and a low reading still resolves
            stub.clickhouse_status.store(500, Ordering::SeqCst);
            let _ = evaluate_one(&state, id, Lock::Wait, None).await;
            stub.clickhouse_status.store(0, Ordering::SeqCst);
            stub.read(0.1);
            let resolved = run(&state, id, None).await;
            assert!(resolved.notified);
            assert_eq!(stub.hooks()[1].1["state"], "resolved");
            // error passes write no history of their own
            assert_eq!(history(&db, id).await.len(), 2);
        }

        #[tokio::test]
        async fn a_rule_another_pass_holds_is_left_to_that_pass() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let id = add_rule(&db, None).await;
            stub.read(0.9);

            // a second replica mid-evaluation holds the row
            let mut other = db.pool().begin().await.expect("begin");
            sqlx::query("select id from alert_rules where id=$1 for update")
                .bind(id)
                .execute(&mut *other)
                .await
                .expect("lock the rule");
            let skipped = evaluate_one(&state, id, Lock::SkipLocked, None)
                .await
                .expect("a held rule is not an error");
            assert!(skipped.is_none());
            other.rollback().await.expect("release the rule");
            assert!(history(&db, id).await.is_empty());

            let evaluated = evaluate_one(&state, id, Lock::SkipLocked, None)
                .await
                .expect("the evaluation succeeds");
            assert!(evaluated.is_some());
            assert_eq!(history(&db, id).await.len(), 1);

            // the scheduled pass leaves a disabled rule alone
            sqlx::query("update alert_rules set enabled=false where id=$1")
                .bind(id)
                .execute(db.pool())
                .await
                .expect("disable the rule");
            assert!(evaluate_one(&state, id, Lock::SkipLocked, None)
                .await
                .expect("a disabled rule is not an error")
                .is_none());
        }

        #[tokio::test]
        async fn evaluate_now_reports_a_failure_and_records_it_on_the_rule() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            state.clickhouse = None;
            let id = add_rule(&db, None).await;
            let app = router().with_state(state);

            let response = app
                .oneshot(
                    Request::post(format!("/api/v1/alert-rules/{id}/evaluate"))
                        .body(Body::empty())
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body = to_bytes(response.into_body(), 1 << 16)
                .await
                .expect("a body");
            let body: Value = serde_json::from_slice(&body).expect("json");
            assert!(body["error"]["message"]
                .as_str()
                .is_some_and(|m| m.contains("CLICKHOUSE_URL")));
            assert_eq!(rule_row(&db, id).await.state, "error");
        }
    }
}
