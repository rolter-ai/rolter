import { getJson, sendJson } from "./request";

// an issued provisioning token as the server stores it. `token_hash` is never
// serialised, so there is no field carrying the secret — a listed token can be
// identified and revoked, never read back.
export interface ScimTokenRow {
  id: string;
  org_id: string;
  name: string;
  created_by: string | null;
  created_at: string;
  /** last time an IdP presented this token; null until it is first used */
  last_used_at: string | null;
  /** set once revoked; a revoked token never authenticates again */
  revoked_at: string | null;
}

// the create response: the stored row plus the one and only sighting of the
// plaintext bearer value
export interface CreatedScimToken extends ScimTokenRow {
  /** the bearer value the IdP is configured with; not stored, not recoverable */
  secret: string;
}

// the base URL an IdP's SCIM connector is pointed at (#2079): the public base
// from `fetchPublicUrl`, then the path scim.rs mounts its resource endpoints
// on. appended to the control plane's own answer and never to
// `window.location.origin`, which is wrong behind a proxy or under a second
// hostname — the IdP calls this from outside, and a wrong value fails in its
// test console with no hint from rolter
export function scimBaseUrl(base: string): string {
  return `${base}/scim/v2`;
}

export function fetchScimTokens(orgId: string): Promise<ScimTokenRow[]> {
  return getJson<ScimTokenRow[]>(`/api/v1/orgs/${orgId}/scim-tokens`);
}

export function createScimToken(orgId: string, input: { name: string }): Promise<CreatedScimToken> {
  return sendJson<CreatedScimToken>("POST", `/api/v1/orgs/${orgId}/scim-tokens`, input);
}

// revocation returns the updated row rather than 204 — the screen uses it to
// show the revoked timestamp without a refetch race
export function revokeScimToken(id: string): Promise<ScimTokenRow> {
  return sendJson<ScimTokenRow>("DELETE", `/api/v1/scim-tokens/${id}`);
}

// --- scim group mappings (crates/rolter-control/src/scim_groups.rs, #540) ---
//
// the operator-facing half of SCIM Groups: the IdP pushes /scim/v2/Groups, and
// these mappings decide what a pushed group is worth. deliberately the same
// model as the SSO one above, down to the vocabulary — one thing to learn, one
// place a privilege bug can hide.

/** an IdP group name granting a role at a scope inside the org that owns it */
export interface ScimGroupMappingRow {
  id: string;
  org_id: string;
  group_name: string;
  /** the most specific non-null scope wins; both null grants at the org */
  team_id: string | null;
  project_id: string | null;
  role: string;
  created_at: string;
}

export interface CreateScimGroupMappingInput {
  group_name: string;
  /** `admin` | `member` | `viewer`; `parse_role` refuses anything else */
  role: string;
  /** omit both to grant at the org; the scope must live in the same org */
  team_id?: string;
  project_id?: string;
}

export function fetchScimGroupMappings(orgId: string): Promise<ScimGroupMappingRow[]> {
  return getJson<ScimGroupMappingRow[]>(`/api/v1/orgs/${orgId}/scim-group-mappings`);
}

// creating one reconciles the group's members straight away rather than at the
// next sync, so the list the screen refetches is already the new truth
export function createScimGroupMapping(
  orgId: string,
  input: CreateScimGroupMappingInput,
): Promise<ScimGroupMappingRow> {
  return sendJson<ScimGroupMappingRow>("POST", `/api/v1/orgs/${orgId}/scim-group-mappings`, input);
}

export function deleteScimGroupMapping(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/scim-group-mappings/${id}`);
}

// --- self-service (crates/rolter-control/src/me.rs, ROL-224) ---
//
// end-user surface: manage your own virtual keys and see your own usage. these
// require a real login session (not the admin token path).
