import { type MfaPolicy } from "./mfa";
import { getJson, sendJson } from "./request";

/**
 * A registered identity provider, as `SsoProvider` serialises it.
 *
 * The sealed client secret is absent from the payload — it is never redacted,
 * it simply never leaves the control plane. What comes in its place is the
 * derived `has_client_secret` boolean, so the screen can tell a provider that
 * is ready to exchange tokens from one whose first symptom would otherwise be
 * a failed login (#1231).
 */
export interface SsoProviderRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  issuer: string;
  client_id: string;
  /**
   * whether a client secret is sealed in the store. derived on the server from
   * `secret_ciphertext.is_some()`; the bytes themselves are never serialised.
   */
  has_client_secret: boolean;
  scopes: string[];
  /** claim in the id token carrying the user's groups; `groups` by default */
  group_claim: string;
  /** role granted when no mapping matches; null refuses that user entirely */
  default_role: string | null;
  enabled: boolean;
  created_at: string;
  /**
   * the callback to register in the identity provider, built from the
   * deployment's public URL by the same function the login flow uses, so it is
   * returned rather than assembled from the browser's origin (#2083)
   */
  redirect_uri: string;
  /** where a user's sign-in through this provider starts; what users follow */
  login_url: string;
}

export interface CreateSsoProviderInput {
  name: string;
  slug: string;
  issuer: string;
  client_id: string;
  /** write-only: sealed with the KEK before storage and never returned */
  client_secret?: string;
  /** defaults to openid/email/profile when omitted */
  scopes?: string[];
  group_claim?: string;
  default_role?: string;
}

export function fetchSsoProviders(orgId: string): Promise<SsoProviderRow[]> {
  return getJson<SsoProviderRow[]>(`/api/v1/orgs/${orgId}/sso-providers`);
}

export function createSsoProvider(
  orgId: string,
  input: CreateSsoProviderInput,
): Promise<SsoProviderRow> {
  return sendJson<SsoProviderRow>("POST", `/api/v1/orgs/${orgId}/sso-providers`, input);
}

/**
 * The editable half of a provider. `slug` is absent on purpose: it is in the
 * login URL, so the server refuses to change it (#1233).
 *
 * `client_secret` is three-valued. Omit it to leave the sealed secret alone,
 * send a value to rotate it, send `""` to clear it and make the provider a
 * public PKCE client.
 */
export interface UpdateSsoProviderInput {
  name: string;
  issuer: string;
  client_id: string;
  client_secret?: string;
  scopes?: string[];
  group_claim?: string;
  default_role?: string;
  enabled: boolean;
}

export function updateSsoProvider(
  id: string,
  input: UpdateSsoProviderInput,
): Promise<SsoProviderRow> {
  return sendJson<SsoProviderRow>("PUT", `/api/v1/sso-providers/${id}`, input);
}

export function deleteSsoProvider(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/sso-providers/${id}`);
}

/** an IdP group name granting a role at a scope inside the provider's own org */
export interface SsoGroupMappingRow {
  id: string;
  provider_id: string;
  group_name: string;
  org_id: string | null;
  team_id: string | null;
  project_id: string | null;
  role: string;
  created_at: string;
}

export interface CreateSsoGroupMappingInput {
  group_name: string;
  role: string;
  /** all three omitted grants at the provider's own org, which is what the
   * dashboard sends; a mapping may never reach outside that org */
  org_id?: string;
  team_id?: string;
  project_id?: string;
}

export function fetchSsoGroupMappings(providerId: string): Promise<SsoGroupMappingRow[]> {
  return getJson<SsoGroupMappingRow[]>(`/api/v1/sso-providers/${providerId}/group-mappings`);
}

export function createSsoGroupMapping(
  providerId: string,
  input: CreateSsoGroupMappingInput,
): Promise<SsoGroupMappingRow> {
  return sendJson<SsoGroupMappingRow>(
    "POST",
    `/api/v1/sso-providers/${providerId}/group-mappings`,
    input,
  );
}

export function deleteSsoGroupMapping(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/sso-group-mappings/${id}`);
}

/**
 * The redirect URI a provider with this slug will have, for the add sheet.
 *
 * Mirrors `redirect_uri` in crates/rolter-control/src/sso.rs: the public base
 * from `fetchPublicUrl`, then `/auth/sso/{slug}/callback`, with nothing
 * encoded — the server does not encode it either, and the slug's charset is
 * url-safe by constraint. The identity provider wants this before it issues
 * the client id and secret the sheet asks for, so it cannot wait for the row;
 * a saved provider carries the server's own `redirect_uri` instead.
 */
export function ssoRedirectUri(base: string, slug: string): string {
  return `${base}/auth/sso/${slug}/callback`;
}

// --- org sign-in policy (crates/rolter-control/src/auth_policy.rs, #240) ---

/** `OrgAuthPolicy`; decides what the login screen offers this org, and what it demands */
export interface OrgAuthPolicy {
  org_id: string;
  allow_password_login: boolean;
  allow_sso: boolean;
  /**
   * How hard this org insists on a second factor (#1078).
   *
   * A user in several orgs gets the strictest value across all of them — a
   * relaxed membership never softens a hardened one.
   */
  mfa_policy: MfaPolicy;
  /**
   * When a `required_*` policy starts making unenrolled members enrol before
   * they get a session (#1852). `null` means it already does; always `null`
   * under `off` and `optional`.
   */
  mfa_enforce_after: string | null;
  updated_at: string;
}

export function fetchAuthPolicy(orgId: string): Promise<OrgAuthPolicy> {
  return getJson<OrgAuthPolicy>(`/api/v1/orgs/${orgId}/auth-policy`);
}

/**
 * Both flags travel together, because the server refuses the *combination*
 * rather than either field: turning both off is an outage, and turning
 * passwords off before an enabled provider exists locks every non-superadmin
 * out. Each comes back 409 carrying its own message.
 */
export function updateAuthPolicy(
  orgId: string,
  input: {
    allow_password_login: boolean;
    allow_sso: boolean;
    /**
     * Omitted keeps the org's current setting rather than resetting it to
     * `off` — the control plane reads the field as optional so a client
     * written before second factors existed cannot silently disarm one.
     */
    mfa_policy?: MfaPolicy;
    /**
     * Read whenever `mfa_policy` is sent — `null` means "at once" — and kept
     * as it is whenever `mfa_policy` is not, so the two travel together.
     */
    mfa_enforce_after?: string | null;
  },
): Promise<OrgAuthPolicy> {
  return sendJson<OrgAuthPolicy>("PUT", `/api/v1/orgs/${orgId}/auth-policy`, input);
}
