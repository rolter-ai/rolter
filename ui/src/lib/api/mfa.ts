import { getJson, sendJson } from "./request";

/** how hard the org insists on a second factor; `off` is the default */
export const MFA_POLICIES = ["off", "optional", "required_superadmin", "required_all"] as const;

export type MfaPolicy = (typeof MFA_POLICIES)[number];

/** `MfaStatus` — what this account's factor looks like right now */
export interface MfaStatus {
  /** an armed factor stands between this account's password and a session */
  enabled: boolean;
  /** a secret was issued and never proved; it gates nothing */
  enrolment_pending: boolean;
  /** unspent recovery codes; `0` with `enabled` is one lost phone from break-glass */
  recovery_codes_remaining: number;
  /** the strictest policy across the account's orgs */
  policy: MfaPolicy;
  /** whether that policy makes the factor mandatory for this account */
  required: boolean;
  /**
   * While the requirement is announced but not in force: when it starts
   * (#1852). `null` when nothing is required, or it already applies.
   */
  enforce_after: string | null;
}

/**
 * The shared secret, returned exactly once.
 *
 * No read path hands it back: the control plane seals it on the way in, so a
 * user who navigates away mid-enrolment starts over rather than recovering it.
 */
export interface MfaEnrolment {
  /** what the QR encodes; carries the issuer, the account and the secret */
  otpauth_uri: string;
  /** the same secret in base32, for typing in when there is no camera */
  secret: string;
  digits: number;
  period: number;
}

/** the ten single-use codes, also shown exactly once */
export interface RecoveryCodes {
  recovery_codes: string[];
}

export function fetchMfaStatus(): Promise<MfaStatus> {
  return getJson<MfaStatus>("/api/v1/me/mfa");
}

export function beginMfaEnrolment(): Promise<MfaEnrolment> {
  return sendJson<MfaEnrolment>("POST", "/api/v1/me/mfa/enroll");
}

/**
 * Arm the factor by proving a code from the issued secret.
 *
 * The code spent here cannot be spent again, so the next sign-in needs the
 * following one — the replay rule, not a fault. The dashboard says so rather
 * than letting it read as a broken enrolment.
 */
export function confirmMfaEnrolment(code: string): Promise<RecoveryCodes> {
  return sendJson<RecoveryCodes>("POST", "/api/v1/me/mfa/confirm", { code });
}

/** replaces the whole batch; every code from the previous one stops working */
export function regenerateRecoveryCodes(): Promise<RecoveryCodes> {
  return sendJson<RecoveryCodes>("POST", "/api/v1/me/mfa/recovery-codes");
}

/**
 * Remove the factor. A current code (or an unspent recovery code) comes with
 * it, so a hijacked session cannot strip the factor it could not get past.
 *
 * Answers 403 when an org policy makes the factor mandatory — not a wrong
 * code, and not something the user can fix themselves.
 */
export function disableMfa(code: string): Promise<void> {
  return sendJson<void>("DELETE", "/api/v1/me/mfa", { code });
}

// --- invitations (crates/rolter-control/src/invitations.rs, #712) ---

// --- org enrolment roster (crates/rolter-control/src/mfa.rs, #1590) ---

/**
 * One active member of an org, as `GET /api/v1/orgs/{id}/mfa-status` lists
 * them (`org_auth_policy:read`). A person is one row however many grants they
 * hold. `recovery_codes_remaining` is a count, never a code.
 */
export interface OrgMemberMfa {
  user_id: string;
  email: string;
  is_superadmin: boolean;
  /** an armed (confirmed) factor; a half-finished enrolment is not one */
  enabled: boolean;
  recovery_codes_remaining: number;
}

export function fetchOrgMfaStatus(orgId: string): Promise<OrgMemberMfa[]> {
  return getJson<OrgMemberMfa[]>(`/api/v1/orgs/${orgId}/mfa-status`);
}
