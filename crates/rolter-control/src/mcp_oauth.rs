//! MCP servers, OAuth consent grants and token sessions (#541).
//!
//! Three rules shape this module:
//!
//! 1. **No token material crosses the API boundary.** Access and refresh
//!    tokens are sealed with the deployment KEK in [`rolter_store`] and are
//!    only readable through `McpOAuthRepo::open_session`, which nothing here
//!    calls. Responses carry metadata and a `has_refresh_token` flag.
//! 2. **A grant is the unit of consent.** Revoking one revokes every session
//!    under it in the same transaction, so withdrawn consent cannot leave a
//!    live token behind.
//! 3. **A user sees their own.** Org admins see every grant and session in
//!    the org; a member or viewer sees only the ones they own, and may revoke
//!    only those — cross-tenant reads are impossible because every listing is
//!    joined through the org.
//!
//! Transport and the authorization-code/OBO exchange live with the MCP proxy
//! (#423); this module owns persistence, listing, revocation and audit.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use rolter_core::Error;
// the one authoritative MCP transport list; the check constraints and the
// dashboard dropdown match it, pinned by `transports_match_the_schema` (#783)
use rolter_core::MCP_TRANSPORTS as TRANSPORTS;
use rolter_store::postgres::models::{
    McpGatewaySettings, McpOAuthGrant, McpOAuthSession, McpServer, McpToolGroup,
};
use rolter_store::postgres::repo::{
    McpAuthConfig, McpGatewaySettingsRepo, McpGatewaySettingsUpdate, McpOAuthRepo, McpServerRepo,
    McpServerUpdate, McpToolGroupRepo, NewMcpServer,
};

use crate::crud::{
    log_audit, pool, publish_config_change, require_non_empty, ApiError, ApiResult, SafeJson,
};
use crate::mcp_oauth_flow::kek;
use crate::rbac::{authorize, holds_admin, Principal, ScopeChain};
use crate::rbac_matrix::{cap, Requirement};
use crate::ControlState;

pub(crate) fn router() -> Router<ControlState> {
    Router::new()
        .route(
            "/api/v1/orgs/{org_id}/mcp-servers",
            get(list_servers).post(create_server),
        )
        .route(
            "/api/v1/mcp-servers/{id}",
            delete(delete_server).patch(update_server),
        )
        .route(
            "/api/v1/mcp-servers/{id}/auth",
            axum::routing::put(set_auth),
        )
        .route("/api/v1/orgs/{org_id}/mcp/grants", get(list_grants))
        .route("/api/v1/mcp/grants/{id}", delete(revoke_grant))
        .route("/api/v1/orgs/{org_id}/mcp/sessions", get(list_sessions))
        .route("/api/v1/mcp/sessions/{id}", delete(revoke_session))
        .route("/api/v1/orgs/{org_id}/mcp/library", get(list_library))
        .route(
            "/api/v1/orgs/{org_id}/mcp/tool-groups",
            get(list_tool_groups).post(create_tool_group),
        )
        .route(
            "/api/v1/mcp/tool-groups/{id}",
            delete(delete_tool_group).put(update_tool_group),
        )
        .route(
            "/api/v1/orgs/{org_id}/mcp/settings",
            get(get_mcp_settings).put(update_mcp_settings),
        )
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Core(Error::Config(message.into()))
}

fn validate_required_scopes(scopes: &[String]) -> ApiResult<()> {
    if scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(invalid("required_scopes must not contain empty values"));
    }
    let unique_scopes: std::collections::HashSet<_> = scopes.iter().collect();
    if unique_scopes.len() != scopes.len() {
        return Err(invalid("required_scopes must not contain duplicates"));
    }
    Ok(())
}

/// Whether the caller holds org admin. Used to decide between "everything in
/// the org" and "only what I own"; a caller who is neither is rejected by the
/// viewer check that runs first. A question, not a guard — the guard is the
/// capability the handler names.
async fn is_org_admin(
    state: &ControlState,
    principal: &Principal,
    org_id: Uuid,
) -> ApiResult<bool> {
    holds_admin(state, principal, ScopeChain::org(org_id)).await
}

/// The owner filter for a listing: `None` means "no filter" (org admins),
/// `Some(user)` restricts to the caller's own rows.
async fn owner_filter(
    state: &ControlState,
    principal: &Principal,
    org_id: Uuid,
    requirement: Requirement,
) -> ApiResult<Option<Uuid>> {
    // any membership at the org is required before anything is listed
    authorize(state, principal, ScopeChain::org(org_id), requirement).await?;
    if is_org_admin(state, principal, org_id).await? {
        return Ok(None);
    }
    Ok(match principal {
        Principal::Superadmin => None,
        Principal::User(user) => Some(user.id),
    })
}

async fn list_servers(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<McpServer>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_server", Read),
    )
    .await?;
    Ok(Json(McpServerRepo(pool(&state)).list(org_id).await?))
}

#[derive(Debug, Deserialize)]
struct CreateMcpServer {
    name: String,
    slug: String,
    url: String,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    required_scopes: Vec<String>,
    #[serde(default)]
    description: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default = "default_custom_source")]
    source: String,
}

fn default_true() -> bool {
    true
}

fn default_custom_source() -> String {
    "custom".to_string()
}

fn validate_tools(tools: &[String]) -> ApiResult<()> {
    if tools.iter().any(|tool| tool.trim().is_empty()) {
        return Err(invalid("tools must not contain empty names"));
    }
    let unique: std::collections::HashSet<_> = tools.iter().collect();
    if unique.len() != tools.len() {
        return Err(invalid("tools must not contain duplicates"));
    }
    Ok(())
}

async fn create_server(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<CreateMcpServer>,
) -> ApiResult<Json<McpServer>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_server", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    require_non_empty(&body.url, "url")?;
    let transport = body.transport.as_deref().unwrap_or("streamable_http");
    if !TRANSPORTS.contains(&transport) {
        return Err(invalid(format!(
            "transport must be one of {}",
            TRANSPORTS.join(", ")
        )));
    }
    // an MCP server is a URL the deployment will be asked to call on a user's
    // behalf, so it is held to the same egress rules as an upstream provider
    if !(body.url.starts_with("http://") || body.url.starts_with("https://")) {
        return Err(invalid("url must be an http(s) endpoint"));
    }
    state
        .egress
        .check_url(&body.url, "MCP server URL")
        .map_err(invalid)?;
    validate_required_scopes(&body.required_scopes)?;
    validate_tools(&body.tools)?;
    if !matches!(body.source.as_str(), "custom" | "library") {
        return Err(invalid("source must be custom or library"));
    }
    if body.source == "library"
        && !matches_catalog(
            &body.slug,
            &body.name,
            &body.url,
            transport,
            &body.tools,
            &body.required_scopes,
        )
    {
        return Err(invalid(
            "library server definition does not match the curated catalog",
        ));
    }
    let server = McpServerRepo(pool(&state))
        .create(NewMcpServer {
            org_id,
            name: &body.name,
            slug: &body.slug,
            url: &body.url,
            transport,
            description: &body.description,
            enabled: body.enabled,
            tools: &body.tools,
            source: &body.source,
            required_scopes: &body.required_scopes,
        })
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "mcp_server.create",
        "mcp_server",
        server.id,
        serde_json::json!({"slug": server.slug, "transport": server.transport}),
    )
    .await;
    Ok(Json(server))
}

#[derive(Debug, Deserialize)]
struct UpdateMcpServer {
    name: Option<String>,
    url: Option<String>,
    transport: Option<String>,
    description: Option<String>,
    enabled: Option<bool>,
    tools: Option<Vec<String>>,
    required_scopes: Option<Vec<String>>,
    // doubly optional so a PATCH can tell "leave it" (absent) from "stop
    // overriding, inherit the org default again" (explicit null); a plain
    // Option would make the second unsayable
    #[serde(default, deserialize_with = "explicit_null")]
    connect_timeout_ms: Option<Option<i32>>,
    #[serde(default, deserialize_with = "explicit_null")]
    request_timeout_ms: Option<Option<i32>>,
    #[serde(default, deserialize_with = "explicit_null")]
    max_retries: Option<Option<i32>>,
}

/// Deserialize a present-but-null field as `Some(None)` rather than `None`.
///
/// serde collapses both "absent" and "null" to `None` for an `Option<Option<T>>`,
/// which for a PATCH silently turns "clear this override" into "leave it
/// alone" — the field is only reached when the key is present, so wrapping in
/// `Some` here is what makes the two distinguishable.
fn explicit_null<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::deserialize(deserializer).map(Some)
}

/// A per-server override and the range the org-wide setting already uses.
/// Checked here so a bad value is a 400 naming the bound rather than a 500
/// from the check constraint.
fn validate_override(value: Option<i32>, field: &str, lo: i32, hi: i32) -> ApiResult<()> {
    match value {
        Some(value) if !(lo..=hi).contains(&value) => Err(invalid(format!(
            "{field} must be between {lo} and {hi}, or null to inherit the org default"
        ))),
        _ => Ok(()),
    }
}

async fn update_server(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateMcpServer>,
) -> ApiResult<Json<McpServer>> {
    let repo = McpServerRepo(pool(&state));
    let current = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(current.org_id),
        cap!("mcp_server", Update),
    )
    .await?;
    let name = body.name.as_deref().unwrap_or(&current.name);
    let url = body.url.as_deref().unwrap_or(&current.url);
    let transport = body.transport.as_deref().unwrap_or(&current.transport);
    let description = body.description.as_deref().unwrap_or(&current.description);
    let enabled = body.enabled.unwrap_or(current.enabled);
    let tools = body.tools.as_deref().unwrap_or(&current.tools);
    let required_scopes = body
        .required_scopes
        .as_deref()
        .unwrap_or(&current.required_scopes);
    require_non_empty(name, "name")?;
    require_non_empty(url, "url")?;
    if !TRANSPORTS.contains(&transport) {
        return Err(invalid(format!(
            "transport must be one of {}",
            TRANSPORTS.join(", ")
        )));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(invalid("url must be an http(s) endpoint"));
    }
    state
        .egress
        .check_url(url, "MCP server URL")
        .map_err(invalid)?;
    validate_required_scopes(required_scopes)?;
    validate_tools(tools)?;
    let connect_timeout_ms = body
        .connect_timeout_ms
        .unwrap_or(current.connect_timeout_ms);
    let request_timeout_ms = body
        .request_timeout_ms
        .unwrap_or(current.request_timeout_ms);
    let max_retries = body.max_retries.unwrap_or(current.max_retries);
    validate_override(connect_timeout_ms, "connect_timeout_ms", 100, 60_000)?;
    validate_override(request_timeout_ms, "request_timeout_ms", 1_000, 300_000)?;
    validate_override(max_retries, "max_retries", 0, 5)?;
    let server = repo
        .update(
            id,
            McpServerUpdate {
                name,
                url,
                transport,
                description,
                enabled,
                tools,
                required_scopes,
                connect_timeout_ms,
                request_timeout_ms,
                max_retries,
            },
        )
        .await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_server.update",
        "mcp_server",
        server.id,
        serde_json::json!({"required_scopes": server.required_scopes}),
    )
    .await;
    Ok(Json(server))
}

/// How rolter should authenticate to a server.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetMcpAuth {
    /// `none` | `bearer` | `header` | `oauth`
    auth_kind: String,
    /// required for `header`, refused otherwise
    auth_header_name: Option<String>,
    /// absent leaves the stored credential, `""` clears it, anything else
    /// replaces it — so an operator renaming a header need not re-type a
    /// secret they cannot read back
    credential: Option<String>,
}

/// Header names rolter refuses to carry an api key in, mirroring the
/// `mcp_servers_auth_header_name_shape` constraint. `authorization` is the
/// important one: allowing it would let header mode forge the bearer path.
const RESERVED_AUTH_HEADERS: [&str; 10] = [
    "authorization",
    "host",
    "content-length",
    "content-type",
    "connection",
    "transfer-encoding",
    "upgrade",
    "te",
    "trailer",
    "proxy-authorization",
];

fn validate_header_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(invalid(
            "auth_header_name must be between 1 and 64 characters",
        ));
    }
    // RFC 9110 field-name: a token, so no spaces, colons or separators
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&\'*+.^_`|~-".contains(&b))
    {
        return Err(invalid("auth_header_name must be a valid HTTP field name"));
    }
    if RESERVED_AUTH_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(invalid(format!(
            "auth_header_name must not be '{name}': rolter sets that header itself"
        )));
    }
    Ok(())
}

/// Set a server's authentication. Separate from `PATCH /mcp-servers/{id}`
/// because it is the only part of a server that takes a secret, and keeping it
/// on its own route means the general edit path never has a credential in its
/// body to leak into a log or an audit entry.
async fn set_auth(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<SetMcpAuth>,
) -> ApiResult<Json<McpServer>> {
    let repo = McpServerRepo(pool(&state));
    let current = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(current.org_id),
        cap!("mcp_server", Update),
    )
    .await?;
    if !matches!(
        body.auth_kind.as_str(),
        "none" | "bearer" | "header" | "oauth"
    ) {
        return Err(invalid("auth_kind must be none, bearer, header or oauth"));
    }
    let carries_credential = matches!(body.auth_kind.as_str(), "bearer" | "header");
    match (body.auth_kind.as_str(), body.auth_header_name.as_deref()) {
        ("header", None) => {
            return Err(invalid(
                "auth_kind 'header' requires auth_header_name, e.g. X-Api-Key",
            ))
        }
        ("header", Some(name)) => validate_header_name(name)?,
        (_, Some(_)) => {
            return Err(invalid(
                "auth_header_name is only meaningful for auth_kind 'header'",
            ))
        }
        (_, None) => {}
    }
    if !carries_credential && body.credential.as_deref().is_some_and(|c| !c.is_empty()) {
        return Err(invalid(format!(
            "auth_kind '{}' carries no credential",
            body.auth_kind
        )));
    }
    // a kind that needs one and has none stored would be refused by the check
    // constraint as a 500; say so as a 400 instead
    if carries_credential
        && !current.has_credential
        && body.credential.as_deref().unwrap_or("").is_empty()
    {
        return Err(invalid(format!(
            "auth_kind '{}' requires a credential",
            body.auth_kind
        )));
    }
    let server = repo
        .set_auth(
            &kek()?,
            id,
            McpAuthConfig {
                auth_kind: &body.auth_kind,
                auth_header_name: body.auth_header_name.as_deref(),
                credential: body.credential.as_deref(),
            },
        )
        .await?;
    publish_config_change(&state).await?;
    // the credential is deliberately not in the audit payload; what changed is
    // which mechanism is in use, which is the reviewable fact
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_server.set_auth",
        "mcp_server",
        server.id,
        serde_json::json!({
            "auth_kind": server.auth_kind,
            "auth_header_name": server.auth_header_name,
            "has_credential": server.has_credential,
        }),
    )
    .await;
    Ok(Json(server))
}

async fn delete_server(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = McpServerRepo(pool(&state));
    let server = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(server.org_id),
        cap!("mcp_server", Delete),
    )
    .await?;
    repo.delete(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_server.delete",
        "mcp_server",
        id,
        serde_json::json!({"slug": server.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Clone, Serialize)]
struct LibraryItem {
    slug: &'static str,
    name: &'static str,
    description: &'static str,
    url: &'static str,
    transport: &'static str,
    tools: &'static [&'static str],
    required_scopes: &'static [&'static str],
    installed: bool,
}

struct LibraryDefinition {
    slug: &'static str,
    name: &'static str,
    description: &'static str,
    url: &'static str,
    transport: &'static str,
    tools: &'static [&'static str],
    required_scopes: &'static [&'static str],
}

/// The curated catalog.
///
/// Every entry is a **snapshot** of what the upstream server published, taken
/// by hand. It is not discovered: nothing here is re-checked against the live
/// server, so a tool renamed upstream goes stale here until someone edits this
/// list. That is a deliberate, cheap first step — an empty list was worse,
/// because it made a card claim the server declares no tools at all, left the
/// installed server contributing nothing to the tool tally, and made the entry
/// unpickable in a tool group (#1252). Replacing this with a `tools/list` call
/// on install is tracked separately.
///
/// The install path in [`create_server`] compares a `library` request against
/// these lists element for element, so filling them in here is what makes an
/// installed curated server arrive with its manifest — a client that sends a
/// different list is rejected as not matching the catalog.
///
/// `required_scopes` is what the OAuth authorization request must ask for.
/// Notion's remote server has none: its consent screen grants access to the
/// pages the user picks rather than to named scopes, so an empty list there is
/// the accurate answer, not a gap.
const LIBRARY: &[LibraryDefinition] = &[
    LibraryDefinition {
        slug: "github",
        name: "GitHub",
        description: "Repository, issue, pull request, and code search tools.",
        url: "https://api.githubcopilot.com/mcp/",
        transport: "streamable_http",
        tools: &[
            "get_me",
            "search_repositories",
            "search_code",
            "search_issues",
            "get_file_contents",
            "create_or_update_file",
            "push_files",
            "create_branch",
            "list_commits",
            "get_commit",
            "list_issues",
            "get_issue",
            "create_issue",
            "update_issue",
            "add_issue_comment",
            "get_issue_comments",
            "list_pull_requests",
            "get_pull_request",
            "get_pull_request_files",
            "get_pull_request_diff",
            "create_pull_request",
            "update_pull_request",
            "create_pull_request_review",
            "merge_pull_request",
            "list_workflow_runs",
            "get_workflow_run",
            "list_code_scanning_alerts",
            "get_code_scanning_alert",
            "list_notifications",
        ],
        required_scopes: &[
            "repo",
            "read:org",
            "read:user",
            "workflow",
            "security_events",
        ],
    },
    LibraryDefinition {
        slug: "sentry",
        name: "Sentry",
        description: "Inspect production issues, traces, and event context.",
        url: "https://mcp.sentry.dev/mcp",
        transport: "streamable_http",
        tools: &[
            "whoami",
            "find_organizations",
            "find_teams",
            "find_projects",
            "find_releases",
            "find_issues",
            "find_errors",
            "find_transactions",
            "get_issue_details",
            "get_trace_details",
            "get_event_attachment",
            "update_issue",
            "search_docs",
            "get_doc",
        ],
        required_scopes: &["org:read", "project:read", "team:read", "event:read"],
    },
    LibraryDefinition {
        slug: "notion",
        name: "Notion",
        description: "Search and update pages in an organization workspace.",
        url: "https://mcp.notion.com/mcp",
        transport: "streamable_http",
        tools: &[
            "search",
            "fetch",
            "notion-create-pages",
            "notion-update-page",
            "notion-move-pages",
            "notion-duplicate-page",
            "notion-create-database",
            "notion-update-database",
            "notion-create-comment",
            "notion-get-comments",
            "notion-get-self",
            "notion-get-user",
            "notion-get-users",
        ],
        // the Notion consent screen grants the pages the user selects, not
        // named scopes, so there is nothing to request here
        required_scopes: &[],
    },
    LibraryDefinition {
        slug: "linear",
        name: "Linear",
        description: "Browse and manage teams, issues, and projects.",
        url: "https://mcp.linear.app/mcp",
        transport: "streamable_http",
        tools: &[
            "list_teams",
            "get_team",
            "list_users",
            "get_user",
            "list_issues",
            "get_issue",
            "create_issue",
            "update_issue",
            "list_my_issues",
            "list_issue_statuses",
            "get_issue_status",
            "list_issue_labels",
            "create_issue_label",
            "list_comments",
            "create_comment",
            "list_projects",
            "get_project",
            "create_project",
            "update_project",
            "list_project_labels",
            "list_cycles",
            "list_documents",
            "get_document",
            "search_documentation",
        ],
        required_scopes: &["read", "write", "issues:create", "comments:create"],
    },
];

/// Whether a `library` install request reproduces a curated entry exactly.
///
/// The lists are compared element for element, order included: a caller may
/// install what the catalog publishes, not an edited subset of it. That is what
/// makes [`LIBRARY`]'s tool manifest reach the stored server — before #1252 the
/// catalog's lists were empty, so the only request that could pass this check
/// was one that also declared nothing.
fn matches_catalog(
    slug: &str,
    name: &str,
    url: &str,
    transport: &str,
    tools: &[String],
    required_scopes: &[String],
) -> bool {
    LIBRARY.iter().any(|item| {
        item.slug == slug
            && item.name == name
            && item.url == url
            && item.transport == transport
            && tools
                .iter()
                .map(String::as_str)
                .eq(item.tools.iter().copied())
            && required_scopes
                .iter()
                .map(String::as_str)
                .eq(item.required_scopes.iter().copied())
    })
}

async fn list_library(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<LibraryItem>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_server", Read),
    )
    .await?;
    let installed: std::collections::HashSet<_> = McpServerRepo(pool(&state))
        .list(org_id)
        .await?
        .into_iter()
        .map(|server| server.slug)
        .collect();
    Ok(Json(
        LIBRARY
            .iter()
            .map(|item| LibraryItem {
                slug: item.slug,
                name: item.name,
                description: item.description,
                url: item.url,
                transport: item.transport,
                tools: item.tools,
                required_scopes: item.required_scopes,
                installed: installed.contains(item.slug),
            })
            .collect(),
    ))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ToolRef {
    server_id: Uuid,
    tool: String,
}

#[derive(Debug, Deserialize)]
struct ToolGroupInput {
    name: String,
    slug: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    tools: Vec<ToolRef>,
}

fn validate_tool_refs(tools: &[ToolRef]) -> ApiResult<()> {
    if tools.iter().any(|item| item.tool.trim().is_empty()) {
        return Err(invalid("tool group references must name a tool"));
    }
    let unique: std::collections::HashSet<_> = tools
        .iter()
        .map(|item| (item.server_id, item.tool.as_str()))
        .collect();
    if unique.len() != tools.len() {
        return Err(invalid("tool group references must be unique"));
    }
    Ok(())
}

async fn validate_tool_refs_for_org(
    state: &ControlState,
    org_id: Uuid,
    tools: &[ToolRef],
) -> ApiResult<()> {
    validate_tool_refs(tools)?;
    let servers = McpServerRepo(pool(state)).list(org_id).await?;
    for item in tools {
        let server = servers
            .iter()
            .find(|server| server.id == item.server_id)
            .ok_or_else(|| invalid("tool group references a server outside this organization"))?;
        if !server.tools.contains(&item.tool) {
            return Err(invalid(format!(
                "tool '{}' is not declared by server '{}'",
                item.tool, server.slug
            )));
        }
    }
    Ok(())
}

async fn list_tool_groups(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<McpToolGroup>>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_tool_group", Read),
    )
    .await?;
    Ok(Json(McpToolGroupRepo(pool(&state)).list(org_id).await?))
}

async fn create_tool_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<ToolGroupInput>,
) -> ApiResult<Json<McpToolGroup>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_tool_group", Create),
    )
    .await?;
    require_non_empty(&body.name, "name")?;
    require_non_empty(&body.slug, "slug")?;
    validate_tool_refs_for_org(&state, org_id, &body.tools).await?;
    let tools = serde_json::to_value(&body.tools).map_err(|error| invalid(error.to_string()))?;
    let group = McpToolGroupRepo(pool(&state))
        .create(
            org_id,
            &body.name,
            &body.slug,
            &body.description,
            body.enabled,
            &tools,
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "mcp_tool_group.create",
        "mcp_tool_group",
        group.id,
        serde_json::json!({"slug": group.slug}),
    )
    .await;
    Ok(Json(group))
}

#[derive(Debug, Deserialize)]
struct UpdateToolGroup {
    name: String,
    #[serde(default)]
    description: String,
    enabled: bool,
    #[serde(default)]
    tools: Vec<ToolRef>,
}

async fn update_tool_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
    SafeJson(body): SafeJson<UpdateToolGroup>,
) -> ApiResult<Json<McpToolGroup>> {
    require_non_empty(&body.name, "name")?;
    let repo = McpToolGroupRepo(pool(&state));
    let current = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(current.org_id),
        cap!("mcp_tool_group", Update),
    )
    .await?;
    validate_tool_refs_for_org(&state, current.org_id, &body.tools).await?;
    let tools = serde_json::to_value(&body.tools).map_err(|error| invalid(error.to_string()))?;
    let group = repo
        .update(id, &body.name, &body.description, body.enabled, &tools)
        .await?;
    log_audit(
        &state,
        &principal,
        Some(group.org_id),
        "mcp_tool_group.update",
        "mcp_tool_group",
        group.id,
        serde_json::json!({"tools": body.tools.len()}),
    )
    .await;
    Ok(Json(group))
}

async fn delete_tool_group(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let repo = McpToolGroupRepo(pool(&state));
    let group = repo.get(id).await?;
    authorize(
        &state,
        &principal,
        ScopeChain::org(group.org_id),
        cap!("mcp_tool_group", Delete),
    )
    .await?;
    repo.delete(id).await?;
    log_audit(
        &state,
        &principal,
        Some(group.org_id),
        "mcp_tool_group.delete",
        "mcp_tool_group",
        id,
        serde_json::json!({"slug": group.slug}),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_mcp_settings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<McpGatewaySettings>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_settings", Read),
    )
    .await?;
    Ok(Json(
        McpGatewaySettingsRepo(pool(&state)).get(org_id).await?,
    ))
}

#[derive(Debug, Deserialize)]
struct McpSettingsInput {
    default_transport: String,
    connect_timeout_ms: i32,
    request_timeout_ms: i32,
    max_retries: i32,
    default_failure_mode: String,
    allow_unlisted_tools: bool,
}

async fn update_mcp_settings(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
    SafeJson(body): SafeJson<McpSettingsInput>,
) -> ApiResult<Json<McpGatewaySettings>> {
    authorize(
        &state,
        &principal,
        ScopeChain::org(org_id),
        cap!("mcp_settings", Update),
    )
    .await?;
    if !matches!(
        body.default_transport.as_str(),
        "sse" | "streamable_http" | "websocket"
    ) {
        return Err(invalid(
            "default_transport must be sse, streamable_http, or websocket",
        ));
    }
    if !(100..=60_000).contains(&body.connect_timeout_ms) {
        return Err(invalid("connect_timeout_ms must be between 100 and 60000"));
    }
    if !(1_000..=300_000).contains(&body.request_timeout_ms) {
        return Err(invalid(
            "request_timeout_ms must be between 1000 and 300000",
        ));
    }
    if !(0..=5).contains(&body.max_retries) {
        return Err(invalid("max_retries must be between 0 and 5"));
    }
    if !matches!(
        body.default_failure_mode.as_str(),
        "fail_open" | "fail_closed"
    ) {
        return Err(invalid(
            "default_failure_mode must be fail_open or fail_closed",
        ));
    }
    let settings = McpGatewaySettingsRepo(pool(&state))
        .update(
            org_id,
            McpGatewaySettingsUpdate {
                default_transport: &body.default_transport,
                connect_timeout_ms: body.connect_timeout_ms,
                request_timeout_ms: body.request_timeout_ms,
                max_retries: body.max_retries,
                default_failure_mode: &body.default_failure_mode,
                allow_unlisted_tools: body.allow_unlisted_tools,
            },
        )
        .await?;
    log_audit(
        &state,
        &principal,
        Some(org_id),
        "mcp_settings.update",
        "mcp_settings",
        org_id,
        serde_json::json!({"default_transport": settings.default_transport}),
    )
    .await;
    Ok(Json(settings))
}

/// A grant with its live/dead state resolved, so a client never has to infer
/// it from a nullable timestamp.
#[derive(Debug, Serialize)]
struct GrantView {
    #[serde(flatten)]
    grant: McpOAuthGrant,
    active: bool,
}

impl From<McpOAuthGrant> for GrantView {
    fn from(grant: McpOAuthGrant) -> Self {
        let active = grant.revoked_at.is_none();
        Self { grant, active }
    }
}

async fn list_grants(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<GrantView>>> {
    let owner = owner_filter(&state, &principal, org_id, cap!("mcp_oauth_grant", Read)).await?;
    Ok(Json(
        McpOAuthRepo(pool(&state))
            .list_grants(org_id, owner)
            .await?
            .into_iter()
            .map(GrantView::from)
            .collect(),
    ))
}

/// Whether the caller may revoke something owned by `owner_id` in `org_id`:
/// an org admin, or the owner themself. Anyone else gets `403` — including a
/// member of a *different* org, whose authorize check fails first.
async fn may_revoke(
    state: &ControlState,
    principal: &Principal,
    org_id: Uuid,
    owner_id: Uuid,
    requirement: Requirement,
) -> ApiResult<()> {
    if is_org_admin(state, principal, org_id).await? {
        return Ok(());
    }
    authorize(state, principal, ScopeChain::org(org_id), requirement).await?;
    match principal {
        Principal::Superadmin => Ok(()),
        Principal::User(user) if user.id == owner_id => Ok(()),
        Principal::User(_) => Err(ApiError::Forbidden),
    }
}

async fn revoke_grant(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<GrantView>> {
    let repo = McpOAuthRepo(pool(&state));
    let grant = repo.get_grant(id).await?;
    let server = McpServerRepo(pool(&state)).get(grant.server_id).await?;
    may_revoke(
        &state,
        &principal,
        server.org_id,
        grant.user_id,
        cap!("mcp_oauth_grant", Delete),
    )
    .await?;
    let actor = match &principal {
        Principal::User(user) => Some(user.id),
        Principal::Superadmin => None,
    };
    // revoking the consent revokes its sessions in the same transaction
    let revoked = repo.revoke_grant(id, actor).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_oauth_grant.revoke",
        "mcp_oauth_grant",
        id,
        serde_json::json!({"server_id": server.id, "user_id": grant.user_id}),
    )
    .await;
    Ok(Json(revoked.into()))
}

async fn list_sessions(
    principal: Principal,
    State(state): State<ControlState>,
    Path(org_id): Path<Uuid>,
) -> ApiResult<Json<Vec<McpOAuthSession>>> {
    let owner = owner_filter(&state, &principal, org_id, cap!("mcp_oauth_session", Read)).await?;
    Ok(Json(
        McpOAuthRepo(pool(&state))
            .list_sessions(org_id, owner)
            .await?,
    ))
}

async fn revoke_session(
    principal: Principal,
    State(state): State<ControlState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<McpOAuthSession>> {
    let repo = McpOAuthRepo(pool(&state));
    let session = repo.get_session(id).await?;
    let grant = repo.get_grant(session.grant_id).await?;
    let server = McpServerRepo(pool(&state)).get(grant.server_id).await?;
    may_revoke(
        &state,
        &principal,
        server.org_id,
        grant.user_id,
        cap!("mcp_oauth_session", Delete),
    )
    .await?;
    let revoked = repo.revoke_session(id).await?;
    publish_config_change(&state).await?;
    log_audit(
        &state,
        &principal,
        Some(server.org_id),
        "mcp_oauth_session.revoke",
        "mcp_oauth_session",
        id,
        serde_json::json!({"grant_id": grant.id, "user_id": grant.user_id}),
    )
    .await;
    Ok(Json(revoked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transports_match_the_schema() {
        // the allowlist, the two check constraints and the dashboard dropdown
        // describe one set. this reads the migrations rather than restating
        // the values, so widening one side without the other fails here (#783)
        let latest = include_str!("../../rolter-store/migrations/0059_mcp_transport_allowlist.sql");
        let settings = include_str!("../../rolter-store/migrations/0055_mcp_management.sql");

        // the SQL tuple each constraint checks against, e.g. "'sse', 'websocket'"
        fn tuple_after(source: &str, needle: &str) -> String {
            let at = source.find(needle).expect("constraint present");
            let rest = &source[at + needle.len()..];
            let open = rest.find('(').expect("open paren");
            let close = rest.find(')').expect("close paren");
            rest[open + 1..close].to_string()
        }

        for tuple in [
            tuple_after(latest, "check (transport in"),
            tuple_after(settings, "check (default_transport in"),
        ] {
            let listed: Vec<String> = tuple
                .split(',')
                .map(|value| value.trim().trim_matches('\'').to_string())
                .collect();
            let expected: Vec<String> = TRANSPORTS.iter().map(|t| t.to_string()).collect();
            assert_eq!(listed, expected, "constraint disagrees with TRANSPORTS");
        }

        // stdio is deliberately gone: a hosted control plane cannot dial a
        // local subprocess, and the HTTP proxy path never served one
        assert!(!TRANSPORTS.contains(&"stdio"));
    }

    #[test]
    fn grant_view_derives_active_from_revocation() {
        let mut grant = McpOAuthGrant {
            id: Uuid::nil(),
            server_id: Uuid::nil(),
            user_id: Uuid::nil(),
            scopes: vec!["tools:read".to_string()],
            granted_at: chrono::Utc::now(),
            revoked_at: None,
            revoked_by: None,
        };
        assert!(GrantView::from(grant.clone()).active);
        grant.revoked_at = Some(chrono::Utc::now());
        assert!(!GrantView::from(grant).active);
    }

    #[test]
    fn registry_rejects_duplicate_or_empty_tool_names() {
        assert!(validate_tools(&["search".to_string(), "search".to_string()]).is_err());
        assert!(validate_tools(&[" ".to_string()]).is_err());
        assert!(validate_tools(&["search".to_string(), "create_issue".to_string()]).is_ok());
    }

    #[test]
    fn legacy_scope_only_server_patch_remains_valid() {
        let patch: UpdateMcpServer = serde_json::from_value(serde_json::json!({
            "required_scopes": ["repo"]
        }))
        .unwrap();
        assert_eq!(patch.required_scopes, Some(vec!["repo".to_string()]));
        assert!(patch.name.is_none());
        assert!(patch.enabled.is_none());
    }

    #[test]
    fn tool_groups_reject_duplicate_server_tool_pairs() {
        let server_id = Uuid::new_v4();
        let duplicate = vec![
            ToolRef {
                server_id,
                tool: "search".to_string(),
            },
            ToolRef {
                server_id,
                tool: "search".to_string(),
            },
        ];
        assert!(validate_tool_refs(&duplicate).is_err());
        assert!(validate_tool_refs(&[ToolRef {
            server_id,
            tool: "search".to_string()
        }])
        .is_ok());
    }

    #[test]
    fn curated_library_slugs_and_tools_are_unique() {
        let mut slugs = std::collections::HashSet::new();
        for item in LIBRARY {
            assert!(
                slugs.insert(item.slug),
                "duplicate library slug: {}",
                item.slug
            );
            assert!(item.url.starts_with("https://"));
            assert!(TRANSPORTS.contains(&item.transport));
            assert!(validate_tools(
                &item
                    .tools
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect::<Vec<_>>()
            )
            .is_ok());
            assert!(validate_required_scopes(
                &item
                    .required_scopes
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect::<Vec<_>>()
            )
            .is_ok());
            // an empty manifest is not a fact about the server, it is a hole in
            // this catalog: it renders as "declares no tools", contributes
            // nothing to the tool tally, and cannot be picked in a tool group
            assert!(
                !item.tools.is_empty(),
                "curated entry {} carries no tools",
                item.slug
            );
        }
    }

    /// Only Notion is allowed an empty scope list, and only because its consent
    /// screen grants selected pages rather than named scopes. Anything else
    /// arriving empty is an unfilled entry, not a server without scopes.
    #[test]
    fn curated_entries_declare_their_oauth_scopes() {
        for item in LIBRARY {
            if item.slug == "notion" {
                assert!(item.required_scopes.is_empty());
            } else {
                assert!(
                    !item.required_scopes.is_empty(),
                    "curated entry {} carries no required scopes",
                    item.slug
                );
            }
        }
    }

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    /// A `library` install is admitted only when it reproduces the catalog
    /// entry, so filling the manifest above is what puts it on the stored
    /// server: a caller cannot install a curated slug with an emptied or
    /// trimmed tool list.
    #[test]
    fn a_curated_install_must_carry_the_catalog_manifest() {
        for item in LIBRARY {
            assert!(matches_catalog(
                item.slug,
                item.name,
                item.url,
                item.transport,
                &owned(item.tools),
                &owned(item.required_scopes),
            ));
            // the pre-#1252 request — a curated slug with nothing declared
            assert!(!matches_catalog(
                item.slug,
                item.name,
                item.url,
                item.transport,
                &[],
                &[],
            ));
            // a trimmed manifest is not the catalog either
            let trimmed: Vec<&str> = item.tools.iter().copied().skip(1).collect();
            assert!(!matches_catalog(
                item.slug,
                item.name,
                item.url,
                item.transport,
                &owned(&trimmed),
                &owned(item.required_scopes),
            ));
        }
    }

    /// A slug that is not in the catalog can never pass as `library`, whatever
    /// it claims to declare.
    #[test]
    fn an_unknown_slug_is_never_a_library_install() {
        assert!(!matches_catalog(
            "totally-not-github",
            "GitHub",
            "https://api.githubcopilot.com/mcp/",
            "streamable_http",
            &[],
            &[],
        ));
        // the right slug pointed at someone else's URL is rejected too
        assert!(!matches_catalog(
            "github",
            "GitHub",
            "https://evil.example.com/mcp/",
            "streamable_http",
            &owned(LIBRARY[0].tools),
            &owned(LIBRARY[0].required_scopes),
        ));
    }
}
