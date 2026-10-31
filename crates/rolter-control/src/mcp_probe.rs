//! Connection status and a try-it surface for registered MCP servers (#951).
//!
//! Configuring a server used to be write-and-hope: the dashboard accepted the
//! row and said nothing about whether it worked. Two endpoints close that,
//! both made by the control plane through [`crate::mcp_client`] under the same
//! egress policy as discovery:
//!
//! - `POST /api/v1/mcp-servers/{id}/test` runs the `initialize` handshake and
//!   reports what happened in the four states the dashboard tells apart:
//!   unreachable, handshake failed, auth rejected, reachable. It records the
//!   outcome on the row, so the status the server list shows survives a reload.
//! - `POST /api/v1/mcp-servers/{id}/tools/call` runs one tool with arguments an
//!   operator supplies and returns the raw JSON-RPC request and reply, the MCP
//!   equivalent of the Playground. A tool that ran and reported an error is a
//!   fifth state, `tool_errored`, because the server did answer.
//!
//! Both authenticate the way discovery does, so a server behind OAuth is spoken
//! to with the caller's own session and never anyone else's. Both need the
//! capability to update the server: a tool call has consequences, and the
//! operator is spending their own credential on it. Neither goes through the
//! gateway, so neither appears in the MCP tool-call log; the call is audited.

use std::time::Instant;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use rolter_store::postgres::repo::McpServerRepo;

use crate::crud::{invalid_field, log_audit, pool, ApiResult, SafeJson};
use crate::mcp_client::{Failure, McpClient};
use crate::mcp_discovery::{record_failed, record_reached, resolve_auth};
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// Largest reply returned whole. A bigger one is cut to a preview, since this
/// is a screen's worth of JSON for a person to read, not a data path.
const MAX_REPLY_BYTES: usize = 256 * 1024;

/// How much of an oversized reply is kept as the preview.
const PREVIEW_BYTES: usize = 32 * 1024;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/mcp-servers/{id}/test", post(test_connection))
        .route("/api/v1/mcp-servers/{id}/tools/call", post(call_tool))
}

/// What the server being unreachable or refusing is called to the dashboard,
/// plus the two reasons a probe could not be attempted at all.
fn outcome_of(failure: &Failure) -> &'static str {
    failure.kind.code()
}

#[derive(Debug, Serialize)]
struct ConnectionStatus {
    /// `reachable`, `unreachable`, `handshake_failed`, `auth_rejected`,
    /// `egress_denied`, or `needs_session` / `credential_unavailable` when
    /// there was nothing to authenticate with yet
    status: &'static str,
    /// a connection was made and something answered
    reachable: bool,
    handshake_completed: bool,
    protocol_version: Option<String>,
    server_name: Option<String>,
    server_version: Option<String>,
    latency_ms: u64,
    /// a secret-free sentence for a failure
    message: Option<String>,
    checked_at: DateTime<Utc>,
}

async fn test_connection(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ConnectionStatus>> {
    let repo = McpServerRepo(pool(&state));
    let server = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_server", Update),
    )
    .await?;
    let started = Instant::now();
    let checked_at = Utc::now();
    let status = match resolve_auth(&state, &server, user_of(&principal)).await {
        Err(code) => ConnectionStatus {
            status: code,
            reachable: false,
            handshake_completed: false,
            protocol_version: None,
            server_name: None,
            server_version: None,
            latency_ms: 0,
            message: Some(match code {
                "needs_session" => {
                    "the server uses OAuth and you have no live session on it; connect first"
                        .to_string()
                }
                _ => "the server's stored credential is not available to the control plane"
                    .to_string(),
            }),
            checked_at,
        },
        Ok(auth) => {
            match McpClient::connect(&state.egress, &server.url, auth, &server.transport).await {
                Ok(client) => {
                    record_reached(&repo, id, &client).await;
                    ConnectionStatus {
                        status: "reachable",
                        reachable: true,
                        handshake_completed: true,
                        protocol_version: Some(client.handshake.protocol_version.clone()),
                        server_name: client.handshake.server_name.clone(),
                        server_version: client.handshake.server_version.clone(),
                        latency_ms: elapsed_ms(started),
                        message: None,
                        checked_at,
                    }
                }
                Err(failure) => {
                    record_failed(&repo, id, failure.kind).await;
                    ConnectionStatus {
                        status: outcome_of(&failure),
                        reachable: failure.kind.reached_server(),
                        handshake_completed: false,
                        protocol_version: None,
                        server_name: None,
                        server_version: None,
                        latency_ms: elapsed_ms(started),
                        message: Some(failure.message),
                        checked_at,
                    }
                }
            }
        }
    };
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_server.test",
        "mcp_server",
        server.id,
        json!({"slug": server.slug, "status": status.status}),
    )
    .await;
    Ok(Json(status))
}

#[derive(Debug, Deserialize)]
struct CallToolInput {
    tool: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ToolCallResult {
    /// `success`, `tool_errored` (the server answered and the tool reported an
    /// error), or one of the failure states of [`ConnectionStatus`]
    outcome: &'static str,
    ok: bool,
    latency_ms: u64,
    /// the JSON-RPC request exactly as sent; absent when it was never sent
    request: Option<Value>,
    /// the JSON-RPC reply, or a preview of an oversized one
    response: Option<Value>,
    message: Option<String>,
}

async fn call_tool(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<CallToolInput>,
) -> ApiResult<Json<ToolCallResult>> {
    let repo = McpServerRepo(pool(&state));
    let server = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_server", Update),
    )
    .await?;
    // only what the server's manifest declares: the dashboard offers exactly
    // that list, and a free-text tool name would make this a general-purpose
    // JSON-RPC relay behind a credential the operator may not hold themselves
    if !server.tools.contains(&body.tool) {
        return Err(invalid_field(
            "tool",
            format!(
                "'{}' is not in the manifest of '{}'",
                body.tool, server.slug
            ),
        ));
    }
    let arguments = body.arguments.unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(invalid_field(
            "arguments",
            "arguments must be a JSON object",
        ));
    }
    let started = Instant::now();
    let result = match resolve_auth(&state, &server, user_of(&principal)).await {
        Err(code) => ToolCallResult {
            outcome: code,
            ok: false,
            latency_ms: 0,
            request: None,
            response: None,
            message: Some("there is nothing to authenticate the call with yet".to_string()),
        },
        Ok(auth) => {
            let attempt = async {
                let mut client =
                    McpClient::connect(&state.egress, &server.url, auth, &server.transport).await?;
                record_reached(&repo, id, &client).await;
                client.call_tool(&body.tool, arguments).await
            }
            .await;
            match attempt {
                Ok(exchange) => {
                    let errored = exchange.response.get("error").is_some()
                        || exchange
                            .response
                            .pointer("/result/isError")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    ToolCallResult {
                        outcome: if errored { "tool_errored" } else { "success" },
                        ok: !errored,
                        latency_ms: elapsed_ms(started),
                        request: Some(exchange.request),
                        response: Some(bounded(exchange.response)),
                        message: None,
                    }
                }
                Err(failure) => {
                    record_failed(&repo, id, failure.kind).await;
                    ToolCallResult {
                        outcome: outcome_of(&failure),
                        ok: false,
                        latency_ms: elapsed_ms(started),
                        request: None,
                        response: None,
                        message: Some(failure.message),
                    }
                }
            }
        }
    };
    // the arguments are not audited: they are whatever the operator typed and
    // can hold the very thing a tool exists to look up
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_server.tool_call",
        "mcp_server",
        server.id,
        json!({"slug": server.slug, "tool": body.tool, "outcome": result.outcome}),
    )
    .await;
    Ok(Json(result))
}

/// The account whose session a call may use: the signed-in user. The admin
/// token and open mode are nobody, so a server behind OAuth has no session for
/// them to speak with.
fn user_of(principal: &Principal) -> Option<Uuid> {
    principal.account_id()
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A reply as it is, or a marker with a preview when it is too large to show.
fn bounded(reply: Value) -> Value {
    let text = reply.to_string();
    if text.len() <= MAX_REPLY_BYTES {
        return reply;
    }
    let end = text.floor_char_boundary(PREVIEW_BYTES);
    json!({
        "truncated": true,
        "size_bytes": text.len(),
        "preview": &text[..end],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_reply_is_returned_whole() {
        let reply = json!({"result": {"content": []}});
        assert_eq!(bounded(reply.clone()), reply);
    }

    #[test]
    fn an_oversized_reply_becomes_a_marked_preview() {
        let reply = json!({"result": "é".repeat(MAX_REPLY_BYTES)});
        let shown = bounded(reply);
        assert_eq!(shown["truncated"], true);
        assert!(shown["size_bytes"].as_u64().unwrap() > MAX_REPLY_BYTES as u64);
        assert!(shown["preview"].as_str().unwrap().len() <= PREVIEW_BYTES);
    }
}
