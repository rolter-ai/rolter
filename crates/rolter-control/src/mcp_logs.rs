//! MCP tool-call event ingestion and query API.
//!
//! The eventual MCP proxy submits one normalized event after each tool call.
//! This module deliberately owns no MCP transport: it stays useful for stdio,
//! SSE, streamable HTTP and WebSocket implementations alike.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::analytics::{clamp_limit, client_or_503, window_params, WindowQuery, WHERE_WINDOW};
use crate::crud::{ApiError, ApiResult};
use crate::rbac::{require_superadmin, Principal};
use crate::ControlState;

const MCP_TRANSPORTS: &[&str] = &["stdio", "sse", "streamable_http", "websocket"];
const MCP_STATUSES: &[&str] = &[
    "success",
    "timeout",
    "auth_denied",
    "transport_error",
    "error",
];

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/mcp/events", post(ingest_event))
        .route("/api/v1/mcp/logs", get(list_events))
        .route("/api/v1/mcp/logs/summary", get(summary))
        .route("/api/v1/mcp/logs/{event_id}", get(event_detail))
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
}

fn validate_atom(value: &str, name: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!("{name} must be 1-256 visible characters"));
    }
    Ok(())
}

fn validate_event(event: &IngestEvent) -> Result<(), String> {
    for (value, name) in [
        (&event.event_id, "event_id"),
        (&event.server, "server"),
        (&event.tool, "tool"),
    ] {
        validate_atom(value, name)?;
    }
    if !MCP_TRANSPORTS.contains(&event.transport.as_str()) {
        return Err(format!("transport must be one of {MCP_TRANSPORTS:?}"));
    }
    if !MCP_STATUSES.contains(&event.status.as_str()) {
        return Err(format!("status must be one of {MCP_STATUSES:?}"));
    }
    if event.latency_ms > u32::MAX.into() {
        return Err("latency_ms exceeds UInt32 range".to_string());
    }
    Ok(())
}

fn redact(value: &mut Value, fields: &[String]) {
    match value {
        Value::Object(object) => {
            for (key, nested) in object.iter_mut() {
                if fields.iter().any(|field| field.eq_ignore_ascii_case(key)) {
                    *nested = Value::String("[REDACTED]".to_string());
                } else {
                    redact(nested, fields);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|nested| redact(nested, fields)),
        _ => {}
    }
}

fn capture(value: Option<Value>, enabled: bool, max_bytes: usize, fields: &[String]) -> String {
    if !enabled || max_bytes == 0 {
        return String::new();
    }
    let Some(mut value) = value else {
        return String::new();
    };
    redact(&mut value, fields);
    let mut rendered = serde_json::to_string(&value).unwrap_or_default();
    if rendered.len() > max_bytes {
        let end = rendered.floor_char_boundary(max_bytes);
        rendered.truncate(end);
        rendered.push_str("…[truncated]");
    }
    rendered
}

/// Never persist a caller-supplied diagnostic verbatim. The status is the
/// durable machine-readable signal; this optional display string is a bounded,
/// secret-free category for the log detail viewer.
fn safe_error(status: &str, error: Option<&str>) -> String {
    if status == "success" || error.is_none() {
        return String::new();
    }
    let error = error.unwrap_or_default().to_ascii_lowercase();
    if error.contains("timeout") {
        "timeout".to_string()
    } else if error.contains("auth") || error.contains("forbidden") || error.contains("denied") {
        "authentication denied".to_string()
    } else if error.contains("connect") || error.contains("transport") || error.contains("dns") {
        "transport failure".to_string()
    } else {
        "tool invocation failed".to_string()
    }
}

async fn ingest_event(
    principal: Principal,
    State(state): State<ControlState>,
    Json(event): Json<IngestEvent>,
) -> ApiResult<StatusCode> {
    require_superadmin(&principal)?;
    validate_event(&event)
        .map_err(|message| ApiError::Core(rolter_core::Error::Config(message)))?;
    let logging = state.store.load().await?.logging;
    let capture_policy = logging.payload_capture;
    let row = json!({
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
        "arguments": capture(event.arguments, capture_policy.enabled, capture_policy.max_bytes, &capture_policy.redact_fields),
        "result": capture(event.result, capture_policy.enabled, capture_policy.max_bytes, &capture_policy.redact_fields),
        "error": safe_error(&event.status, event.error.as_deref()),
    });
    let ch = client_or_503(&state).map_err(|_| {
        ApiError::Core(rolter_core::Error::Store(
            "MCP log ingestion requires CLICKHOUSE_URL".to_string(),
        ))
    })?;
    ch.insert_mcp_tool_call(&row)
        .await
        .map_err(|err| ApiError::Core(rolter_core::Error::Store(err.to_string())))?;
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

fn parse_cursor(cursor: Option<&str>) -> Result<(String, String), &'static str> {
    let Some(cursor) = cursor.filter(|value| !value.is_empty()) else {
        return Ok((String::new(), String::new()));
    };
    let Some((timestamp, event_id)) = cursor.split_once('|') else {
        return Err("cursor must be timestamp|event_id");
    };
    if timestamp.is_empty() || event_id.is_empty() || timestamp.len() > 64 || event_id.len() > 256 {
        return Err("cursor must be timestamp|event_id");
    }
    Ok((timestamp.to_string(), event_id.to_string()))
}

fn validate_filter(value: Option<&str>, label: &str) -> Result<(), String> {
    if value.is_some_and(|value| value.len() > 256 || value.chars().any(char::is_control)) {
        return Err(format!("{label} filter is invalid"));
    }
    Ok(())
}

async fn list_events(
    principal: Principal,
    State(state): State<ControlState>,
    Query(q): Query<McpLogsQuery>,
) -> Response {
    if let Err(error) = require_superadmin(&principal) {
        return error.into_response();
    }
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
    let (cursor_ts, cursor_event_id) = match parse_cursor(q.cursor.as_deref()) {
        Ok(cursor) => cursor,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": message}})),
            )
                .into_response()
        }
    };
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return response,
    };
    let limit = clamp_limit(q.limit);
    let sql = format!(
        "select ts, event_id, server, tool, transport, status, latency_ms, org_id, team_id, project_id, \
                virtual_key_id, user_id, request_id, trace_id, error \
         from mcp_tool_call_logs where {WHERE_WINDOW} \
           and ({{server:String}} = '' or server = {{server:String}}) \
           and ({{tool:String}} = '' or tool = {{tool:String}}) \
           and ({{transport:String}} = '' or transport = {{transport:String}}) \
           and ({{status:String}} = '' or status = {{status:String}}) \
           and ({{key:String}} = '' or virtual_key_id = {{key:String}}) \
           and ({{user:String}} = '' or user_id = {{user:String}}) \
           and ({{cursor_ts:String}} = '' or ts < parseDateTime64BestEffort({{cursor_ts:String}}) \
                or (ts = parseDateTime64BestEffort({{cursor_ts:String}}) and event_id < {{cursor_event_id:String}})) \
         order by ts desc, event_id desc limit {{limit:UInt32}} format JSON"
    );
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
        ("cursor_event_id", cursor_event_id),
        ("limit", limit.to_string()),
    ] {
        params.push((format!("param_{name}"), value));
    }
    match ch.query(&sql, &params).await {
        Ok(data) => {
            let next_cursor = data.last().and_then(|row| {
                Some(format!(
                    "{}|{}",
                    row.get("ts")?.as_str()?,
                    row.get("event_id")?.as_str()?
                ))
            });
            Json(json!({"data": data, "next_cursor": next_cursor})).into_response()
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": error.to_string()}})),
        )
            .into_response(),
    }
}

async fn event_detail(
    principal: Principal,
    State(state): State<ControlState>,
    Path(event_id): Path<String>,
) -> Response {
    if let Err(error) = require_superadmin(&principal) {
        return error.into_response();
    }
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
    let sql = "select ts, event_id, server, tool, transport, status, latency_ms, org_id, team_id, project_id, \
                      virtual_key_id, user_id, request_id, trace_id, arguments, result, error \
               from mcp_tool_call_logs where event_id = {event_id:String} \
               order by ts desc limit 1 format JSON";
    match ch
        .query(sql, &[("param_event_id".to_string(), event_id)])
        .await
    {
        Ok(data) => match data.into_iter().next() {
            Some(row) => Json(row).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": error.to_string()}})),
        )
            .into_response(),
    }
}

async fn summary(
    principal: Principal,
    State(state): State<ControlState>,
    Query(q): Query<WindowQuery>,
) -> Response {
    if let Err(error) = require_superadmin(&principal) {
        return error.into_response();
    }
    let ch = match client_or_503(&state) {
        Ok(ch) => ch,
        Err(response) => return response,
    };
    let sql = format!(
        "select count() as calls, countIf(status != 'success') as failures, \
                round(avg(latency_ms), 1) as avg_latency_ms, \
                quantile(0.95)(latency_ms) as p95_latency_ms \
         from mcp_tool_call_logs where {WHERE_WINDOW} format JSON"
    );
    match ch.query(&sql, &window_params(&q)).await {
        Ok(data) => Json(json!({"data": data})).into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"message": error.to_string()}})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            };
            assert!(validate_event(&event).is_ok());
        }
    }

    #[test]
    fn capture_redacts_nested_sensitive_arguments_before_truncation() {
        let captured = capture(
            Some(json!({"nested": {"token": "secret"}, "query": "hello"})),
            true,
            1024,
            &["token".to_string()],
        );
        assert!(captured.contains("[REDACTED]"));
        assert!(!captured.contains("secret"));
    }

    #[test]
    fn cursor_requires_timestamp_and_event_id() {
        assert!(parse_cursor(Some("2026-07-19 12:00:00.000|evt-1")).is_ok());
        assert!(parse_cursor(Some("not-a-cursor")).is_err());
    }

    #[test]
    fn error_message_is_reduced_to_a_safe_category() {
        assert_eq!(
            safe_error("transport_error", Some("connect failed: bearer secret")),
            "transport failure"
        );
    }
}
