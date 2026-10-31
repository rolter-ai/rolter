// the words the dashboard puts on enums the control plane stores as bare
// identifiers (#2098, #2023). A badge printed the identifier as it came, so a
// Russian screen read "HEALTHY" and "REVOKED".
//
// One table per enum family maps each value this build knows to its catalog
// key. A value from a later build prints as stored rather than as a blank or a
// raw key, the way `stateLabel` treats an alerting state it has not met.
//
// What stays verbatim (PRODUCT.md) never goes through here: strategy names,
// model ids and provider kinds are copied back into config.

export type Translate = (key: string, options?: Record<string, unknown>) => string;

/** the label for `value` from a table of catalog keys; an unknown value as stored */
export function enumLabel(t: Translate, keys: Readonly<Record<string, string>>, value: string) {
  return Object.prototype.hasOwnProperty.call(keys, value) ? t(keys[value]) : value;
}

// a grant is `active` or `revoked`; a session is `active`, `expired` or
// `revoked` (crates/rolter-control/src/mcp_oauth.rs)
const OAUTH_STATE_KEYS = {
  active: "pages.mcpOAuth.states.active",
  expired: "pages.mcpOAuth.states.expired",
  revoked: "pages.mcpOAuth.states.revoked",
} as const;

/** an OAuth grant's or session's state, translated */
export const oauthStateLabel = (state: string, t: Translate) =>
  enumLabel(t, OAUTH_STATE_KEYS, state);

/** a grant is live or revoked, and carries no state string of its own */
export const grantStateLabel = (active: boolean, t: Translate) =>
  t(active ? OAUTH_STATE_KEYS.active : OAUTH_STATE_KEYS.revoked);

// what a connector's last delivery probe concluded
const CONNECTOR_HEALTH_KEYS = {
  healthy: "pages.connectors.health.healthy",
  unhealthy: "pages.connectors.health.unhealthy",
  unknown: "pages.connectors.health.unknown",
} as const;

/** a connector's health status, translated */
export const connectorHealthLabel = (status: string, t: Translate) =>
  enumLabel(t, CONNECTOR_HEALTH_KEYS, status);

// the editor's pickers and the rendered preview read the same words
const DECORATOR_ROLE_KEYS = {
  system: "pages.promptRepo.roleSystem",
  assistant: "pages.promptRepo.roleAssistant",
  user: "pages.promptRepo.roleUser",
} as const;

const DECORATOR_POSITION_KEYS = {
  prepend: "pages.promptRepo.positionPrepend",
  append: "pages.promptRepo.positionAppend",
} as const;

/** a prompt decorator's message role, translated */
export const decoratorRoleLabel = (role: string, t: Translate) =>
  enumLabel(t, DECORATOR_ROLE_KEYS, role);

/** where a prompt decorator sits relative to the caller's messages, translated */
export const decoratorPositionLabel = (position: string, t: Translate) =>
  enumLabel(t, DECORATOR_POSITION_KEYS, position);

const SKILL_ROLE_KEYS = {
  viewer: "pages.skillsRepo.roleViewer",
  member: "pages.skillsRepo.roleMember",
  admin: "pages.skillsRepo.roleAdmin",
} as const;

/** the lowest role that may use a skill, translated */
export const skillRoleLabel = (role: string, t: Translate) => enumLabel(t, SKILL_ROLE_KEYS, role);

// how an MCP tool call ended (`MCP_STATUSES` in api/mcp.ts). the transport names
// beside it (`streamable_http`, `sse`) are protocol identifiers and stay verbatim
const MCP_STATUS_KEYS = {
  success: "pages.mcpLogs.statuses.success",
  error: "pages.mcpLogs.statuses.error",
  timeout: "pages.mcpLogs.statuses.timeout",
  auth_denied: "pages.mcpLogs.statuses.auth_denied",
  transport_error: "pages.mcpLogs.statuses.transport_error",
} as const;

/** how an MCP tool call ended, translated */
export const mcpStatusLabel = (status: string, t: Translate) =>
  enumLabel(t, MCP_STATUS_KEYS, status);
