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
  /**
   * How emailing the link went (#1828): `sent`, `failed`, or `not_configured`
   * when the control plane has no mail relay. The link is returned to the admin
   * whichever it is.
   */
  mail_status: "not_configured" | "sent" | "failed";
  mail_attempted_at: string | null;
  /** why a `failed` send failed, in words that are safe to show */
  mail_error: string | null;
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

/**
 * Email a pending invitation's link again (#3304). It is the same link, so the
 * earlier message still works. The answer is the invitation with its mail
 * outcome updated; `409` is an invitation that is no longer pending, has no
 * kept link, or a control plane with no relay.
 */
export function resendInvitation(id: string): Promise<Invitation> {
  return sendJson<Invitation>("POST", `/api/v1/invitations/${id}/resend`);
}

/**
 * What a relay check answers (#3304): `ok`, or the `stage` that failed and a
 * `reason` that names no address or credential. `409` is a control plane with
 * no mail relay, and `429` a check run again within a few seconds (#3394).
 */
export interface MailRelayCheck {
  ok: boolean;
  stage?: "connect" | "tls" | "auth" | "relay";
  reason?: string;
}

/** connect to the relay, negotiate TLS and authenticate, sending nothing */
export function checkMailRelay(orgId: string): Promise<MailRelayCheck> {
  return sendJson<MailRelayCheck>("POST", `/api/v1/orgs/${orgId}/invitations/mail-check`);
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
