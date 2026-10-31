// users and memberships (crates/rolter-control/src/crud/users.rs and memberships.rs, ROL-223)
import type { DeploymentRole } from "./rbac";
import { getJson, sendJson } from "./request";

export const ROLES = ["admin", "member", "viewer"] as const;

export type Role = (typeof ROLES)[number];

// scope types a membership can be granted at (virtual_key is not a role scope)
export const MEMBERSHIP_SCOPE_TYPES = ["org", "team", "project"] as const;

export type MembershipScopeType = (typeof MEMBERSHIP_SCOPE_TYPES)[number];

export interface UserRow {
  id: string;
  email: string;
  /** what the account goes by; null until set (or provisioned by an IdP) */
  display_name?: string | null;
  /** a line on who to ask about what; null until set */
  bio?: string | null;
  is_superadmin: boolean;
  /** set when the account is deactivated (login blocked); null when active */
  deactivated_at?: string | null;
  created_at: string;
  /**
   * The delegable deployment role the account holds (#1834), or none. It
   * reaches deployment-wide screens only and grants nothing inside an org.
   */
  deployment_role?: DeploymentRole | null;
}

export interface MembershipRow {
  id: string;
  user_id: string;
  org_id?: string | null;
  team_id?: string | null;
  project_id?: string | null;
  role: string;
  /**
   * Who granted it: `manual` (invitation, seed, admin api), `sso` (an IdP
   * group mapping, recomputed at each sign-in) or `scim` (a pushed group,
   * reconciled on each sync). A grant from the IdP comes back after a revoke
   * unless its mapping changes, which the users screen says before revoking.
   */
  source?: string;
  created_at: string;
}

// returned by inviteUser: the new account plus its initial org membership
export interface CreatedUser {
  user: UserRow;
  membership: MembershipRow;
}

export interface InviteUserInput {
  email: string;
  /** optional initial password; omit for an sso-only shell account */
  password?: string;
  /** role granted at the org; defaults to member */
  role?: string;
}

export interface UpdateUserInput {
  email?: string;
  password?: string;
  is_superadmin?: boolean;
  deactivated?: boolean;
}

export interface CreateMembershipInput {
  user_id: string;
  scope_type: MembershipScopeType;
  scope_id: string;
  role: string;
}

// every account with a membership anywhere in the org's tree. a superadmin also
// asks for the accounts that hold no membership at all, such as the first
// administrator `rolter-seed --admin-email` creates, so they can find their own
// row to edit (#2804); the control plane ignores the flag for anyone else
export function fetchUsers(
  orgId: string,
  { includeUnassigned = false }: { includeUnassigned?: boolean } = {},
): Promise<UserRow[]> {
  const query = includeUnassigned ? "?include_unassigned=true" : "";
  return getJson<UserRow[]>(`/api/v1/orgs/${orgId}/users${query}`);
}

// every account in the deployment, whichever org it belongs to, email-ordered
// and deactivated ones included. superadmin-only (`deployment_user:read`), a
// 403 for anyone else, so a caller asks only on an explicit yes from the gate.
// it is a separate read from the org's list: that one is reached through
// memberships, so an actor who belongs only to another org is not on it (#2871)
export function fetchDeploymentUsers(): Promise<UserRow[]> {
  return getJson<UserRow[]>("/api/v1/users");
}

// create/invite an account and grant it a role in the org atomically
export function inviteUser(orgId: string, input: InviteUserInput): Promise<CreatedUser> {
  return sendJson<CreatedUser>("POST", `/api/v1/orgs/${orgId}/users`, input);
}

/**
 * Give an account a deployment role, or `null` to clear it. Superadmin only,
 * and deliberately not a field of `updateUser`: neither role can reach it, so
 * an operator cannot widen their own reach.
 */
export function setDeploymentRole(id: string, role: DeploymentRole | null): Promise<UserRow> {
  return sendJson<UserRow>("PUT", `/api/v1/users/${id}/deployment-role`, { role });
}

export function updateUser(id: string, input: UpdateUserInput): Promise<UserRow> {
  return sendJson<UserRow>("PUT", `/api/v1/users/${id}`, input);
}

export function deleteUser(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/users/${id}`);
}

// every role grant scoped within the org (org/team/project)
export function fetchMemberships(orgId: string): Promise<MembershipRow[]> {
  return getJson<MembershipRow[]>(`/api/v1/orgs/${orgId}/memberships`);
}

export function createMembership(
  orgId: string,
  input: CreateMembershipInput,
): Promise<MembershipRow> {
  return sendJson<MembershipRow>("POST", `/api/v1/orgs/${orgId}/memberships`, input);
}

// change a grant's role in one request; one `membership.update` audit row
export function updateMembership(id: string, role: string): Promise<MembershipRow> {
  return sendJson<MembershipRow>("PUT", `/api/v1/memberships/${id}`, { role });
}

export function deleteMembership(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/memberships/${id}`);
}

// --- administrator password reset (crates/rolter-control/src/password_reset.rs, #2240) ---

export interface CreatedPasswordReset {
  /** shown once; the server keeps only its digest */
  token: string;
  /** ready-made link the member opens to choose a new password */
  reset_url: string;
  /** how many of the member's sessions the reset ended */
  sessions_revoked: number;
  reset: { id: string; user_id: string; expires_at: string };
}

/**
 * Issue a one-time reset link for a member and end their sessions. The caller
 * never sees or picks a password; the member chooses theirs at the link.
 */
export function issuePasswordReset(orgId: string, userId: string): Promise<CreatedPasswordReset> {
  return sendJson<CreatedPasswordReset>(
    "POST",
    `/api/v1/orgs/${orgId}/users/${userId}/password-reset`,
  );
}

export interface PasswordResetPreview {
  email: string;
  expires_at: string;
}

// unauthenticated: the member has no session, the token is the credential
export function previewPasswordReset(token: string): Promise<PasswordResetPreview> {
  return getJson<PasswordResetPreview>(`/api/v1/password-reset/${encodeURIComponent(token)}`);
}

// stores the password and signs nobody in: the member signs in as usual, so an
// armed second factor still applies
export function redeemPasswordReset(
  token: string,
  password: string,
): Promise<{ email: string; sign_in_required: true }> {
  return sendJson<{ email: string; sign_in_required: true }>(
    "POST",
    `/api/v1/password-reset/${encodeURIComponent(token)}`,
    { password },
  );
}

// --- the rbac capability matrix (crates/rolter-control/src/rbac_matrix.rs) ---
//
// `CAPABILITIES` is the one table that backs the guard *and* this endpoint, so
// what the dashboard renders is by construction what the control plane
// enforces. The screen used to draw a hardcoded 3x7 matrix that had drifted
// away from the real 50-odd resources (#1178).
