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
//!   endpoint gets, and redirects are not followed. It is stored as the HTTP
//!   client parses it, so the address checked is the address dialled.
//! - **The credential is write-only.** A channel secret is sealed with the
//!   deployment KEK, unsealed only to build the `Authorization` header, and
//!   never returned by the API or placed in an audit entry or history row.
//!   It belongs to the endpoint's origin: repointing a channel at another
//!   origin drops it unless the same request supplies a new one.
//! - **A receiver's response body is never stored.** The history keeps the HTTP
//!   status or the class of transport failure, since an error body can echo the
//!   credential it just rejected.

use std::sync::OnceLock;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::{EgressPolicy, Error};
use rolter_store::postgres::crypto::{Kek, KEK_ENV};
use rolter_store::postgres::repo::AuditLogRepo;

use crate::crud::{pool, ApiError, ApiResult, SafeJson};
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
/// How many of a rule's newest history rows the retry decision reads: enough
/// to count the failed attempts behind the longest [`retry_delay`].
const HISTORY_WINDOW: i64 = 16;

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

/// Check a channel body and return its endpoint as the HTTP client parses it,
/// which is the form it is stored in.
///
/// Storing the parsed form rather than what was typed means the delivery-time
/// egress check and the request both read one spelling of the host.
fn validate_channel(input: &ChannelInput) -> ApiResult<reqwest::Url> {
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
    if let Some(secret) = &input.managed_secret {
        if secret.trim().is_empty() {
            return Err(invalid("managed_secret must not be empty"));
        }
        // checked here rather than found at the first delivery, where a
        // trailing newline from a file would fail every alert with no hint
        if bearer(secret).is_none() {
            return Err(invalid(
                "managed_secret must be a valid HTTP header value, without control characters or line breaks",
            ));
        }
    }
    Ok(endpoint)
}

/// The same guard a connector endpoint gets: an operator-supplied URL the
/// control plane will POST to is an SSRF surface.
fn check_egress(egress: &EgressPolicy, endpoint: &reqwest::Url) -> ApiResult<()> {
    egress
        .check_url(endpoint.as_str(), "channel endpoint")
        .map_err(invalid)
}

/// The `Authorization` value a channel secret is sent as, or `None` when the
/// secret cannot be one.
fn bearer(secret: &str) -> Option<HeaderValue> {
    let mut value = HeaderValue::from_str(&format!("Bearer {secret}")).ok()?;
    value.set_sensitive(true);
    Some(value)
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
        .map_err(|_| ApiError::Curated("failed to encrypt channel credential".into()))
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
    SafeJson(input): SafeJson<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    authorize_superadmin(&principal, superadmin_cap!("alert_channel", Create))?;
    let endpoint = validate_channel(&input)?;
    check_egress(&state.egress, &endpoint)?;
    let secret = input.managed_secret.as_deref().map(seal).transpose()?;
    let channel: Channel = sqlx::query_as(&format!(
        "insert into alert_channels (name, kind, endpoint, enabled, secret_ciphertext, secret_nonce) values ($1, 'webhook', $2, $3, $4, $5) returning {}", channel_columns()))
        .bind(input.name.trim()).bind(endpoint.as_str()).bind(input.enabled)
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
    SafeJson(input): SafeJson<ChannelInput>,
) -> ApiResult<Json<Channel>> {
    authorize_superadmin(&principal, superadmin_cap!("alert_channel", Update))?;
    let endpoint = validate_channel(&input)?;
    let secret = input.managed_secret.as_deref().map(seal).transpose()?;
    let mut tx = pool(&state).begin().await.map_err(store_error)?;
    // no key update rather than for update: an evaluation delivering to this
    // channel holds it for key share, and an edit need not wait for that
    let (stored, had_secret): (String, bool) = sqlx::query_as(
        "select endpoint, secret_ciphertext is not null from alert_channels where id=$1 for no key update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(store_error)?
    .ok_or_else(|| Error::NotFound(format!("alert channel {id}")))?;
    let stored = reqwest::Url::parse(&stored).ok();
    let endpoint_changed = stored.as_ref() != Some(&endpoint);
    // a disabled channel sends nothing, so switching off one whose unchanged
    // endpoint a since-tightened policy denies must not be refused by it
    if input.enabled || endpoint_changed {
        check_egress(&state.egress, &endpoint)?;
    }
    // a secret was given for one receiver; repointed at another origin, the
    // channel would hand it to whoever answers there, so it is dropped unless
    // this request brings the new receiver's own
    let origin_changed = stored.is_none_or(|stored| stored.origin() != endpoint.origin());
    let clear_secret = origin_changed && secret.is_none();
    let channel: Channel = sqlx::query_as(&format!(
        "update alert_channels set name=$2, endpoint=$3, enabled=$4, \
         secret_ciphertext = case when $7 then null else coalesce($5, secret_ciphertext) end, \
         secret_nonce = case when $7 then null else coalesce($6, secret_nonce) end, \
         updated_at=now() where id=$1 returning {}",
        channel_columns()
    ))
    .bind(id)
    .bind(input.name.trim())
    .bind(endpoint.as_str())
    .bind(input.enabled)
    .bind(secret.as_ref().map(|(c, _)| c.as_slice()))
    .bind(secret.as_ref().map(|(_, n)| n.as_slice()))
    .bind(clear_secret)
    .fetch_one(&mut *tx)
    .await
    .map_err(store_error)?;
    tx.commit().await.map_err(store_error)?;
    // whether the endpoint moved and what became of the secret, never the
    // endpoint itself: its query string can carry a token
    audit(
        &state,
        &principal,
        "alert.channel.update",
        id,
        serde_json::json!({
            "name": channel.name,
            "enabled": channel.enabled,
            "endpoint_changed": endpoint_changed,
            "secret_replaced": secret.is_some(),
            "secret_cleared": clear_secret && had_secret,
        }),
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
    SafeJson(input): SafeJson<RuleInput>,
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
    SafeJson(input): SafeJson<RuleInput>,
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
    /// evaluates it whether or not it is enabled, and retries a state that was
    /// not delivered without waiting out [`retry_delay`].
    Wait,
    /// Leave a rule another pass holds to that pass, and skip one that is no
    /// longer enabled. The scheduled evaluator runs this way.
    SkipLocked,
}

/// A rule as read before its signal query, with the database clock at that
/// moment.
#[derive(sqlx::FromRow)]
struct Unlocked {
    #[sqlx(flatten)]
    rule: Rule,
    read_at: DateTime<Utc>,
}

/// One history row, as the transition decision reads it.
#[derive(sqlx::FromRow)]
struct Reported {
    state: String,
    delivery_status: String,
    sent_at: DateTime<Utc>,
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
/// from reporting the same change twice. A reading older than the one the rule
/// already records is dropped, so a slow query cannot undo a newer result.
async fn evaluate_one(
    state: &ControlState,
    id: Uuid,
    lock: Lock,
    kek: Option<&Kek>,
) -> ApiResult<Option<Evaluation>> {
    let filter = if lock == Lock::SkipLocked {
        " and enabled"
    } else {
        ""
    };
    let Some(Unlocked {
        rule: unlocked,
        read_at,
    }) = sqlx::query_as(&format!(
        "select {}, clock_timestamp() as read_at from alert_rules where id=$1{filter}",
        rule_columns()
    ))
    .bind(id)
    .fetch_optional(pool(state))
    .await
    .map_err(store_error)?
    else {
        return Ok(None);
    };
    let reading = read_signal(state, &unlocked.signal, unlocked.window_secs).await;

    let mut tx = pool(state).begin().await.map_err(store_error)?;
    // the channel is locked before the rule: deleting a channel locks it and
    // then, through on delete set null, every rule naming it, so taking the
    // two in that same order makes a delete wait for an in-flight delivery
    // instead of deadlocking with it. key share still lets the channel be
    // edited meanwhile
    let channel: Option<ChannelTarget> = match unlocked.channel_id {
        Some(channel_id) => sqlx::query_as(
            "select id, endpoint, enabled, secret_ciphertext, secret_nonce from alert_channels where id=$1 for key share",
        )
        .bind(channel_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(store_error)?,
        None => None,
    };
    let locking = match lock {
        Lock::Wait => "for update",
        Lock::SkipLocked => "for update skip locked",
    };
    let Some(rule): Option<Rule> = sqlx::query_as(&format!(
        "select {} from alert_rules where id=$1{filter} {locking}",
        rule_columns()
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(store_error)?
    else {
        return Ok(None);
    };
    // an edit landed between the read and the lock, so the reading measures a
    // query the rule no longer asks for, or the channel locked above is no
    // longer the rule's; the next pass starts from the edited rule. a channel
    // deleted meanwhile has already cleared the rule's channel_id, which
    // matches the missing channel and is evaluated as no channel
    let edited = rule.signal != unlocked.signal
        || rule.window_secs != unlocked.window_secs
        || rule.channel_id != channel.as_ref().map(|c| c.id);
    // another evaluation started after this one and has already recorded its
    // reading, which this older one must not overwrite
    let superseded = rule.last_evaluated_at.is_some_and(|at| at > read_at);
    if edited || superseded {
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
            // `read_signal` words the reason for the rule card, and never with
            // the ClickHouse endpoint in it, so the caller may read it too
            return Err(ApiError::Curated(reason));
        }
    };
    let next_state = if value >= rule.threshold {
        "firing"
    } else {
        "ok"
    };
    let history: Vec<Reported> = sqlx::query_as(
        "select state, delivery_status, sent_at from alert_notification_history \
         where rule_id=$1 order by sent_at desc limit $2",
    )
    .bind(id)
    .bind(HISTORY_WINDOW)
    .fetch_all(&mut *tx)
    .await
    .map_err(store_error)?;
    // the time the reading was taken rather than now(), so a later reading
    // can tell this one is older
    let rule: Rule = sqlx::query_as(&format!(
        "update alert_rules set state=$2, last_value=$3, last_evaluated_at=$4, last_error=null, updated_at=now() where id=$1 returning {}",
        rule_columns()
    ))
    .bind(id)
    .bind(next_state)
    .bind(value)
    .bind(read_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(store_error)?;
    let redeliver = match (&channel, lock) {
        (Some(channel), Lock::Wait) if channel.enabled => Redeliver::Now,
        (Some(channel), Lock::SkipLocked) if channel.enabled => Redeliver::After(read_at),
        _ => Redeliver::Never,
    };
    let notification = match transition(&history, next_state, redeliver) {
        Some(change) => {
            Some(report(&state.egress, &mut tx, &rule, channel.as_ref(), change, kek).await?)
        }
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

/// Whether a state that holds is sent again when no attempt at it got
/// through.
#[derive(Clone, Copy)]
enum Redeliver {
    /// The rule has no enabled channel, so there is nothing to retry with.
    Never,
    /// Retry now. **Evaluate now** is an operator asking for it.
    Now,
    /// Retry once [`retry_delay`] has passed since the last attempt, measured
    /// at this moment. The scheduled pass runs this way.
    After(DateTime<Utc>),
}

/// What a reading reports, given the rule's newest history rows, newest first:
/// the state to record and deliver, or `None`.
///
/// Decided against the history rather than the rule's `state` column, which
/// also holds `unknown` and `error`. A rule whose evaluation failed between two
/// firing readings is still the same alert and is not reported again, and one
/// that recovered while its evaluation was failing still owes a `resolved`. A
/// rule with no history that reads high reports `firing`, so a rule enabled
/// while its condition already holds says so on its first pass.
///
/// A state that holds is sent again only while no attempt at it was
/// `delivered`: a failed delivery, or one skipped because the channel was
/// disabled or missing, is retried once the rule has an enabled channel. The
/// scheduled pass spaces the retries out (see [`retry_delay`]), so a dead
/// endpoint costs a handful of history rows an hour rather than one a minute.
fn transition(
    history: &[Reported],
    next_state: &str,
    redeliver: Redeliver,
) -> Option<&'static str> {
    let holding = match (history.first().map(|r| r.state.as_str()), next_state) {
        (Some("firing"), "ok") => return Some("resolved"),
        (Some("firing"), _) => "firing",
        (_, "firing") => return Some("firing"),
        (Some("resolved"), _) => "resolved",
        _ => return None,
    };
    let mut unreported = history
        .iter()
        .take_while(|r| r.state == holding && r.delivery_status != "delivered");
    let last = unreported.next()?;
    match redeliver {
        Redeliver::Never => None,
        Redeliver::Now => Some(holding),
        Redeliver::After(now) => {
            let failures = usize::from(last.delivery_status == "failed")
                + unreported.filter(|r| r.delivery_status == "failed").count();
            (now - last.sent_at >= retry_delay(failures)).then_some(holding)
        }
    }
}

/// How long after the last attempt a state that was not delivered is retried,
/// given how many attempts at it failed.
///
/// The pass after the first failure, then two, four, eight, sixteen and
/// thirty-two passes on, then hourly. Each delay is a minute under that
/// spacing because an attempt is recorded up to 40 seconds into its pass, so
/// the pass that many minutes later reads a little under it.
fn retry_delay(failures: usize) -> chrono::Duration {
    let passes = 1_i64 << failures.saturating_sub(1).min(6);
    chrono::Duration::minutes((passes - 1).min(59))
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
    channel: Option<&ChannelTarget>,
    change: &'static str,
    kek: Option<&Kek>,
) -> ApiResult<Notification> {
    let id = Uuid::new_v4();
    let outcome = match channel {
        None => Outcome::skipped("no channel configured"),
        Some(channel) if !channel.enabled => Outcome::skipped("channel disabled"),
        Some(channel) => deliver(egress, channel, &payload(id, rule, change), kek).await,
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
    .bind(channel.map(|c| c.id))
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
    // tightened since; a stored row is not a standing permission to egress.
    // the log gets the reason and the channel, never the endpoint, whose query
    // string can carry a token
    if let Some(reason) = egress.url_deny_reason(&channel.endpoint) {
        tracing::warn!(channel = %channel.id, reason, "alert delivery refused by the egress policy");
        return Outcome::failed("endpoint denied by the egress policy");
    }
    let authorization = match (&channel.secret_ciphertext, &channel.secret_nonce) {
        (Some(ciphertext), Some(nonce)) => {
            match kek.and_then(|kek| kek.decrypt(ciphertext, nonce).ok()) {
                Some(secret) => match bearer(&secret) {
                    Some(value) => Some(value),
                    // refused at save time; a secret stored before that check
                    // existed would otherwise fail as a bare "delivery failed"
                    None => return Outcome::failed("channel secret is not a valid header value"),
                },
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

    /// Validate `input` and run the save-time egress check, as a create does.
    fn save(input: &ChannelInput, egress: &EgressPolicy) -> ApiResult<reqwest::Url> {
        let endpoint = validate_channel(input)?;
        check_egress(egress, &endpoint)?;
        Ok(endpoint)
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
        assert!(save(&channel("https://hooks.example/rolter", None), &egress()).is_ok());
    }

    #[test]
    fn channels_reject_empty_managed_secret() {
        assert!(save(
            &channel("https://hooks.example/rolter", Some(String::new())),
            &egress()
        )
        .is_err());
    }

    #[test]
    fn channels_reject_endpoints_with_userinfo() {
        assert!(save(
            &channel("https://user:pass@hooks.example/rolter", None),
            &egress()
        )
        .is_err());
    }

    #[test]
    fn channels_reject_non_http_schemes() {
        assert!(save(&channel("ftp://hooks.example/rolter", None), &egress()).is_err());
    }

    #[test]
    fn channels_go_through_the_egress_policy() {
        // cloud instance metadata is denied by default, in every spelling the
        // http client would dial it by (#1953)
        for endpoint in [
            "http://169.254.169.254/latest/meta-data/",
            "http://2852039166/latest/meta-data/",
            "http://0xa9fea9fe/",
            "http://169.254.43518/",
            "http://0251.0376.0251.0376/",
            "http://169.254.169.254./",
            "http://[::ffff:169.254.169.254]/",
        ] {
            assert!(
                save(&channel(endpoint, None), &egress()).is_err(),
                "{endpoint} was saved"
            );
        }
        // and loopback in every spelling once the operator denies it
        let no_loopback = EgressPolicy {
            block_loopback: true,
            ..EgressPolicy::default()
        };
        for endpoint in ["http://2130706433/", "http://0.0.0.0:4001/", "http://[::]/"] {
            assert!(
                save(&channel(endpoint, None), &no_loopback).is_err(),
                "{endpoint} was saved"
            );
        }
        // and an operator can deny private ranges too
        let strict = EgressPolicy {
            block_private: true,
            ..EgressPolicy::default()
        };
        assert!(save(&channel("http://10.0.0.5/hook", None), &strict).is_err());
        assert!(save(&channel("http://10.0.0.5/hook", None), &egress()).is_ok());
    }

    #[test]
    fn channels_store_the_endpoint_as_the_client_parses_it() {
        let endpoint = save(&channel("HTTPS://Alerts.Example.COM", None), &egress()).unwrap();
        assert_eq!(endpoint.as_str(), "https://alerts.example.com/");
    }

    #[test]
    fn channels_reject_a_secret_that_is_not_a_header_value() {
        // the case index, not the value, in the message: an assertion that
        // prints a secret is the habit this module exists to avoid
        for (case, value) in ["tok\n", "tok\r\nX-Injected: 1", "a\u{7f}b"]
            .into_iter()
            .enumerate()
        {
            assert!(
                save(
                    &channel("https://hooks.example/rolter", Some(value.into())),
                    &egress()
                )
                .is_err(),
                "case {case} was accepted"
            );
        }
        assert!(save(
            &channel("https://hooks.example/rolter", Some("tok-123".into())),
            &egress()
        )
        .is_ok());
    }

    /// A fixed "now", so a row written zero minutes ago is not a hair after it.
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn row(state: &str, delivery_status: &str, minutes_ago: i64) -> Reported {
        Reported {
            state: state.into(),
            delivery_status: delivery_status.into(),
            sent_at: now() - chrono::Duration::minutes(minutes_ago),
        }
    }

    #[test]
    fn a_transition_is_decided_against_the_last_reported_state() {
        let now = Redeliver::After(now());
        // nothing reported yet: a high reading fires, a low one is quiet
        assert_eq!(transition(&[], "firing", now), Some("firing"));
        assert_eq!(transition(&[], "ok", now), None);
        // after a resolve the next high reading fires again
        let resolved = [row("resolved", "delivered", 5)];
        assert_eq!(transition(&resolved, "firing", now), Some("firing"));
        assert_eq!(transition(&resolved, "ok", now), None);
        // a firing alert is reported once, and resolved once
        let firing = [row("firing", "delivered", 5)];
        assert_eq!(transition(&firing, "firing", now), None);
        assert_eq!(transition(&firing, "ok", now), Some("resolved"));
        // a firing that never got through still resolves when the reading drops
        let failed = [row("firing", "failed", 0)];
        assert_eq!(
            transition(&failed, "ok", Redeliver::Never),
            Some("resolved")
        );
    }

    #[test]
    fn a_state_that_was_not_delivered_is_retried_with_a_growing_delay() {
        let at = Redeliver::After(now());
        // the first failure is retried on the next pass, however soon
        assert_eq!(
            transition(&[row("firing", "failed", 0)], "firing", at),
            Some("firing")
        );
        // the second waits a pass
        let twice = [row("firing", "failed", 0), row("firing", "failed", 1)];
        assert_eq!(transition(&twice, "firing", at), None);
        let twice = [row("firing", "failed", 1), row("firing", "failed", 2)];
        assert_eq!(transition(&twice, "firing", at), Some("firing"));
        // a resolve that failed is retried the same way
        let resolve = [row("resolved", "failed", 0), row("firing", "delivered", 9)];
        assert_eq!(transition(&resolve, "ok", at), Some("resolved"));
        // one delivery ends the retries, and so does having no channel to use
        let delivered = [row("firing", "delivered", 0), row("firing", "failed", 1)];
        assert_eq!(transition(&delivered, "firing", at), None);
        assert_eq!(
            transition(&[row("firing", "failed", 90)], "firing", Redeliver::Never),
            None
        );
        // a transition skipped while the channel was off goes out once it is on
        assert_eq!(
            transition(&[row("firing", "skipped", 0)], "firing", at),
            Some("firing")
        );
        // evaluate now does not wait out the delay
        let many: Vec<Reported> = (0..10).map(|m| row("firing", "failed", m)).collect();
        assert_eq!(transition(&many, "firing", at), None);
        assert_eq!(transition(&many, "firing", Redeliver::Now), Some("firing"));
    }

    #[test]
    fn the_retry_delay_doubles_up_to_an_hour() {
        let minutes: Vec<i64> = (0..10).map(|f| retry_delay(f).num_minutes()).collect();
        assert_eq!(minutes, [0, 0, 1, 3, 7, 15, 31, 59, 59, 59]);
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
        use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
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
            /// When set, the receiver holds each request until [`Self::release`]
            /// is notified, so a test can act while a delivery is in flight.
            holding: Arc<AtomicBool>,
            release: Arc<tokio::sync::Notify>,
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
                    holding: Arc::default(),
                    release: Arc::default(),
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
            if stub.holding.load(Ordering::SeqCst) {
                stub.release.notified().await;
            }
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

        async fn scheduled(
            state: &ControlState,
            id: Uuid,
            kek: Option<&Kek>,
        ) -> Option<Evaluation> {
            evaluate_one(state, id, Lock::SkipLocked, kek)
                .await
                .expect("the evaluation succeeds")
        }

        /// Move a rule's history back in time, as if the passes that wrote it
        /// ran that long ago.
        async fn age_history(db: &TestSchema, rule_id: Uuid, minutes: i32) {
            sqlx::query(
                "update alert_notification_history \
                 set sent_at = sent_at - make_interval(mins => $2) where rule_id = $1",
            )
            .bind(rule_id)
            .bind(minutes)
            .execute(db.pool())
            .await
            .expect("age the history");
        }

        async fn put(state: &ControlState, uri: String, body: Value) -> (StatusCode, Value) {
            let response = router()
                .with_state(state.clone())
                .oneshot(
                    Request::put(uri)
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body.to_string()))
                        .expect("a valid request"),
                )
                .await
                .expect("the router answers");
            let status = response.status();
            let body = to_bytes(response.into_body(), 1 << 16)
                .await
                .expect("a body");
            (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
        }

        #[tokio::test]
        async fn a_rejected_delivery_is_recorded_failed_and_retried_until_it_gets_through() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let id = add_rule(&db, Some(channel)).await;
            let kek = kek();
            stub.hook_status.store(500, Ordering::SeqCst);

            stub.read(0.9);
            let evaluation = run(&state, id, Some(&kek)).await;
            assert!(!evaluation.notified);
            // the evaluation itself succeeded, so the rule's state still moves
            assert_eq!(evaluation.rule.state, "firing");
            assert_eq!(evaluation.rule.last_error, None);
            assert_eq!(
                history(&db, id).await,
                vec![("firing".into(), "failed".into(), Some("HTTP 500".into()))]
            );

            // the next pass tries again, whatever the time
            scheduled(&state, id, Some(&kek)).await;
            assert_eq!(stub.hooks().len(), 2);
            // after a second failure the pass a moment later waits
            scheduled(&state, id, Some(&kek)).await;
            assert_eq!(stub.hooks().len(), 2);
            assert_eq!(history(&db, id).await.len(), 2);

            // two minutes on, the receiver is back
            age_history(&db, id, 2).await;
            stub.hook_status.store(200, Ordering::SeqCst);
            let retried = scheduled(&state, id, Some(&kek)).await.expect("evaluated");
            assert!(retried.notified);
            let hooks = stub.hooks();
            assert_eq!(hooks.len(), 3);
            assert_eq!(hooks[2].1["state"], "firing");
            assert_eq!(
                hooks[2].0.as_deref(),
                Some(format!("Bearer {SECRET}").as_str())
            );

            // delivered once: neither the schedule nor evaluate now sends it again
            age_history(&db, id, 120).await;
            scheduled(&state, id, Some(&kek)).await;
            run(&state, id, Some(&kek)).await;
            assert_eq!(stub.hooks().len(), 3);
            let rows: Vec<String> = history(&db, id)
                .await
                .into_iter()
                .map(|(_, status, _)| status)
                .collect();
            assert_eq!(rows, ["failed", "failed", "delivered"]);
        }

        #[tokio::test]
        async fn a_transition_skipped_while_the_channel_was_off_goes_out_once_it_is_on() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), false, false).await;
            let id = add_rule(&db, Some(channel)).await;

            stub.read(0.9);
            scheduled(&state, id, None).await;
            // still off: nothing to retry with, so nothing new is written
            scheduled(&state, id, None).await;
            assert_eq!(history(&db, id).await.len(), 1);

            sqlx::query("update alert_channels set enabled=true where id=$1")
                .bind(channel)
                .execute(db.pool())
                .await
                .expect("enable the channel");
            let evaluation = scheduled(&state, id, None).await.expect("evaluated");
            assert!(evaluation.notified);
            assert_eq!(stub.hooks().len(), 1);
            assert_eq!(
                history(&db, id).await,
                vec![
                    (
                        "firing".into(),
                        "skipped".into(),
                        Some("channel disabled".into())
                    ),
                    ("firing".into(), "delivered".into(), Some("HTTP 200".into())),
                ]
            );
        }

        #[tokio::test]
        async fn an_older_reading_does_not_overwrite_a_newer_one() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;
            // another replica took a reading after this one started and has
            // already recorded it
            sqlx::query(
                "update alert_rules set state='ok', last_value=0.1, \
                 last_evaluated_at = clock_timestamp() + interval '1 hour' where id=$1",
            )
            .bind(id)
            .execute(db.pool())
            .await
            .expect("record a newer reading");

            stub.read(0.9);
            let evaluation = run(&state, id, None).await;
            assert!(evaluation.notification.is_none());
            assert_eq!(evaluation.rule.state, "ok");
            assert_eq!(evaluation.rule.last_value, Some(0.1));
            assert!(stub.hooks().is_empty());
            assert!(history(&db, id).await.is_empty());
        }

        #[tokio::test]
        async fn deleting_a_channel_waits_for_a_delivery_in_flight() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            let id = add_rule(&db, Some(channel)).await;
            stub.read(0.9);
            stub.holding.store(true, Ordering::SeqCst);

            let evaluation = tokio::spawn({
                let state = state.clone();
                async move { evaluate_one(&state, id, Lock::Wait, None).await }
            });
            // the receiver has the request and is holding it
            tokio::time::timeout(Duration::from_secs(10), async {
                while stub.hooks().is_empty() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the delivery reaches the receiver");

            let delete = tokio::spawn({
                let pool = db.pool().clone();
                async move {
                    sqlx::query("delete from alert_channels where id=$1 /* in-flight delete */")
                        .bind(channel)
                        .execute(&pool)
                        .await
                }
            });
            // the delete is queued behind the delivery's lock before the
            // receiver answers, which is the order that used to deadlock
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let waiting: i64 = sqlx::query_scalar(
                        "select count(*) from pg_stat_activity \
                         where wait_event_type = 'Lock' and query like '%in-flight delete%' \
                         and query not like '%pg_stat_activity%'",
                    )
                    .fetch_one(db.pool())
                    .await
                    .expect("read pg_stat_activity");
                    if waiting > 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the delete waits on a lock");
            stub.release.notify_one();

            let evaluation = evaluation
                .await
                .expect("the evaluation task")
                .expect("the evaluation is not a deadlock victim")
                .expect("the rule exists");
            assert!(evaluation.notified);
            delete
                .await
                .expect("the delete task")
                .expect("the delete is not a deadlock victim");
            assert_eq!(
                history(&db, id).await,
                vec![("firing".into(), "delivered".into(), Some("HTTP 200".into()))]
            );
            assert_eq!(rule_row(&db, id).await.channel_id, None);
        }

        #[tokio::test]
        async fn a_repointed_channel_drops_its_secret_unless_given_a_new_one() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let state = state_with(&db, &stub);
            let channel = add_channel(&db, &stub.hook_url(), true, true).await;
            let uri = format!("/api/v1/alert-channels/{channel}");

            // another path on the same origin keeps it
            let (status, body) = put(
                &state,
                uri.clone(),
                json!({"name": "ops", "endpoint": format!("{}/other", stub.url), "enabled": true}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["secret_configured"], true);

            // another origin does not
            let (status, body) = put(
                &state,
                uri,
                json!({"name": "ops", "endpoint": "HTTPS://Alerts.Example.com/rolter", "enabled": true}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["secret_configured"], false);
            // stored as the client parses it
            assert_eq!(body["endpoint"], "https://alerts.example.com/rolter");
            let detail: Value = sqlx::query_scalar(
                "select detail from audit_log where action = 'alert.channel.update' \
                 and target_id = $1 order by at desc limit 1",
            )
            .bind(channel)
            .fetch_one(db.pool())
            .await
            .expect("an audit entry");
            assert_eq!(detail["endpoint_changed"], true);
            assert_eq!(detail["secret_cleared"], true);
            assert!(!detail.to_string().contains("alerts.example.com"));
        }

        #[tokio::test]
        async fn a_channel_the_policy_now_denies_can_still_be_switched_off() {
            let Some(db) = scratch().await else { return };
            let stub = Stub::start().await;
            let mut state = state_with(&db, &stub);
            // saved while loopback was allowed
            let channel = add_channel(&db, &stub.hook_url(), true, false).await;
            state.egress = Arc::new(EgressPolicy {
                block_loopback: true,
                ..EgressPolicy::default()
            });
            let uri = format!("/api/v1/alert-channels/{channel}");
            let endpoint = stub.hook_url();

            let (status, body) = put(
                &state,
                uri.clone(),
                json!({"name": "ops", "endpoint": endpoint, "enabled": false}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["enabled"], false);
            // but not switched back on, or pointed at another denied address
            let (status, _) = put(
                &state,
                uri.clone(),
                json!({"name": "ops", "endpoint": endpoint, "enabled": true}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            let (status, _) = put(
                &state,
                uri,
                json!({"name": "ops", "endpoint": "http://2130706433/hook", "enabled": false}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
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
