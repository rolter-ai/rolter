import { getJson, sendJson } from "./request";

/**
 * A scoped control-plane API token, as `ApiToken` serialises it
 * (crates/rolter-control/src/api_tokens.rs, #1836).
 *
 * The secret never appears in a list: only the `token_prefix` that tells one
 * token from another. A revoked or expired token stays in the list, so the
 * state is derived from `revoked_at` and `expires_at` (see `apiTokenState`).
 */
export interface ApiTokenRow {
  id: string;
  org_id: string;
  /** both null: org-wide; `project_id` set: that project (its team too, when named) */
  team_id: string | null;
  project_id: string | null;
  name: string;
  /** a built-in role, or null when the token holds a custom role */
  role: string | null;
  custom_role_id: string | null;
  token_prefix: string;
  expires_at: string | null;
  created_by: string | null;
  created_at: string;
  rotated_at: string | null;
  last_used_at: string | null;
  revoked_at: string | null;
}

/** a minted or rotated token: the row plus the secret, returned exactly once */
export interface MintedApiToken extends ApiTokenRow {
  secret: string;
}

export interface MintApiTokenInput {
  name: string;
  team_id?: string;
  project_id?: string;
  /** exactly one of `role` and `custom_role_id` */
  role?: string;
  custom_role_id?: string;
  /** 1 to 730; the server's default is 90 when omitted */
  expires_in_days?: number;
  /** cannot be combined with `expires_in_days` */
  never_expires?: boolean;
}

export type ApiTokenState = "active" | "expired" | "revoked";

/** what a token can do right now; revocation wins over expiry */
export function apiTokenState(token: ApiTokenRow, now: number = Date.now()): ApiTokenState {
  if (token.revoked_at) return "revoked";
  if (token.expires_at && Date.parse(token.expires_at) <= now) return "expired";
  return "active";
}

export function fetchApiTokens(orgId: string): Promise<ApiTokenRow[]> {
  return getJson<ApiTokenRow[]>(`/api/v1/orgs/${orgId}/api-tokens`);
}

export function mintApiToken(orgId: string, input: MintApiTokenInput): Promise<MintedApiToken> {
  return sendJson<MintedApiToken>("POST", `/api/v1/orgs/${orgId}/api-tokens`, input);
}

/** the new secret replaces the old one in the same instant */
export function rotateApiToken(id: string): Promise<MintedApiToken> {
  return sendJson<MintedApiToken>("POST", `/api/v1/api-tokens/${id}/rotate`);
}

export function revokeApiToken(id: string): Promise<ApiTokenRow> {
  return sendJson<ApiTokenRow>("DELETE", `/api/v1/api-tokens/${id}`);
}
