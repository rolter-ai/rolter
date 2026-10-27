import { type LoginResponse } from "./auth";
import { getJson, sendJson } from "./request";
import { type Role } from "./users";

export interface Invitation {
  id: string;
  org_id: string;
  email: string;
  role: Role;
  team_id: string | null;
  project_id: string | null;
  invited_by: string | null;
  /**
   * The sender's address, resolved by the control plane whatever users the
   * caller may list (#2325). Null for the admin token, a deleted sender, and
   * the rows other endpoints return.
   */
  invited_by_email: string | null;
  expires_at: string;
  accepted_at: string | null;
  revoked_at: string | null;
  created_at: string;
}

export interface CreatedInvitation {
  invitation: Invitation;
  /** shown once; the server keeps only its digest */
  token: string;
  accept_url: string;
}

export function listInvitations(orgId: string): Promise<Invitation[]> {
  return getJson<Invitation[]>(`/api/v1/orgs/${orgId}/invitations`);
}

export function createInvitation(
  orgId: string,
  body: {
    email: string;
    role: Role;
    scope_type?: "org" | "team" | "project";
    scope_id?: string;
  },
): Promise<CreatedInvitation> {
  return sendJson<CreatedInvitation>("POST", `/api/v1/orgs/${orgId}/invitations`, body);
}

export function revokeInvitation(id: string): Promise<Invitation> {
  return sendJson<Invitation>("DELETE", `/api/v1/invitations/${id}`);
}

export interface InvitationPreview {
  org_name: string;
  email: string;
  role: Role;
  /** where the role applies; a team or project invitation also carries `scope_name` */
  scope_type: "org" | "team" | "project";
  scope_name?: string;
  expires_at: string;
  /**
   * An account already exists under this email. Accepting then adds the role
   * to it and asks for no password: the invitee signs in as usual afterwards,
   * because an invite link never signs anyone in to an existing account (#1935).
   */
  has_account: boolean;
}

/**
 * The invitation was accepted and its role granted, but no session came with
 * it: the invitee signs in through the normal sign-in (#1935).
 * `existing_account` — the email already had an account, whose own password
 * (and second factor) still apply. `second_factor` — a new account in an org
 * whose `required_*` policy is in force; the sign-in is where it enrols.
 */
export interface InvitationSignInRequired {
  sign_in_required: true;
  email: string;
  reason: "existing_account" | "second_factor";
}

export type AcceptInvitationOutcome = LoginResponse | InvitationSignInRequired;

export function isInvitationSignInRequired(
  outcome: AcceptInvitationOutcome,
): outcome is InvitationSignInRequired {
  return "sign_in_required" in outcome && outcome.sign_in_required === true;
}

// unauthenticated: the invitee has no account yet, the token is the credential
export function previewInvitation(token: string): Promise<InvitationPreview> {
  return getJson<InvitationPreview>(`/api/v1/invitations/accept/${encodeURIComponent(token)}`);
}

// `password` only for an invitation that creates the account; an existing
// account keeps its own and is sent to sign in with it
export function acceptInvitation(
  token: string,
  password?: string,
): Promise<AcceptInvitationOutcome> {
  return sendJson<AcceptInvitationOutcome>(
    "POST",
    `/api/v1/invitations/accept/${encodeURIComponent(token)}/accept`,
    password === undefined ? {} : { password },
  );
}
