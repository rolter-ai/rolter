//! MCP tool-call event ingestion and query API.
//!
//! The eventual MCP proxy submits one normalized event after each tool call.
//! This module deliberately owns no MCP transport: it stays useful for stdio,
//! SSE, streamable HTTP and WebSocket implementations alike. The gateway proxies
//! only SSE and streamable HTTP; a `websocket` event can come only from an
//! external producer, and is accepted so such rows stay valid.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::analytics::{
    clamp_limit, client_or_503, keyset_predicate, next_keyset_cursor, parse_keyset_cursor,
    query_failed, window_params, with_access, WindowQuery, WHERE_WINDOW,
};
use crate::analytics_access::{McpLogAccess, MCP_ROW_VISIBLE, PAYLOAD_VISIBLE};
use crate::crud::{ApiError, ApiResult};
use crate::ingest_failure::{self, Stream};
use crate::rbac::{authorize_superadmin, Principal};
use crate::rbac_matrix::superadmin_cap;
use crate::request_log_columns::McpColumns;
use crate::time_bounds::{InvalidParam, Query, TimeBounds};
use crate::ControlState;

use parking_lot::Mutex;
use rolter_core::mcp_log::{capture, safe_error, MCP_OUTCOMES, MCP_STATUSES};
use rolter_core::payload_redaction::PayloadRedactor;
use rolter_core::MCP_TRANSPORTS;
pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/mcp/events", post(ingest_event))
        .route("/api/v1/mcp/logs", get(list_events))
        .route("/api/v1/mcp/logs/summary", get(summary))
        .route("/api/v1/mcp/logs/{event_id}", get(event_detail))
}

/// Every column `mcp_tool_call_logs` has in the database the client reads from.
const MCP_PROBE_SQL: &str = "select name from system.columns \
     where database = currentDatabase() and table = 'mcp_tool_call_logs' format JSON";

/// Which optional columns the table has: `error_scopes` (`042`),
/// `notification` (`047`) and `outcome` (`051`). A table the probe cannot see, or a probe ClickHouse
/// refuses, reads as having them all, the behaviour before the probe existed, so
/// it is no worse than it was; a server that cannot be reached is an error, since
/// the select that follows would fail on the same connection.
async fn mcp_columns(ch: &crate::analytics::ClickHouseClient) -> anyhow::Result<McpColumns> {
    let now = std::time::Instant::now();
    if let Some(columns) = ch.mcp_columns.fresh(now) {
        return Ok(columns);
    }
    let columns = match ch.query(MCP_PROBE_SQL, &[]).await {
        Ok(rows) if rows.is_empty() => McpColumns::ALL,
        Ok(rows) => McpColumns::from_names(
            rows.iter()
                .filter_map(|row| row.get("name").and_then(Value::as_str)),
        ),
        Err(err) if crate::analytics::is_transport(&err) => return Err(err),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not read the columns of mcp_tool_call_logs; selecting every optional column"
            );
            McpColumns::ALL
        }
    };
    ch.mcp_columns.store(now, columns);
    if !columns.error_scopes {
        tracing::info!(
            "mcp_tool_call_logs lacks error_scopes; apply clickhouse/042_mcp_log_error_scopes.sql to record them"
        );
    }
    if !columns.notification {
        tracing::info!(
            "mcp_tool_call_logs lacks notification; apply clickhouse/047_mcp_log_notification.sql to tell notifications from replied calls"
        );
    }
    if !columns.outcome {
        tracing::info!(
            "mcp_tool_call_logs lacks outcome; apply clickhouse/051_mcp_log_outcome.sql to tell accepted legacy-sse calls from answered ones"
        );
    }
    Ok(columns)
}

#[derive(Debug, Deserialize)]
struct IngestEvent {
    event_id: String,
    server: String,
    tool: String,
    transport: String,
    status: String,
    latency_ms: u64,
    #[serde(default)]
    org_id: String,
    #[serde(default)]
    team_id: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    virtual_key_id: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    trace_id: String,
    arguments: Option<Value>,
    result: Option<Value>,
    error: Option<String>,
    /// when the tool call itself happened, RFC 3339. only the submitter knows
    /// this — a proxy that buffers or retries can be minutes late, and leaving
    /// the column to its `now64(3)` default stamps the ingest time instead
    /// (#1210)
    ts: Option<String>,
    /// the call was a JSON-RPC notification, so no reply was awaited (#3466).
    /// Stored only when set, so a table without `047` still accepts the row
    #[serde(default)]
    notification: bool,
    /// what refines `status` for a call whose result nothing judged, one of
    /// `MCP_OUTCOMES` (#3529), beside a `success` status. Stored only when
    /// set, so a table without `051` still accepts the row
    #[serde(default)]
    outcome: Option<String>,
}

fn validate_atom(value: &str, name: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!("{name} must be 1-256 visible characters"));
    }
    Ok(())
}

fn validate_event(event: &IngestEvent) -> Result<(), String> {
    // an `unread` row stands for a whole body whose calls were not itemised,
    // so it is the one row that names no tool (#3528)
    let unread = event.outcome.as_deref() == Some("unread");
    for (value, name) in [
        (&event.event_id, "event_id"),
        (&event.server, "server"),
        (&event.tool, "tool"),
    ] {
        if name == "tool" && unread && value.is_empty() {
            continue;
        }
        validate_atom(value, name)?;
    }
    if !MCP_TRANSPORTS.contains(&event.transport.as_str()) {
        return Err(format!("transport must be one of {MCP_TRANSPORTS:?}"));
    }
    if !MCP_STATUSES.contains(&event.status.as_str()) {
        return Err(format!("status must be one of {MCP_STATUSES:?}"));
    }
    if let Some(outcome) = event.outcome.as_deref().filter(|o| !o.is_empty()) {
        if !MCP_OUTCOMES.contains(&outcome) {
            return Err(format!("outcome must be one of {MCP_OUTCOMES:?}"));
        }
        // the status is what an un-migrated table reads the row as
        if event.status != "success" {
            return Err("an outcome goes with the status success".to_string());
        }
    }
    if event.latency_ms > u32::MAX.into() {
        return Err("latency_ms exceeds UInt32 range".to_string());
    }
    Ok(())
}

/// The instant the tool call happened, in the RFC 3339 form ClickHouse's
/// `best_effort` parser reads into `DateTime64(3)`. Falls back to the ingest
/// time when the submitter names none, which is the closest honest answer
/// available here.
fn event_ts(supplied: Option<&str>) -> Result<String, String> {
    let ts = match supplied.filter(|value| !value.is_empty()) {
        Some(value) => DateTime::parse_from_rfc3339(value)
            .map_err(|_| "ts must be an RFC 3339 timestamp".to_string())?
            .with_timezone(&Utc),
        None => Utc::now(),
    };
    Ok(ts.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The redaction policy of the stored capture settings, compiled once and
/// reused until they change: compiling the detectors per event would put a
/// regex build on every ingested call. The same policy the request log applies;
/// one that does not compile still leaves the key rule in force.
fn redactor_for(policy: &rolter_core::PayloadCaptureConfig) -> Arc<PayloadRedactor> {
    static CACHE: Mutex<Option<(String, Arc<PayloadRedactor>)>> = Mutex::new(None);
    let key = serde_json::to_string(policy).unwrap_or_default();
    let mut slot = CACHE.lock();
    if let Some((cached, redactor)) = &*slot {
        if *cached == key {
            return Arc::clone(redactor);
        }
    }
    let redactor = Arc::new(policy.redactor().0);
    *slot = Some((key, Arc::clone(&redactor)));
    redactor
}

async fn ingest_event(
    principal: Principal,
    State(state): State<ControlState>,
    Json(event): Json<IngestEvent>,
) -> ApiResult<StatusCode> {
    authorize_superadmin(&principal, superadmin_cap!("mcp_log", Create))?;
    validate_event(&event)
        .map_err(|message| ApiError::Core(rolter_core::Error::Config(message)))?;
    let logging = state.store.load().await?.logging;
    let capture_policy = logging.payload_capture;
    let redactor = redactor_for(&capture_policy);
    let ts = event_ts(event.ts.as_deref())
        .map_err(|message| ApiError::Core(rolter_core::Error::Config(message)))?;
    let mut row = json!({
        "ts": ts,
        "event_id": event.event_id,
        "server": event.server,
        "tool": event.tool,
        "transport": event.transport,
        "status": event.status,
        "latency_ms": event.latency_ms as u32,
        "org_id": event.org_id,
        "team_id": event.team_id,
        "project_id": event.project_id,
        "virtual_key_id": event.virtual_key_id,
        "user_id": event.user_id,
        "request_id": event.request_id,
        "trace_id": event.trace_id,
        "arguments": capture(event.arguments, capture_policy.enabled, capture_policy.max_bytes, &redactor),
        "result": capture(event.result, capture_policy.enabled, capture_policy.max_bytes, &redactor),
        "error": safe_error(&event.status, event.error.as_deref()),
    });
    if event.notification {
        row["notification"] = json!(1);
    }
    if let Some(outcome) = event.outcome.filter(|o| !o.is_empty()) {
        row["outcome"] = json!(outcome);
    }
    let ch = client_or_503(&state).map_err(|_| {
        ingest_failure::unconfigured(
            &state.metrics,
            Stream::McpLogs,
            "MCP log ingestion requires CLICKHOUSE_URL",
        )
    })?;
    ch.insert_mcp_tool_call(&row)
        .await
        .map_err(|err| ingest_failure::insert_failed(&state.metrics, Stream::McpLogs, &err))?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Deserialize)]
struct McpLogsQuery {
    since: Option<String>,
    until: Option<String>,
    server: Option<String>,
    tool: Option<String>,
    transport: Option<String>,
    status: Option<String>,
    key: Option<String>,
    user: Option<String>,
    limit: Option<u32>,
    /// opaque `timestamp|event_id` cursor returned from the preceding page
    cursor: Option<String>,
}

impl TimeBounds for McpLogsQuery {
    fn time_bounds(&self) -> (Option<&str>, Option<&str>) {
        (self.since.as_deref(), self.until.as_deref())
    }
}

fn validate_filter(value: Option<&str>, label: &str) -> Result<(), String> {
    if value.is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control)) {
        return Err(format!("{label} filter is invalid"));
    }
    Ok(())
}

/// Build the keyset-paginated event list query.
///
/// Rows are filtered to the caller's tenancy in the database, and a caller's
/// own rows always pass (#1831). The list carries no tool arguments or results,
/// so it needs no body mask.
///
/// The cursor bound comes from the shared [`keyset_predicate`], which the
/// invocations list pages on too — one place owns the `OrZero` parse that an
/// absent cursor depends on (#1177).
fn list_events_sql(columns: McpColumns) -> String {
    let cursor = keyset_predicate("event_id");
    let notification = notification_column(columns);
    let outcome = outcome_column(columns);
    let not_awaited = not_awaited(columns);
    format!(
        "select ts, event_id, server, tool, transport, status, latency_ms, org_id, team_id, project_id, \
                virtual_key_id, user_id, request_id, trace_id, error, {notification}, {outcome} \
         from mcp_tool_call_logs where {WHERE_WINDOW} \
           and {MCP_ROW_VISIBLE} \
           and ({{server:String}} = '' or server = {{server:String}}) \
           and ({{tool:String}} = '' or tool = {{tool:String}}) \
           and ({{transport:String}} = '' or transport = {{transport:String}}) \
           and ({{status:String}} = '' or if({{status:String}} = 'not_awaited', {not_awaited}, \
                status = {{status:String}} and ({{status:String}} != 'success' or not {not_awaited}))) \
           and ({{key:String}} = '' or virtual_key_id = {{key:String}}) \
           and ({{user:String}} = '' or user_id = {{user:String}}) \
           and {cursor} \
         order by ts desc, event_id desc limit {{limit:UInt32}} format JSON"
    )
}

async fn list_events(
    McpLogAccess(access): McpLogAccess,
    State(state): State<ControlState>,
    Query(q): Query<McpLogsQuery>,
) -> Response {
    for (value, label) in [
        (q.server.as_deref(), "server"),
        (q.tool.as_deref(), "tool"),
        (q.transport.as_deref(), "transport"),
        (q.status.as_deref(), "status"),
        (q.key.as_deref(), "key"),
        (q.user.as_deref(), "user"),
    ] {
        if let Err(message) = validate_filter(value, label) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": message}})),
            )
                .into_response();
        }
    }
    let (cursor_ts, cursor_event_id) = match parse_keyset_cursor(q.cursor.as_deref(), "event_id") {
        Ok(cursor) => cursor,
        Err(message) => return InvalidParam::cursor(message).into_response(),
    };
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return response,
    };
    let limit = clamp_limit(q.limit);
    let columns = match mcp_columns(ch).await {
        Ok(present) => present,
        Err(error) => return query_failed("mcp log query failed", &error),
    };
    let sql = list_events_sql(columns);
    let mut params = window_params(&WindowQuery {
        since: q.since,
        until: q.until,
        bucket: None,
    });
    for (name, value) in [
        ("server", q.server.unwrap_or_default()),
        ("tool", q.tool.unwrap_or_default()),
        ("transport", q.transport.unwrap_or_default()),
        ("status", q.status.unwrap_or_default()),
        ("key", q.key.unwrap_or_default()),
        ("user", q.user.unwrap_or_default()),
        ("cursor_ts", cursor_ts),
        ("cursor_id", cursor_event_id),
        ("limit", limit.to_string()),
    ] {
        params.push((format!("param_{name}"), value));
    }
    match ch.query(&sql, &with_access(params, &access)).await {
        Ok(data) => {
            let next_cursor = next_keyset_cursor(&data, "event_id");
            Json(json!({"data": data, "next_cursor": next_cursor})).into_response()
        }
        Err(error) => {
            // the table may have changed under the probe's answer
            ch.mcp_columns.forget();
            query_failed("mcp log query failed", &error)
        }
    }
}

/// `notification`, or its default while the table lacks `047`, typed as the
/// column is so the row reads the same either way
fn notification_column(columns: McpColumns) -> &'static str {
    if columns.notification {
        "notification"
    } else {
        "toUInt8(0) as notification"
    }
}

/// `outcome`, or its default while the table lacks `051`
fn outcome_column(columns: McpColumns) -> &'static str {
    if columns.outcome {
        "outcome"
    } else {
        "toString('') as outcome"
    }
}

/// Whether a row is a call no reply was awaited for, so its status says
/// nothing about the tool (#3526, #3529): a notification the upstream
/// accepted, or a call with an `outcome` such as `accepted`. Reads `0` for what the table cannot record.
fn not_awaited(columns: McpColumns) -> String {
    let mut parts = Vec::new();
    if columns.notification {
        // a notification the HTTP layer refused or failed was not delivered,
        // so it is a failure like any other
        parts.push("(notification = 1 and status = 'success')");
    }
    if columns.outcome {
        parts.push("outcome != ''");
    }
    if parts.is_empty() {
        "0".to_string()
    } else {
        format!("({})", parts.join(" or "))
    }
}

/// The detail query. The two bodies are masked in the database for a caller
/// below the `request_payload` floor at the row's scope, and `payload_withheld`
/// says so, exactly as the invocation list does (#1820). The flag reads the
/// table-qualified columns because an unqualified `arguments` in the same
/// select would resolve to the masked alias and always report "nothing
/// withheld".
fn event_detail_sql(columns: McpColumns) -> String {
    // the column's own default stands in until the migration is applied, typed
    // as the column is so the row reads the same either way
    let error_scopes = if columns.error_scopes {
        "error_scopes"
    } else {
        "emptyArrayString() as error_scopes"
    };
    let notification = notification_column(columns);
    let outcome = outcome_column(columns);
    format!(
        "select ts, event_id, server, tool, transport, status, latency_ms, org_id, team_id, project_id, \
                virtual_key_id, user_id, request_id, trace_id, \
                if({PAYLOAD_VISIBLE}, arguments, '') as arguments, \
                if({PAYLOAD_VISIBLE}, result, '') as result, \
                toUInt8(not {PAYLOAD_VISIBLE} \
                        and (notEmpty(mcp_tool_call_logs.arguments) \
                             or notEmpty(mcp_tool_call_logs.result))) as payload_withheld, \
                error, {error_scopes}, {notification}, {outcome} \
         from mcp_tool_call_logs \
         where event_id = {{event_id:String}} and {MCP_ROW_VISIBLE} \
         order by ts desc limit 1 format JSON"
    )
}

/// One event. A row the caller may not see answers `404`, the same as one that
/// does not exist, so an id cannot be probed across tenants.
async fn event_detail(
    McpLogAccess(access): McpLogAccess,
    State(state): State<ControlState>,
    Path(event_id): Path<String>,
) -> Response {
    if let Err(message) = validate_filter(Some(&event_id), "event_id") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": message}})),
        )
            .into_response();
    }
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return response,
    };
    let params = with_access(vec![("param_event_id".to_string(), event_id)], &access);
    let columns = match mcp_columns(ch).await {
        Ok(present) => present,
        Err(error) => return query_failed("mcp log query failed", &error),
    };
    match ch.query(&event_detail_sql(columns), &params).await {
        Ok(data) => match data.into_iter().next() {
            Some(row) => Json(row).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        Err(error) => {
            // the table may have changed under the probe's answer
            ch.mcp_columns.forget();
            query_failed("mcp log query failed", &error)
        }
    }
}

async fn summary(
    McpLogAccess(access): McpLogAccess,
    State(state): State<ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return response,
    };
    let columns = match mcp_columns(ch).await {
        Ok(present) => present,
        Err(error) => return query_failed("mcp log query failed", &error),
    };
    match ch
        .query(
            &summary_sql(columns),
            &with_access(window_params(&q), &access),
        )
        .await
    {
        Ok(data) => Json(json!({"data": data})).into_response(),
        Err(error) => {
            ch.mcp_columns.forget();
            query_failed("mcp log query failed", &error)
        }
    }
}

/// The summary row. A call no reply was awaited for (a notification the
/// upstream accepted, or one recorded `accepted` or `unread`) says nothing about whether the tool succeeded, so it is
/// counted apart in `not_awaited` and left out of `successes`, `failures` and
/// both rates, whose denominator is `calls - not_awaited` (#3526). A rate is
/// `null` for a window with no awaited call. The latencies are over every call.
fn summary_sql(columns: McpColumns) -> String {
    let not_awaited = not_awaited(columns);
    format!(
        "select count() as calls, \
                countIf({not_awaited}) as not_awaited, \
                countIf(not {not_awaited} and status = 'success') as successes, \
                countIf(not {not_awaited} and status != 'success') as failures, \
                if(calls = not_awaited, null, round(successes / (calls - not_awaited), 4)) \
                    as success_rate, \
                if(calls = not_awaited, null, round(failures / (calls - not_awaited), 4)) \
                    as failure_rate, \
                round(avg(latency_ms), 1) as avg_latency_ms, \
                quantile(0.95)(latency_ms) as p95_latency_ms \
         from mcp_tool_call_logs where {WHERE_WINDOW} and {MCP_ROW_VISIBLE} format JSON"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_log_columns::McpColumnCache;

    #[test]
    fn event_validation_covers_success_timeout_auth_and_transport() {
        for status in ["success", "timeout", "auth_denied", "transport_error"] {
            let event = IngestEvent {
                event_id: format!("evt-{status}"),
                server: "docs".to_string(),
                tool: "search".to_string(),
                transport: "streamable_http".to_string(),
                status: status.to_string(),
                latency_ms: 12,
                org_id: String::new(),
                team_id: String::new(),
                project_id: String::new(),
                virtual_key_id: String::new(),
                user_id: String::new(),
                request_id: "req-1".to_string(),
                trace_id: "trace-1".to_string(),
                arguments: None,
                result: None,
                error: None,
                ts: None,
                notification: false,
                outcome: None,
            };
            assert!(validate_event(&event).is_ok());
        }
    }

    #[test]
    fn an_ingested_outcome_is_one_of_the_known_set_beside_success() {
        let event = |status: &str, outcome: &str| IngestEvent {
            event_id: "evt".to_string(),
            server: "docs".to_string(),
            tool: "search".to_string(),
            transport: "sse".to_string(),
            status: status.to_string(),
            latency_ms: 1,
            org_id: String::new(),
            team_id: String::new(),
            project_id: String::new(),
            virtual_key_id: String::new(),
            user_id: String::new(),
            request_id: String::new(),
            trace_id: String::new(),
            arguments: None,
            result: None,
            error: None,
            ts: None,
            notification: false,
            outcome: Some(outcome.to_string()),
        };
        assert!(validate_event(&event("success", "accepted")).is_ok());
        assert!(validate_event(&event("error", "")).is_ok());
        assert!(validate_event(&event("success", "maybe")).is_err());
        assert!(validate_event(&event("error", "accepted")).is_err());
        // only an `unread` row may name no tool
        let toolless = |outcome: &str| IngestEvent {
            tool: String::new(),
            ..event("success", outcome)
        };
        assert!(validate_event(&toolless("unread")).is_ok());
        assert!(validate_event(&toolless("accepted")).is_err());
        assert!(validate_event(&toolless("")).is_err());
        assert!(validate_event(&IngestEvent {
            tool: " ".to_string(),
            ..event("success", "unread")
        })
        .is_err());
    }

    /// #3526: the rates leave out every row no reply was awaited for, and a
    /// table without the columns reads every row as awaited
    #[test]
    fn the_summary_counts_unawaited_calls_apart_from_the_rates() {
        let all = summary_sql(McpColumns::ALL);
        assert!(all.contains(
            "countIf(((notification = 1 and status = 'success') or outcome != '')) as not_awaited"
        ));
        assert!(all.contains(
            "countIf(not ((notification = 1 and status = 'success') or outcome != '') \
             and status != 'success') as failures"
        ));
        assert!(all.contains("round(failures / (calls - not_awaited), 4)"));
        let none = summary_sql(McpColumns::default());
        assert!(none.contains("countIf(0) as not_awaited"));
        assert!(!none.contains("notification") && !none.contains("outcome"));
        let notes_only = summary_sql(McpColumns {
            notification: true,
            ..McpColumns::default()
        });
        assert!(notes_only
            .contains("countIf(((notification = 1 and status = 'success'))) as not_awaited"));
        // `051` applied without `047`: only the outcome marks a call not awaited
        let outcome_only = McpColumns {
            outcome: true,
            ..McpColumns::default()
        };
        let summary = summary_sql(outcome_only);
        assert!(summary.contains("countIf((outcome != '')) as not_awaited"));
        assert!(!summary.contains("notification"));
        let list = list_events_sql(outcome_only);
        assert!(list.contains("error, toUInt8(0) as notification, outcome"));
        let detail = event_detail_sql(outcome_only);
        assert!(detail.contains("toUInt8(0) as notification, outcome from"));
    }

    #[test]
    fn a_supplied_event_time_is_normalized_and_a_bad_one_is_rejected() {
        // the submitter's own instant survives, at the column's precision
        assert_eq!(
            event_ts(Some("2025-09-05T00:02:22.061+00:00")).unwrap(),
            "2025-09-05T00:02:22.061Z"
        );
        // an offset is carried into utc rather than dropped
        assert_eq!(
            event_ts(Some("2025-09-05T02:02:22.061+02:00")).unwrap(),
            "2025-09-05T00:02:22.061Z"
        );
        assert!(event_ts(Some("not a timestamp")).is_err());
        // no ts means ingest time, still in the form clickhouse parses
        let now = event_ts(None).unwrap();
        assert!(now.ends_with('Z'), "{now}");
        assert!(DateTime::parse_from_rfc3339(&now).is_ok(), "{now}");
    }

    #[test]
    fn list_query_never_strict_parses_an_absent_cursor() {
        // an absent cursor binds `cursor_ts` to ''; clickhouse still evaluates
        // the parse of that constant, so a strict parse failed every first page
        // with "Cannot read DateTime" (#1177)
        let sql = list_events_sql(McpColumns::ALL);
        assert!(!sql.contains("parseDateTime64BestEffort("));
        // two for the shared window bounds, two for the keyset cursor
        assert_eq!(sql.matches("parseDateTime64BestEffortOrZero(").count(), 4);
        // the short-circuit guard stays, so keyset pagination is unchanged
        assert!(sql.contains("{cursor_ts:String} = '' or ts < "));
        assert!(sql.contains("and event_id < {cursor_id:String}"));
    }

    #[test]
    fn list_query_binds_every_filter_as_a_param() {
        let sql = list_events_sql(McpColumns::ALL);
        for param in [
            "{server:String}",
            "{tool:String}",
            "{transport:String}",
            "{status:String}",
            "{key:String}",
            "{user:String}",
            "{limit:UInt32}",
        ] {
            assert!(sql.contains(param), "missing bound param {param}");
        }
    }

    #[test]
    fn every_read_is_filtered_to_the_callers_reach() {
        // list and summary share the row filter; the detail query adds the mask
        let list = list_events_sql(McpColumns::ALL);
        assert!(list.contains(&format!("and {MCP_ROW_VISIBLE}")));
        let detail = event_detail_sql(McpColumns::ALL);
        assert!(detail.contains(&format!("and {MCP_ROW_VISIBLE}")));
        assert!(detail.contains(&format!(
            "if({PAYLOAD_VISIBLE}, arguments, '') as arguments"
        )));
        assert!(detail.contains(&format!("if({PAYLOAD_VISIBLE}, result, '') as result")));
        // the flag must read the column, not the masked alias of the same name
        assert!(detail.contains("notEmpty(mcp_tool_call_logs.arguments)"));
        assert!(detail.contains("{event_id:String}"));
    }

    #[test]
    fn the_detail_reads_a_missing_optional_column_as_its_default() {
        // before `042` or `047` is applied the select stands the column's default
        // in, so it still answers and the row has the same shape
        let all = event_detail_sql(McpColumns::ALL);
        assert!(all.contains("error, error_scopes, notification, outcome from"));
        let none = event_detail_sql(McpColumns::default());
        assert!(none.contains(
            "error, emptyArrayString() as error_scopes, toUInt8(0) as notification, \
             toString('') as outcome from"
        ));
        let scopes_only = event_detail_sql(McpColumns {
            error_scopes: true,
            ..McpColumns::default()
        });
        assert!(scopes_only.contains(
            "error, error_scopes, toUInt8(0) as notification, toString('') as outcome from"
        ));
        // the list reads the same two flags the same way
        let list = list_events_sql(McpColumns::default());
        assert!(list.contains("error, toUInt8(0) as notification, toString('') as outcome"));
        let list = list_events_sql(McpColumns::ALL);
        assert!(list.contains("error, notification, outcome"));
    }

    /// a ClickHouse stand-in for the detail read: it answers the probe from
    /// `present`, and refuses a select that names a column bare when the table
    /// lacks it, as the real engine does
    async fn detail_stand_in(present: McpColumns) -> String {
        use axum::routing::post;
        let app = Router::new().fallback(post(move |sql: String| async move {
            if sql.contains("system.columns") {
                let mut rows = vec![json!({"name": "ts"}), json!({"name": "error"})];
                if present.error_scopes {
                    rows.push(json!({"name": "error_scopes"}));
                }
                if present.notification {
                    rows.push(json!({"name": "notification"}));
                }
                if present.outcome {
                    rows.push(json!({"name": "outcome"}));
                }
                return Json(json!({"data": rows})).into_response();
            }
            for (column, has) in [
                (", error_scopes,", present.error_scopes),
                (", notification,", present.notification),
                (", outcome ", present.outcome),
            ] {
                if !has && sql.contains(column) {
                    return (
                        StatusCode::NOT_FOUND,
                        "Code: 47. DB::Exception: Unknown expression identifier",
                    )
                        .into_response();
                }
            }
            Json(json!({"data": [{"event_id": "e"}]})).into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        url
    }

    #[tokio::test]
    async fn the_detail_reads_a_table_without_the_optional_columns_and_one_with_them() {
        for bits in 0..8u8 {
            let present = McpColumns {
                error_scopes: bits & 1 != 0,
                notification: bits & 2 != 0,
                outcome: bits & 4 != 0,
            };
            let ch = crate::analytics::ClickHouseClient::new(&detail_stand_in(present).await);
            let probed = mcp_columns(&ch).await.unwrap();
            assert_eq!(probed, present);
            let rows = ch
                .query(&event_detail_sql(probed), &[])
                .await
                .expect("the select names only columns the table has");
            assert_eq!(rows.len(), 1);
            // the unprobed select is what failed before the probe existed
            assert_eq!(
                ch.query(&event_detail_sql(McpColumns::ALL), &[])
                    .await
                    .is_ok(),
                present.complete()
            );
        }
    }

    #[test]
    fn the_column_probe_settles_only_once_every_column_is_found() {
        let cache = McpColumnCache::default();
        let now = std::time::Instant::now();
        assert_eq!(cache.fresh(now), None);
        let partial = McpColumns {
            error_scopes: true,
            ..McpColumns::default()
        };
        cache.store(now, partial);
        assert_eq!(cache.fresh(now), Some(partial));
        // a missing column is looked for again within the minute, a full set
        // is kept for longer
        let later = now + crate::request_log_columns::UNSETTLED_FOR;
        assert_eq!(cache.fresh(later), None);
        cache.store(now, McpColumns::ALL);
        assert_eq!(cache.fresh(later), Some(McpColumns::ALL));
        cache.forget();
        assert_eq!(cache.fresh(now), None);
    }

    /// #3466: an ingested row naming `notification` is stored by a table that
    /// has not applied `047`, because the insert skips the fields it lacks
    #[tokio::test]
    async fn an_ingest_insert_skips_a_column_the_table_lacks() {
        use axum::routing::post;
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = seen.clone();
        let app = Router::new().fallback(post(move |uri: axum::http::Uri, _body: String| {
            let recorded = recorded.clone();
            async move {
                recorded
                    .lock()
                    .push(uri.query().unwrap_or_default().to_string());
                StatusCode::OK
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let ch = crate::analytics::ClickHouseClient::new(&url);
        ch.insert_mcp_tool_call(&json!({"event_id": "e", "notification": 1}))
            .await
            .unwrap();
        let query = seen.lock()[0].clone();
        assert!(
            query.contains("input_format_skip_unknown_fields=1"),
            "{query}"
        );
        assert!(
            query.contains("date_time_input_format=best_effort"),
            "{query}"
        );
    }

    #[test]
    fn cursor_requires_timestamp_and_event_id() {
        assert!(parse_keyset_cursor(Some("2026-07-19 12:00:00.000|evt-1"), "event_id").is_ok());
        // the rejection names this list's own id column
        assert_eq!(
            parse_keyset_cursor(Some("not-a-cursor"), "event_id"),
            Err("cursor must be timestamp|event_id".to_string())
        );
    }
}
