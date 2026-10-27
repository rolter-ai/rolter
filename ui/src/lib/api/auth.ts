import { type MfaEnrolment, type RecoveryCodes } from "./mfa";
import { getJson, request, sendJson } from "./request";

export interface LoginResponse {
  /** opaque bearer token; store it and send as Authorization: Bearer */
  token: string;
  expires_at: string;
  user: {
    id: string;
    email: string;
    is_superadmin: boolean;
    /// set when an admin deactivated the account; a non-null value blocks login
    deactivated_at: string | null;
    created_at: string;
  };
  /**
   * Present when an org requires a second factor this account has not set up
   * yet, and the org's grace window has not run out (#1852): from this moment
   * the next sign-in walks the account through enrolment before it gets a
   * session. Absent otherwise.
   */
  mfa_enrol_by?: string;
}

/**
 * The other half of `LoginOutcome`: the password was right, and the account
 * still owes a second factor (#1078).
 *
 * The control plane serialises the enum untagged, so the two branches are told
 * apart by a field rather than by a discriminator — `mfa_required` exists for
 * exactly that and is always `true` when it is present at all.
 */
export interface MfaChallenge {
  mfa_required: true;
  /** present this to `verifyMfaChallenge`; authenticates nothing on its own */
  mfa_token: string;
  expires_at: string;
  /** seconds from the response until `expires_at`; time the prompt from this */
  expires_in: number;
}

/**
 * The third branch of `LoginOutcome` (#1852): the password was right, the org
 * requires a second factor, and this account has none armed.
 *
 * The token is not a session and opens nothing a session does. It is good for
 * exactly two calls — `beginSignInEnrolment` for a secret and
 * `confirmSignInEnrolment` to prove it — and the second is what mints the
 * session.
 */
export interface MfaEnrolmentChallenge {
  mfa_enrolment_required: true;
  enrolment_token: string;
  expires_at: string;
  /** seconds from the response until `expires_at`; time the prompt from this */
  expires_in: number;
}

/**
 * When a challenge dies, on this browser's clock: the moment it arrived plus
 * the control plane's `expires_in`.
 *
 * Never `expires_at` compared with `Date.now()`. That compares the server's
 * clock with the laptop's, and a laptop running more than the challenge's
 * lifetime fast would expire every prompt the moment it appeared — under a
 * `required_*` policy, a member who could never get in (#1852).
 */
export function challengeDeadline(
  challenge: { expires_in: number },
  receivedAt: number = Date.now(),
): number {
  return receivedAt + challenge.expires_in * 1000;
}

/** what `POST /auth/login` answers: a session, or a challenge that mints one */
export type LoginOutcome = LoginResponse | MfaChallenge | MfaEnrolmentChallenge;

export function isMfaChallenge(outcome: LoginOutcome): outcome is MfaChallenge {
  return (outcome as MfaChallenge).mfa_required === true;
}

export function isMfaEnrolmentChallenge(outcome: LoginOutcome): outcome is MfaEnrolmentChallenge {
  return (outcome as MfaEnrolmentChallenge).mfa_enrolment_required === true;
}

// authenticate a local account. Answers a session token, a challenge when the
// account has an armed factor, or an enrolment challenge when an org requires a
// factor the account has not armed. Rejects (throws) on bad credentials, when
// local accounts aren't configured, and — as `mfa_enrolment_required` — when
// an org requires a factor and this control plane cannot enrol one.
export function login(email: string, password: string): Promise<LoginOutcome> {
  return sendJson<LoginOutcome>("POST", "/api/v1/auth/login", {
    email,
    password,
  });
}

/**
 * Redeem a challenge for a real session with a TOTP code or a recovery code.
 *
 * Unauthenticated by necessity: the caller has no session yet, which is the
 * whole point of the exchange. Every rejection is the same
 * `invalid_credentials` — a wrong code, an expired challenge and an exhausted
 * one are deliberately indistinguishable, so a guesser cannot tell "keep
 * going" from "start over".
 */
export function verifyMfaChallenge(mfaToken: string, code: string): Promise<LoginResponse> {
  return sendJson<LoginResponse>("POST", "/api/v1/auth/mfa/verify", {
    mfa_token: mfaToken,
    code,
  });
}

/**
 * Mint the secret a sign-in bound by a `required_*` policy enrols with (#1852).
 *
 * Unauthenticated like the step-up: the enrolment token in the body is the
 * only credential. Every call mints a fresh secret and replaces the pending
 * one, so it is asked for once per challenge. A dead token — expired, spent,
 * or the account enrolled meanwhile — answers 401 `invalid_credentials`, and
 * the way on is the password step again.
 */
export function beginSignInEnrolment(enrolmentToken: string): Promise<MfaEnrolment> {
  return sendJson<MfaEnrolment>("POST", "/api/v1/auth/mfa/enroll", {
    enrolment_token: enrolmentToken,
  });
}

/** a session minted by enrolling, and the recovery codes that enrolment issued */
export type EnrolledSignIn = LoginResponse & RecoveryCodes;

/**
 * Arm the factor with a code from the minted secret and sign in.
 *
 * A wrong code is a 400 and keeps the challenge; a dead one is a 401. The
 * code spent here cannot be spent again, the same replay rule as enrolling
 * from the account screen.
 */
export function confirmSignInEnrolment(
  enrolmentToken: string,
  code: string,
): Promise<EnrolledSignIn> {
  return sendJson<EnrolledSignIn>("POST", "/api/v1/auth/mfa/confirm", {
    enrolment_token: enrolmentToken,
    code,
  });
}

/** what `POST /auth/sso/exchange` answers: the login body plus the roles the sign-in granted */
export interface SsoExchangeResponse extends LoginResponse {
  granted_roles: unknown[];
}

/**
 * Redeem the one-time code a browser SSO sign-in ends with
 * (`/login?sso_code=…`) for a session.
 *
 * Sent with no `Authorization` header on purpose, and not through `sendJson`,
 * which attaches whatever token is stored: the caller is mid sign-in, and a
 * stale token must not ride along. The code is single use, so a caller must
 * not retry it. A spent, expired or unknown code is a `400` with
 * `code == "invalid_exchange_code"`.
 */
export async function exchangeSsoCode(code: string): Promise<SsoExchangeResponse> {
  return request<SsoExchangeResponse>("/auth/sso/exchange", {
    method: "POST",
    body: { code },
    auth: false,
  });
}

/** what the login screen may offer; see crates/rolter-control/src/auth_policy.rs */
export interface AuthMethods {
  /** render the email + password form */
  password: boolean;
  /** one entry per enabled identity provider; empty when no IdP is configured */
  sso: { slug: string; name: string; start_url: string }[];
}

// unauthenticated by necessity: read before anyone has a session, so the login
// screen knows whether this deployment uses passwords, sso, or both
export function getAuthMethods(): Promise<AuthMethods> {
  return getJson<AuthMethods>("/api/v1/auth/methods");
}

export function logout(): Promise<void> {
  return sendJson<void>("POST", "/api/v1/auth/logout");
}
