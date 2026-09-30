import type { MembershipRow, OrgAuthPolicy, SsoProviderRow } from "@/lib/api";

/**
 * How many people a list of membership rows stands for.
 *
 * `GET /api/v1/orgs/{id}/memberships` answers with one row per grant anywhere
 * in the org's tree, so a person holding a role on the org and another on one
 * of its teams is two rows. A count that reads "this organization has N
 * members" has to count them once.
 */
export function distinctPeople(rows: Pick<MembershipRow, "user_id">[]): number {
  return new Set(rows.map((row) => row.user_id)).size;
}

/**
 * Whether taking `target` out of service, or deleting it, leaves the org with
 * no way for a member to sign in.
 *
 * It does when password sign-in is off and `target` is the last enabled
 * provider. The control plane refuses the mirror change, turning passwords off
 * with no enabled provider (`set_policy` in `auth_policy.rs`), but accepts this
 * one, so the dashboard is the only place that can say so first.
 *
 * `policy` is the saved policy, not the draft on the policy card: an unsaved
 * switch changes nothing for anybody yet. A provider that is already disabled
 * is not a route members have, so switching it off again or deleting it
 * changes nothing either.
 */
export function locksOutMembers(
  providers: SsoProviderRow[],
  target: SsoProviderRow,
  policy: Pick<OrgAuthPolicy, "allow_password_login">,
): boolean {
  if (policy.allow_password_login || !target.enabled) return false;
  return !providers.some((provider) => provider.id !== target.id && provider.enabled);
}

export interface SecretGap {
  /** the enabled providers with no sealed client secret, in list order */
  missing: SsoProviderRow[];
  /** every enabled provider is in `missing`, so none can finish a sign-in */
  all: boolean;
}

/**
 * The enabled providers that hold no client secret.
 *
 * A provider that expects a secret and has none fails at the token exchange,
 * the moment its member is sent back from the identity provider. With password
 * sign-in on that member can still use a password; with it off there is
 * nothing to fall back to.
 */
export function secretGap(providers: SsoProviderRow[]): SecretGap {
  const enabled = providers.filter((provider) => provider.enabled);
  const missing = enabled.filter((provider) => !provider.has_client_secret);
  return { missing, all: missing.length > 0 && missing.length === enabled.length };
}
