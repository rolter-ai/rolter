import type { McpOAuthClientInput, McpOAuthDiscovery, McpServerRow } from "@/lib/api";

// the OAuth client section's wire rules (#1415), kept out of the component so
// each one is pinned by a unit test. since #1347 a client needs only its id:
// RFC 9728 discovery resolves the endpoints, the typed pair is a fallback, and
// `manual` is the one mode where that pair is required

// the same rule mcp_oauth_flow.rs enforces on write — https, with http allowed
// only on loopback so a local stub stays usable. checked here as well so a typo
// disables the button instead of costing a round trip
const LOOPBACK = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/;
export const oauthEndpoint = (value: string) => /^https:\/\//.test(value) || LOOPBACK.test(value);

export interface OAuthDraft {
  discovery: McpOAuthDiscovery;
  authorizeUrl: string;
  tokenUrl: string;
  issuer: string;
  clientId: string;
  /** never pre-filled: the stored secret is sealed and is never read back */
  secret: string;
  clearSecret: boolean;
  scopes: string;
}

export const splitList = (value: string) => [
  ...new Set(
    value
      .split(/[,\n]/)
      .map((item) => item.trim())
      .filter(Boolean),
  ),
];

// a row from a control plane older than #1347 has no discovery column; it
// behaved as `auto` there, so that is what it is read as
export const oauthDraft = (server: McpServerRow | null): OAuthDraft => ({
  discovery: server?.oauth_discovery ?? "auto",
  authorizeUrl: server?.authorize_url ?? "",
  tokenUrl: server?.token_url ?? "",
  issuer: server?.oauth_issuer ?? "",
  clientId: server?.client_id ?? "",
  secret: "",
  clearSecret: false,
  scopes: (server?.default_scopes ?? []).join(", "),
});

const discoveryOf = (server: McpServerRow | null): McpOAuthDiscovery =>
  server?.oauth_discovery ?? "auto";

/** Whether the operator has started describing a client at all. */
export const oauthTouched = (draft: OAuthDraft, server: McpServerRow | null) =>
  !!(
    draft.authorizeUrl.trim() ||
    draft.tokenUrl.trim() ||
    draft.clientId.trim() ||
    draft.issuer.trim()
  ) || draft.discovery !== discoveryOf(server);

export type OAuthProblem = "clientId" | "pair" | "manualNeedsEndpoints" | "endpoint";

/** The first reason the API would refuse this client, or `null`. */
export function oauthProblem(draft: OAuthDraft): OAuthProblem | null {
  const authorize = draft.authorizeUrl.trim();
  const token = draft.tokenUrl.trim();
  const issuer = draft.issuer.trim();
  if (
    (authorize && !oauthEndpoint(authorize)) ||
    (token && !oauthEndpoint(token)) ||
    (issuer && !oauthEndpoint(issuer))
  )
    return "endpoint";
  if (!draft.clientId.trim()) return "clientId";
  // half a pair is not a fallback anybody can use, in either mode
  if (!!authorize !== !!token) return "pair";
  if (draft.discovery === "manual" && !authorize) return "manualNeedsEndpoints";
  return null;
}

/** Valid when untouched (nothing is sent) or when the API would take it. */
export const oauthValid = (draft: OAuthDraft, server: McpServerRow | null) =>
  !oauthTouched(draft, server) || oauthProblem(draft) === null;

// only send the PUT when something actually moved: the endpoint writes an audit
// entry on every call, and re-saving identical values would fill the log with
// changes nobody made
export const oauthChanged = (draft: OAuthDraft, server: McpServerRow | null) =>
  draft.discovery !== discoveryOf(server) ||
  draft.authorizeUrl.trim() !== (server?.authorize_url ?? "") ||
  draft.tokenUrl.trim() !== (server?.token_url ?? "") ||
  draft.issuer.trim() !== (server?.oauth_issuer ?? "") ||
  draft.clientId.trim() !== (server?.client_id ?? "") ||
  splitList(draft.scopes).join(" ") !== (server?.default_scopes ?? []).join(" ") ||
  !!draft.secret ||
  draft.clearSecret;

// the PUT replaces the whole client, so a field left out is a field cleared:
// `discovery` and `issuer` always travel, since an omitted mode reads as `auto`
// and an omitted issuer unpins it. the endpoints go as a pair or not at all
export function toOAuthInput(draft: OAuthDraft): McpOAuthClientInput {
  const authorize = draft.authorizeUrl.trim();
  const token = draft.tokenUrl.trim();
  const input: McpOAuthClientInput = {
    client_id: draft.clientId.trim(),
    discovery: draft.discovery,
    issuer: draft.issuer.trim() || null,
    default_scopes: splitList(draft.scopes),
  };
  if (authorize && token) {
    input.authorize_url = authorize;
    input.token_url = token;
  }
  // the secret is tri-state on the wire: omitted leaves it alone, "" clears it,
  // a value rotates it. an empty input must never clear a secret the operator
  // simply was not rotating
  if (draft.clearSecret) input.client_secret = "";
  else if (draft.secret) input.client_secret = draft.secret;
  return input;
}

/**
 * Whether saving this URL makes the control plane drop what discovery found.
 *
 * The store compares the stored url with the one sent using `is distinct from`
 * (#1416), and the dialog sends the field untrimmed, so this compares exactly:
 * a trailing space the operator typed is a new url to the backend too.
 */
export const urlResetsDiscovery = (url: string, server: McpServerRow | null) =>
  !!server?.oauth_discovered_at && url !== server.url;

/** Whether saving this draft makes the control plane drop what discovery found. */
export const oauthResetsDiscovery = (draft: OAuthDraft, server: McpServerRow | null, url: string) =>
  !!server?.oauth_discovered_at &&
  (urlResetsDiscovery(url, server) ||
    draft.discovery !== discoveryOf(server) ||
    draft.issuer.trim() !== (server.oauth_issuer ?? ""));

/**
 * Whether Connect has anywhere to send the browser: a client id, and either a
 * configured pair or a mode that is allowed to discover one.
 */
export const oauthConnectable = (server: McpServerRow) =>
  !!server.client_id &&
  (discoveryOf(server) === "auto" || !!(server.authorize_url && server.token_url));
