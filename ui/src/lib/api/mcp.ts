import { type AnalyticsWindow, getAnalytics, windowParams } from "./analytics";
import { getJson, sendJson } from "./request";

export const MCP_TRANSPORTS = ["stdio", "streamable_http", "sse", "websocket"] as const;

export const MCP_STATUSES = [
  "success",
  "error",
  "timeout",
  "auth_denied",
  "transport_error",
] as const;

export interface McpLogRow {
  ts: string;
  event_id: string;
  server: string;
  tool: string;
  transport: string;
  status: string;
  latency_ms: number;
  org_id: string;
  team_id: string;
  project_id: string;
  virtual_key_id: string;
  user_id: string;
  request_id: string;
  trace_id: string;
  error: string | null;
  /// 1 for a `tools/call` sent as a JSON-RPC notification, which no reply
  /// answers, so `status` is only what the HTTP layer said (#3466, #3526); 0
  /// before ClickHouse migration 047
  notification?: number | string;
  /// what refines `status` for a call whose result nothing judged (#3529):
  /// `accepted` when a legacy `sse` server took the call and sends its reply on
  /// the stream. empty for every other row, and before migration 051
  outcome?: string;
}

export interface McpLogDetail extends McpLogRow {
  arguments: string | null;
  result: string | null;
  /// 1 when the arguments and result were captured but the caller's role is
  /// below the payload floor, so the server blanked them (#2396)
  payload_withheld?: number | string;
  /// the OAuth scopes an upstream's `insufficient_scope` refusal asked for
  /// (#3375); absent on a row written before ClickHouse migration 042
  error_scopes?: string[];
}

export interface McpLogsQuery extends AnalyticsWindow {
  server?: string;
  tool?: string;
  transport?: string;
  status?: string;
  key?: string;
  user?: string;
  limit?: number;
  cursor?: string;
}

export interface McpSummaryRow {
  calls: string | number;
  /// calls no reply was awaited for (notifications, `accepted` calls), counted
  /// apart and left out of `successes`, `failures` and both rates (#3526)
  not_awaited?: string | number;
  successes?: string | number;
  failures: string | number;
  /// over the awaited calls only; null when the window has none
  success_rate?: number | null;
  failure_rate?: number | null;
  // clickhouse averages an empty window to null (and quantile to nan); the
  // control plane forwards the JSON as-is
  avg_latency_ms: number | null;
  p95_latency_ms: number | null;
}

export function fetchMcpLogs(
  query: McpLogsQuery = {},
): Promise<{ data: McpLogRow[]; next_cursor: string | null }> {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== "") {
      params.set(key, String(value));
    }
  }
  const qs = params.toString();
  return getAnalytics<{ data: McpLogRow[]; next_cursor: string | null }>(
    `/api/v1/mcp/logs${qs ? `?${qs}` : ""}`,
  );
}

/**
 * The one aggregate row, or `null` for an empty envelope rather than
 * `undefined` — the same rule as `fetchAnalyticsSummary`, one endpoint over
 * (#1611). react-query v5 rejects a query function that resolves to
 * `undefined`, so an empty `data` turned the MCP logs summary into the load
 * error panel instead of zeroes.
 */
export function fetchMcpSummary(window: AnalyticsWindow = {}): Promise<McpSummaryRow | null> {
  return getAnalytics<{ data: McpSummaryRow[] }>(
    `/api/v1/mcp/logs/summary${windowParams(window)}`,
  ).then((r) => r.data[0] ?? null);
}

export function fetchMcpLogDetail(eventId: string): Promise<McpLogDetail> {
  // by-id: a 404 here is "no such event", not "analytics is unavailable"
  return getAnalytics<McpLogDetail>(`/api/v1/mcp/logs/${encodeURIComponent(eventId)}`, {
    notFoundIsUnavailable: false,
  });
}

// ---------------------------------------------------------------------------
// mcp servers, oauth consent grants and token sessions
// (crates/rolter-control/src/mcp_oauth.rs, #561)
//
// three properties of that module are load-bearing for the screens below:
// no token material ever crosses this boundary (a session carries metadata and
// `has_refresh_token`, never a token); a grant is the unit of consent, so
// revoking one revokes its sessions in the same transaction; and a listing is
// owner-scoped — an org admin sees the whole org, everyone else sees only the
// rows they own

export interface McpServerRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  url: string;
  transport: string;
  description: string;
  enabled: boolean;
  tools: string[];
  source: "custom" | "library";
  required_scopes: string[];
  created_at: string;
  /// authorization endpoint a user's browser is sent to for consent (#707)
  authorize_url: string | null;
  /// token endpoint the code, refresh and exchange grants are posted to
  token_url: string | null;
  /// the OAuth client rolter presents. the matching secret is sealed and
  /// deliberately never crosses this boundary
  client_id: string | null;
  /// scopes requested when a consent flow does not name its own
  default_scopes: string[];
  /// whether a sealed client secret is stored, so the UI can show the client is
  /// confidential without the control plane handing the secret out
  has_client_secret: boolean;
  /// how rolter authenticates to this server (#952). `oauth` is the per-user
  /// consent flow; `bearer` and `header` present one deployment-wide credential
  auth_kind: McpAuthKind;
  /// the header a `header`-kind credential is presented in, e.g. `X-Api-Key`
  auth_header_name: string | null;
  /// whether a sealed static credential is stored. the credential itself is
  /// write-only and never crosses this boundary
  has_credential: boolean;
  /// per-server transport overrides; `null` inherits the deployment default
  connect_timeout_ms: number | null;
  request_timeout_ms: number | null;
  max_retries: number | null;
  /// the authorization server's issuer as an operator pinned it (#1347), for
  /// RFC 9207 `iss` validation against something authentic
  oauth_issuer: string | null;
  /// `auto` tries RFC 9728 discovery before the configured endpoints; `manual`
  /// never probes
  oauth_discovery: McpOAuthDiscovery;
  /// what the last successful discovery resolved — a cache the interactive
  /// authorize refreshes, cleared whenever the url, issuer or mode moves
  oauth_discovered_issuer: string | null;
  oauth_discovered_authorize_url: string | null;
  oauth_discovered_token_url: string | null;
  /// whether that metadata advertised `authorization_response_iss_parameter_supported`
  oauth_discovered_iss_supported: boolean;
  oauth_discovered_at: string | null;
  /// whether the server ever answered `tools/list`. `pending` means `tools` is
  /// only the catalog's fallback list (or what an operator typed); `discovered`
  /// means the server answered, so an empty `tools` is "declares no tools";
  /// `failed` means a later attempt got no answer and the last manifest is kept
  tools_discovery_status: McpToolsDiscoveryStatus;
  /// a bounded code for the last attempt, `null` after a success
  tools_discovery_error: McpToolsDiscoveryError | null;
  /// the last successful discovery
  tools_discovered_at: string | null;
  /// the last attempt of any outcome; `null` means discovery has not run yet
  tools_discovery_attempted_at: string | null;
  /// names in `tools` the last successful discovery no longer returned. they
  /// are kept, never pruned, so a tool group naming one keeps working
  tools_stale: string[];
  /// the last completed MCP handshake, or `null` when none ever completed
  last_contact_at: string | null;
  /// what that handshake negotiated
  last_protocol_version: string | null;
  last_server_name: string | null;
  /// the last failed contact. kept after a later success, so the current state
  /// is whichever of `last_contact_at` and `last_error_at` is later
  last_error_kind: McpConnectionError | null;
  last_error_at: string | null;
}

export const MCP_CONNECTION_ERRORS = [
  "unreachable",
  "handshake_failed",
  "auth_rejected",
  "egress_denied",
] as const;

export type McpConnectionError = (typeof MCP_CONNECTION_ERRORS)[number];

export type McpToolsDiscoveryStatus = "pending" | "discovered" | "failed";

export const MCP_TOOLS_DISCOVERY_ERRORS = [
  "unreachable",
  "auth_rejected",
  "handshake_failed",
  "egress_denied",
  "needs_session",
  "credential_unavailable",
] as const;

export type McpToolsDiscoveryError = (typeof MCP_TOOLS_DISCOVERY_ERRORS)[number];

export interface McpServerToolRow {
  name: string;
  description: string | null;
  input_schema: unknown;
  stale: boolean;
}

export interface McpServerToolsResponse {
  status: McpToolsDiscoveryStatus;
  error: McpToolsDiscoveryError | null;
  discovered_at: string | null;
  tools: McpServerToolRow[];
}

export const MCP_OAUTH_DISCOVERY_MODES = ["auto", "manual"] as const;

export type McpOAuthDiscovery = (typeof MCP_OAUTH_DISCOVERY_MODES)[number];

export const MCP_AUTH_KINDS = ["none", "bearer", "header", "oauth"] as const;

export type McpAuthKind = (typeof MCP_AUTH_KINDS)[number];

/**
 * Header names the control plane refuses as `auth_header_name`, lowercased —
 * mirrors `RESERVED_AUTH_HEADERS` in `crates/rolter-control/src/mcp_oauth.rs`
 * and the `mcp_servers_auth_header_name_shape` constraint, so the form refuses
 * them before the server has to.
 */
export const MCP_RESERVED_AUTH_HEADERS = [
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
] as const;

/**
 * `PUT /api/v1/mcp-servers/{id}/auth`. `credential` is tri-state on the wire:
 * omitted leaves the stored one, `""` clears it, anything else replaces it —
 * so a form must only send it when the operator actually typed one.
 */
export interface McpServerAuthInput {
  auth_kind: McpAuthKind;
  /** required for `header`, refused for every other kind */
  auth_header_name?: string;
  credential?: string;
}

/**
 * The transport overrides a `PATCH` may carry. Each is tri-state as well:
 * absent leaves the override, `null` drops it back to inheriting, a number
 * sets it. A form that always sends every field cannot say "leave it".
 */
export interface McpTransportOverridesPatch {
  connect_timeout_ms?: number | null;
  request_timeout_ms?: number | null;
  max_retries?: number | null;
}

export interface McpServerInput {
  name: string;
  slug: string;
  url: string;
  transport: string;
  description: string;
  enabled: boolean;
  tools: string[];
  source: "custom" | "library";
  required_scopes: string[];
}

export interface McpLibraryItem {
  slug: string;
  name: string;
  description: string;
  url: string;
  transport: string;
  tools: string[];
  required_scopes: string[];
  installed: boolean;
}

export interface McpToolRef {
  server_id: string;
  tool: string;
}

export interface McpToolGroupRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  description: string;
  enabled: boolean;
  tools: McpToolRef[];
  created_at: string;
  updated_at: string;
}

/** What an MCP tool group can be assigned to (#3295). */
export type McpAssignmentKind = "virtual_key" | "project" | "team";

export interface McpToolGroupAssignmentRow {
  group_id: string;
  subject_kind: McpAssignmentKind;
  subject_id: string;
  created_at: string;
}

export interface McpAssignmentSubject {
  kind: McpAssignmentKind;
  id: string;
}

export interface McpGatewaySettingsRow {
  org_id: string;
  default_transport: string;
  connect_timeout_ms: number;
  request_timeout_ms: number;
  max_retries: number;
  default_failure_mode: "fail_open" | "fail_closed";
  allow_unlisted_tools: boolean;
  updated_at: string;
}

export interface McpOAuthGrantRow {
  id: string;
  server_id: string;
  user_id: string;
  scopes: string[];
  granted_at: string;
  revoked_at: string | null;
  revoked_by: string | null;
  /** Resolved server-side so clients never infer state from a nullable timestamp. */
  active: boolean;
}

/** Session metadata for a grant without exposing sealed token material. */
export interface McpOAuthSessionRow {
  id: string;
  grant_id: string;
  scopes: string[];
  expires_at: string;
  refresh_expires_at: string | null;
  revoked_at: string | null;
  created_at: string;
  last_used_at: string | null;
  /** Whether a stored refresh token makes this session renewable. */
  has_refresh_token: boolean;
}

export function fetchMcpServers(orgId: string): Promise<McpServerRow[]> {
  return getJson<McpServerRow[]>(`/api/v1/orgs/${orgId}/mcp-servers`);
}

export function createMcpServer(orgId: string, input: McpServerInput): Promise<McpServerRow> {
  return sendJson<McpServerRow>("POST", `/api/v1/orgs/${orgId}/mcp-servers`, input);
}

export function updateMcpServer(
  id: string,
  input: Omit<McpServerInput, "slug" | "source"> & McpTransportOverridesPatch,
): Promise<McpServerRow> {
  return sendJson<McpServerRow>("PATCH", `/api/v1/mcp-servers/${id}`, input);
}

// the only route that takes a server's static credential, kept apart from the
// PATCH so the general edit path never carries a secret. setting anything here
// needs ROLTER_KEK on the control plane, which refuses rather than storing a
// credential in the clear
export function setMcpServerAuth(id: string, input: McpServerAuthInput): Promise<McpServerRow> {
  return sendJson<McpServerRow>("PUT", `/api/v1/mcp-servers/${id}/auth`, input);
}

// asks the server for its tool manifest now. a discovery that got an error
// code answers 200 with the row carrying it, so a rejection here is the
// request itself failing
export function discoverMcpServerTools(id: string): Promise<McpServerRow> {
  return sendJson<McpServerRow>("POST", `/api/v1/mcp-servers/${id}/discover`);
}

export function fetchMcpServerTools(id: string): Promise<McpServerToolsResponse> {
  return getJson<McpServerToolsResponse>(`/api/v1/mcp-servers/${id}/tools`);
}

/// every answer a connection test can give; it is always an HTTP 200, so the
/// failure is in `status`
export const MCP_PROBE_STATUSES = [
  "reachable",
  "unreachable",
  "handshake_failed",
  "auth_rejected",
  "egress_denied",
  "needs_session",
  "credential_unavailable",
] as const;

export type McpProbeStatus = (typeof MCP_PROBE_STATUSES)[number];

export interface McpConnectionTest {
  status: McpProbeStatus;
  reachable: boolean;
  handshake_completed: boolean;
  protocol_version: string | null;
  server_name: string | null;
  server_version: string | null;
  latency_ms: number;
  message: string | null;
  checked_at: string;
}

/// `tool_errored` is the server answering and the tool reporting an error: not
/// a connection failure. the rest are the failure statuses of a connection test
export type McpToolCallOutcome = "success" | "tool_errored" | Exclude<McpProbeStatus, "reachable">;

export interface McpToolCallInput {
  tool: string;
  arguments?: Record<string, unknown>;
}

export interface McpToolCallResult {
  outcome: McpToolCallOutcome;
  ok: boolean;
  latency_ms: number;
  /// the raw JSON-RPC messages. a response too large to keep arrives as
  /// `{ truncated: true, size_bytes, preview }`
  request: unknown;
  response: unknown;
  message: string | null;
}

// a live handshake against the server, with the caller's own credential or
// session. a failure answers 200 with the reason in `status`
export function testMcpServerConnection(id: string): Promise<McpConnectionTest> {
  return sendJson<McpConnectionTest>("POST", `/api/v1/mcp-servers/${id}/test`);
}

// runs one tool of the server's manifest for real. a 400 means the tool is not
// in the manifest or the arguments are not an object
export function callMcpServerTool(id: string, input: McpToolCallInput): Promise<McpToolCallResult> {
  return sendJson<McpToolCallResult>("POST", `/api/v1/mcp-servers/${id}/tools/call`, input);
}

export function deleteMcpServer(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/mcp-servers/${id}`);
}

export function fetchMcpLibrary(orgId: string): Promise<McpLibraryItem[]> {
  return getJson<McpLibraryItem[]>(`/api/v1/orgs/${orgId}/mcp/library`);
}

export function fetchMcpToolGroups(orgId: string): Promise<McpToolGroupRow[]> {
  return getJson<McpToolGroupRow[]>(`/api/v1/orgs/${orgId}/mcp/tool-groups`);
}

export function createMcpToolGroup(
  orgId: string,
  input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at"> & {
    /** written in the same transaction as the group; empty means the whole organization */
    assignments?: McpAssignmentSubject[];
  },
): Promise<McpToolGroupRow> {
  return sendJson<McpToolGroupRow>("POST", `/api/v1/orgs/${orgId}/mcp/tool-groups`, input);
}

export function updateMcpToolGroup(
  id: string,
  input: Omit<McpToolGroupRow, "id" | "org_id" | "slug" | "created_at" | "updated_at">,
): Promise<McpToolGroupRow> {
  return sendJson<McpToolGroupRow>("PUT", `/api/v1/mcp/tool-groups/${id}`, input);
}

export function deleteMcpToolGroup(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/mcp/tool-groups/${id}`);
}

/** Every assignment of every tool group of the org. */
export function fetchMcpToolGroupAssignments(orgId: string): Promise<McpToolGroupAssignmentRow[]> {
  return getJson<McpToolGroupAssignmentRow[]>(`/api/v1/orgs/${orgId}/mcp/tool-group-assignments`);
}

/** Replaces the whole set; an empty one hands the group back to the organization. */
export function replaceMcpToolGroupAssignments(
  groupId: string,
  assignments: McpAssignmentSubject[],
): Promise<McpToolGroupAssignmentRow[]> {
  return sendJson<McpToolGroupAssignmentRow[]>(
    "PUT",
    `/api/v1/mcp/tool-groups/${groupId}/assignments`,
    { assignments },
  );
}

export function fetchMcpSettings(orgId: string): Promise<McpGatewaySettingsRow> {
  return getJson<McpGatewaySettingsRow>(`/api/v1/orgs/${orgId}/mcp/settings`);
}

export function updateMcpSettings(
  orgId: string,
  input: Omit<McpGatewaySettingsRow, "org_id" | "updated_at">,
): Promise<McpGatewaySettingsRow> {
  return sendJson<McpGatewaySettingsRow>("PUT", `/api/v1/orgs/${orgId}/mcp/settings`, input);
}

export function fetchMcpGrants(orgId: string): Promise<McpOAuthGrantRow[]> {
  return getJson<McpOAuthGrantRow[]>(`/api/v1/orgs/${orgId}/mcp/grants`);
}

// revoking consent cascades to every session under the grant
export function revokeMcpGrant(id: string): Promise<McpOAuthGrantRow> {
  return sendJson<McpOAuthGrantRow>("DELETE", `/api/v1/mcp/grants/${id}`);
}

export function fetchMcpSessions(orgId: string): Promise<McpOAuthSessionRow[]> {
  return getJson<McpOAuthSessionRow[]>(`/api/v1/orgs/${orgId}/mcp/sessions`);
}

// revoking one session leaves the consent standing; a new session can be
// minted against the same grant without asking the user again
export function revokeMcpSession(id: string): Promise<McpOAuthSessionRow> {
  return sendJson<McpOAuthSessionRow>("DELETE", `/api/v1/mcp/sessions/${id}`);
}

// ---------------------------------------------------------------------------
// mcp oauth client registration and the consent flow it opens
// (crates/rolter-control/src/mcp_oauth_flow.rs, #707)
//
// the block above is the shelf — grants and sessions that already exist. this
// is the lifecycle that fills it, and three of its rules are visible from
// here: the client secret is write-only (a read answers with
// `has_client_secret` and nothing else), the redirect uri is deployment-derived
// rather than sent by the caller, and nothing minted under a grant may ever
// carry scopes that grant does not

/** A server's registered OAuth client as it reads back — never the secret. */
export interface McpOAuthClientRow {
  server_id: string;
  authorize_url: string | null;
  token_url: string | null;
  client_id: string | null;
  default_scopes: string[];
  has_client_secret: boolean;
  /** the callback to register upstream; derived from the deployment, so it is
   * returned rather than guessed from the browser's own origin */
  redirect_uri: string;
  issuer: string | null;
  discovery: McpOAuthDiscovery;
  /** the RFC 8707 resource every token for this server is bound to */
  resource: string | null;
  discovered_issuer: string | null;
  discovered_authorize_url: string | null;
  discovered_token_url: string | null;
  discovered_at: string | null;
}

export interface McpOAuthClientInput {
  /** optional since #1347, and both or neither: under `auto` discovery they
   * are only the fallback, under `manual` they are required */
  authorize_url?: string;
  token_url?: string;
  client_id: string;
  /** `null` unpins the issuer */
  issuer?: string | null;
  /** always sent: an omitted mode is read as `auto`, which silently reset a
   * `manual` server on every re-save */
  discovery?: McpOAuthDiscovery;
  /** omitted leaves the sealed secret alone; `""` clears it, which is how a
   * confidential client is downgraded to a public one */
  client_secret?: string;
  default_scopes?: string[];
}

/** Where to send the browser for consent, and how long that url stays good. */
export interface McpAuthorizeStarted {
  authorization_url: string;
  state: string;
  expires_in: number;
}

export function fetchMcpOAuthClient(serverId: string): Promise<McpOAuthClientRow> {
  return getJson<McpOAuthClientRow>(`/api/v1/mcp-servers/${serverId}/oauth-client`);
}

export function setMcpOAuthClient(
  serverId: string,
  input: McpOAuthClientInput,
): Promise<McpOAuthClientRow> {
  return sendJson<McpOAuthClientRow>("PUT", `/api/v1/mcp-servers/${serverId}/oauth-client`, input);
}

// the control plane does not redirect here: the caller is a `fetch` from the
// dashboard, which cannot usefully follow a cross-origin 302, so the url comes
// back for the browser to open itself
export function startMcpOAuth(serverId: string, scopes?: string[]): Promise<McpAuthorizeStarted> {
  return sendJson<McpAuthorizeStarted>(
    "POST",
    `/api/v1/mcp-servers/${serverId}/oauth/authorize`,
    scopes ? { scopes } : {},
  );
}

/**
 * Consent again for more access (#3296, RFC 6749 §3.3 step-up): the scopes the
 * grant already holds and the server requires are kept and `scopes` are added,
 * so the re-consent widens the grant instead of replacing it with the narrower
 * set. Answered like `startMcpOAuth`, with the url to open.
 */
export function startMcpStepUp(serverId: string, scopes: string[]): Promise<McpAuthorizeStarted> {
  return sendJson<McpAuthorizeStarted>("POST", `/api/v1/mcp-servers/${serverId}/oauth/authorize`, {
    step_up_scopes: scopes,
  });
}

// renews one session from its stored refresh token. a refusal from the
// authorization server revokes the session rather than being retried, so a
// failure here can legitimately mean "consent is gone, ask for it again"
export function refreshMcpSession(id: string): Promise<McpOAuthSessionRow> {
  return sendJson<McpOAuthSessionRow>("POST", `/api/v1/mcp/sessions/${id}/refresh`);
}

export interface McpSessionExchangeInput {
  scopes?: string[];
  /** RFC 8693 `audience` — the downstream service the token is for */
  audience?: string;
}

// on-behalf-of exchange (RFC 8693): a narrower session descending from the
// same grant, revocable on its own and never broader than the consent
export function exchangeMcpSession(
  id: string,
  input: McpSessionExchangeInput = {},
): Promise<McpOAuthSessionRow> {
  return sendJson<McpOAuthSessionRow>("POST", `/api/v1/mcp/sessions/${id}/exchange`, input);
}

// ---------------------------------------------------------------------------
// complexity routing policy (stored in route params, validated server-side)
