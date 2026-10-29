/**
 * The refusal codes a single sign-on callback answers with (`SsoFailure` in
 * `crates/rolter-control/src/sso.rs`): in `?sso_error=` when a browser is sent
 * back to the login page, and as `error.code` to a JSON caller (#3468).
 *
 * Both paths word a code through the one catalog entry,
 * `errors.api.codes.<code>` (#3524), so the same failure reads the same
 * wherever it surfaces and a new code is worded once per catalog.
 * `ui/scripts/sso-failure-codes.test.ts` holds this list to the control
 * plane's, to `KNOWN_ERROR_CODES` and to every catalog.
 */
export const SSO_FAILURES = [
  "idp_error",
  "state_expired",
  "sso_disabled",
  "unknown_provider",
  "no_mapped_group",
  "account_deactivated",
  "account_outside_org",
  "idp_verification_failed",
  "not_configured",
  "internal_error",
] as const;

/**
 * The SSO codes `describeError` does not translate on its own, although their
 * sentence is in `errors.api.codes` with the rest. Their names say nothing of
 * single sign-on, and `describeError` would word any endpoint's
 * `error.code` of that name with the SSO sentence: today only the SSO
 * callback sends them as a JSON `error.code`, but the MCP consent flow already
 * uses both as redirect `reason=` codes with copy of its own
 * (`CONSENT_FAILURES` in `mcp-consent.ts`), so a generic name is kept from
 * reading as an SSO failure wherever it arrives. A JSON refusal carrying one is
 * worded by its status; the login page knows the code came from single
 * sign-on and words it.
 */
export const SSO_SHARED_NAMES = ["not_configured", "internal_error"] as const;

export type SsoFailure = (typeof SSO_FAILURES)[number];

/**
 * The catalog key for an SSO failure code. A code this dashboard has no
 * wording for is still an SSO failure, not a blank screen, so it reads as the
 * generic one.
 */
export function ssoFailureKey(code: string): `errors.api.codes.${SsoFailure}` {
  const known = (SSO_FAILURES as readonly string[]).includes(code)
    ? (code as SsoFailure)
    : "internal_error";
  return `errors.api.codes.${known}`;
}
