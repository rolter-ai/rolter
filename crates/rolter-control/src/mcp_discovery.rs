//! Tool discovery for registered MCP servers (#1283).
//!
//! A server's tool manifest used to be whatever an operator typed, or the
//! curated catalog's hand-taken snapshot. This asks the server instead: a
//! `tools/list` call, made by the control plane through [`crate::mcp_client`],
//! whose answer is written back to the server row.
//!
//! The rules are the maintainer's decision on #1283:
//!
//! - **When.** Best effort when a server is installed, after its credential is
//!   set, and once an OAuth consent has produced a session; and on demand from
//!   the dashboard. Never blocking: the install returns first and a server that
//!   is down leaves a note on the row, not an error.
//! - **As whom.** A server with a static credential is asked with it. A server
//!   behind OAuth is asked with the caller's own live session, so discovery is
//!   per grant: nobody's token is borrowed, and with no session the row stays
//!   `pending` rather than guessing.
//! - **Reconciliation.** A tool the server no longer returns is kept and
//!   flagged in `tools_stale`, never pruned, so a tool group that names it still
//!   validates and an operator decides what to do about it.
//! - **Fallback.** The catalog's static lists only fill `tools` until the first
//!   discovery; the row says which by its `tools_discovery_status`.
//! - **Egress.** The call goes through the egress policy like every other
//!   operator-supplied address.

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use rolter_store::postgres::models::McpServer;
use rolter_store::postgres::repo::{McpContact, McpOAuthRepo, McpServerRepo, McpToolDiscovery};

use crate::crud::{log_audit, pool, ApiResult};
use crate::mcp_client::{ClientAuth, DiscoveredTool, McpClient};
use crate::mcp_oauth_flow::kek;
use crate::rbac::{authorize, Principal, ScopeChain};
use crate::rbac_matrix::cap;
use crate::ControlState;

/// Most tools one server may contribute. A server listing more is truncated,
/// which flags nothing: the cap exists so a hostile or broken server cannot
/// fill a row.
const MAX_TOOLS: usize = 1000;

/// Largest input schema kept for one tool, in serialized bytes.
const MAX_SCHEMA_BYTES: usize = 64 * 1024;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route("/api/v1/mcp-servers/{id}/discover", post(discover_now))
        .route("/api/v1/mcp-servers/{id}/tools", get(list_discovered))
}

/// Merge a fresh `tools/list` answer into a server's manifest.
///
/// Returns the new manifest and the names in it that the server did not return.
/// Discovered tools come first, in the server's order; a previously known tool
/// that is missing follows, in its old order, and is flagged. Names are
/// de-duplicated, so a server that lists one twice does not.
pub(crate) fn reconcile(
    previous: &[String],
    discovered: &[DiscoveredTool],
) -> (Vec<String>, Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let mut tools: Vec<String> = discovered
        .iter()
        .map(|tool| tool.name.clone())
        .filter(|name| seen.insert(name.clone()))
        .take(MAX_TOOLS)
        .collect();
    let mut stale = Vec::new();
    for name in previous {
        if seen.insert(name.clone()) {
            tools.push(name.clone());
            stale.push(name.clone());
        }
    }
    (tools, stale)
}

/// The schemas as stored: name, description and input schema per tool, with an
/// oversized schema replaced by a marker rather than dropped silently.
fn schemas_json(discovered: &[DiscoveredTool]) -> Value {
    Value::Array(
        discovered
            .iter()
            .take(MAX_TOOLS)
            .map(|tool| {
                let oversized = serde_json::to_string(&tool.input_schema)
                    .map(|text| text.len() > MAX_SCHEMA_BYTES)
                    .unwrap_or(true);
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": if oversized {
                        json!({"type": "object", "x-rolter-truncated": true})
                    } else {
                        tool.input_schema.clone()
                    },
                })
            })
            .collect(),
    )
}

/// How to authenticate a discovery call, or the code to record when it cannot
/// be done yet.
pub(crate) async fn resolve_auth(
    state: &ControlState,
    server: &McpServer,
    user: Option<Uuid>,
) -> Result<ClientAuth, &'static str> {
    let repo = McpServerRepo(pool(state));
    match server.auth_kind.as_str() {
        "none" => Ok(ClientAuth::None),
        "bearer" | "header" => {
            let kek = kek().map_err(|_| "credential_unavailable")?;
            let credential = repo
                .credential(&kek, server.id)
                .await
                .map_err(|_| "credential_unavailable")?
                .ok_or("credential_unavailable")?;
            if server.auth_kind == "bearer" {
                return Ok(ClientAuth::Bearer(credential));
            }
            let name = server
                .auth_header_name
                .clone()
                .ok_or("credential_unavailable")?;
            Ok(ClientAuth::Header {
                name,
                value: credential,
            })
        }
        "oauth" => {
            let user = user.ok_or("needs_session")?;
            let token = live_session_token(state, server, user)
                .await
                .ok_or("needs_session")?;
            Ok(ClientAuth::Bearer(token))
        }
        _ => Err("credential_unavailable"),
    }
}

/// The access token of `user`'s newest live session on `server`, if any.
async fn live_session_token(
    state: &ControlState,
    server: &McpServer,
    user: Uuid,
) -> Option<String> {
    let kek = kek().ok()?;
    let repo = McpOAuthRepo(pool(state));
    let sessions = repo.list_sessions(server.org_id, Some(user)).await.ok()?;
    let now = Utc::now();
    for session in sessions {
        if session.revoked_at.is_some() || session.expires_at <= now {
            continue;
        }
        let Some(context) = repo.session_context(session.id).await.ok().flatten() else {
            continue;
        };
        if context.server_id != server.id {
            continue;
        }
        if let Ok(Some(tokens)) = repo.open_session(&kek, session.id, now).await {
            return Some(tokens.access_token);
        }
    }
    None
}

/// Ask the server for its tools and record the answer. Never fails the caller
/// for an upstream problem: that is a note on the returned row.
pub(crate) async fn discover(
    state: &ControlState,
    server_id: Uuid,
    user: Option<Uuid>,
) -> ApiResult<McpServer> {
    let repo = McpServerRepo(pool(state));
    let server = repo.get(server_id).await?;
    let auth = match resolve_auth(state, &server, user).await {
        Ok(auth) => auth,
        Err(code) => return Ok(repo.record_tool_discovery_failure(server_id, code).await?),
    };
    let listed = async {
        let mut client =
            McpClient::connect(&state.egress, &server.url, auth, &server.transport).await?;
        // the handshake completing is contact whether or not the list follows
        record_reached(&repo, server_id, &client).await;
        client.list_tools().await
    }
    .await;
    match listed {
        Ok(tools) => {
            let (manifest, stale) = reconcile(&server.tools, &tools);
            let schemas = schemas_json(&tools);
            Ok(repo
                .record_tool_discovery(
                    server_id,
                    McpToolDiscovery {
                        tools: &manifest,
                        stale: &stale,
                        schemas: &schemas,
                    },
                )
                .await?)
        }
        Err(failure) => {
            record_failed(&repo, server_id, failure.kind).await;
            // the sentence is secret-free by construction, so it may be logged
            tracing::debug!(
                %server_id,
                kind = failure.kind.code(),
                reason = %failure.message,
                "MCP tool discovery got no answer"
            );
            Ok(repo
                .record_tool_discovery_failure(server_id, failure.kind.code())
                .await?)
        }
    }
}

/// Note on the row that the handshake completed. Bookkeeping: a failure to
/// write it is not worth failing the call it describes.
pub(crate) async fn record_reached(repo: &McpServerRepo<'_>, id: Uuid, client: &McpClient) {
    let handshake = &client.handshake;
    let _ = repo
        .record_contact(
            id,
            McpContact::Reached {
                protocol_version: &handshake.protocol_version,
                server_name: handshake.server_name.as_deref(),
            },
        )
        .await;
}

/// Note on the row why contact failed. `FailureKind` only reaches the column's
/// allowed values, which is what the check constraint holds it to.
pub(crate) async fn record_failed(
    repo: &McpServerRepo<'_>,
    id: Uuid,
    kind: crate::mcp_client::FailureKind,
) {
    let _ = repo
        .record_contact(id, McpContact::Failed { kind: kind.code() })
        .await;
}

/// Run discovery in the background for a server that was just installed or
/// authenticated, so the request that caused it returns first. A panic or an
/// error here is only a missing note on the row.
pub(crate) fn discover_in_background(state: &ControlState, server_id: Uuid, user: Option<Uuid>) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = discover(&state, server_id, user).await {
            tracing::debug!(%server_id, ?error, "background MCP tool discovery was not recorded");
        }
    });
}

async fn discover_now(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<McpServer>> {
    let server = McpServerRepo(pool(&state)).get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_server", Update),
    )
    .await?;
    let user = match &principal {
        Principal::User(user) => Some(user.id),
        // neither holds a person's own grant to the server
        Principal::Superadmin { .. } | Principal::Token(_) => None,
    };
    let server = discover(&state, id, user).await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_server.discover",
        "mcp_server",
        server.id,
        json!({
            "slug": server.slug,
            "status": server.tools_discovery_status,
            "error": server.tools_discovery_error,
        }),
    )
    .await;
    Ok(Json(server))
}

#[derive(Debug, Serialize)]
struct ToolView {
    name: String,
    description: String,
    input_schema: Value,
    /// the last discovery did not return it; kept so a group naming it still
    /// validates
    stale: bool,
}

#[derive(Debug, Serialize)]
struct DiscoveredTools {
    status: String,
    error: Option<String>,
    discovered_at: Option<chrono::DateTime<Utc>>,
    tools: Vec<ToolView>,
}

/// The server's manifest with the schemas discovery saw. A tool in the manifest
/// with no stored schema (the catalog's fallback list, or one kept after it
/// vanished) is listed with an empty one rather than omitted.
async fn list_discovered(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<DiscoveredTools>> {
    let repo = McpServerRepo(pool(&state));
    let server = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_server", Read),
    )
    .await?;
    let schemas = repo.tool_schemas(id).await?;
    let by_name: std::collections::HashMap<&str, &Value> = schemas
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| Some((item.get("name")?.as_str()?, item)))
                .collect()
        })
        .unwrap_or_default();
    let tools = server
        .tools
        .iter()
        .map(|name| {
            let stored = by_name.get(name.as_str());
            ToolView {
                name: name.clone(),
                description: stored
                    .and_then(|item| item.get("description"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input_schema: stored
                    .and_then(|item| item.get("input_schema"))
                    .cloned()
                    .unwrap_or(Value::Null),
                stale: server.tools_stale.contains(name),
            }
        })
        .collect();
    Ok(Json(DiscoveredTools {
        status: server.tools_discovery_status,
        error: server.tools_discovery_error,
        discovered_at: server.tools_discovered_at,
        tools,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> DiscoveredTool {
        DiscoveredTool {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn a_first_discovery_replaces_the_fallback_and_flags_what_the_server_lacks() {
        let (tools, stale) = reconcile(&names(&["a", "gone"]), &[tool("a"), tool("b")]);
        assert_eq!(tools, names(&["a", "b", "gone"]));
        assert_eq!(stale, names(&["gone"]));
    }

    #[test]
    fn a_tool_that_comes_back_is_no_longer_flagged() {
        let (tools, stale) = reconcile(&names(&["a", "gone"]), &[tool("a"), tool("gone")]);
        assert_eq!(tools, names(&["a", "gone"]));
        assert!(stale.is_empty());
    }

    #[test]
    fn an_empty_answer_keeps_every_known_tool_flagged() {
        let (tools, stale) = reconcile(&names(&["a", "b"]), &[]);
        assert_eq!(tools, names(&["a", "b"]));
        assert_eq!(stale, names(&["a", "b"]));
    }

    #[test]
    fn a_server_that_lists_a_tool_twice_contributes_it_once() {
        let (tools, stale) = reconcile(&[], &[tool("a"), tool("a")]);
        assert_eq!(tools, names(&["a"]));
        assert!(stale.is_empty());
    }

    #[test]
    fn an_oversized_schema_is_marked_not_stored() {
        let mut big = tool("big");
        big.input_schema = json!({"description": "x".repeat(MAX_SCHEMA_BYTES + 1)});
        let schemas = schemas_json(&[big, tool("small")]);
        assert_eq!(schemas[0]["input_schema"]["x-rolter-truncated"], true);
        assert_eq!(schemas[1]["input_schema"]["type"], "object");
    }
}
