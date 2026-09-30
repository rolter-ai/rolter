// the slug rule for an sso provider, mirrored so a bad value is caught in the
// sheet before the round trip (#2304).
//
// the rule is the store's check constraint `sso_providers_slug_charset`
// (crates/rolter-store/migrations/0047_sso_providers.sql) and the server
// refuses the same values with a 400 (`validate_slug` in
// crates/rolter-control/src/sso.rs). `sso-slug.test.ts` reads the migration, so
// the three cannot drift apart unnoticed.
//
// the slug is registered at the identity provider as part of the redirect uri,
// so nothing here rewrites what was typed: `suggestSsoSlug` only proposes a
// corrected value for the admin to retype.

/** the longest slug the store accepts, in characters */
export const SSO_SLUG_MAX = 63;

export const SSO_SLUG_PATTERN = /^[a-z0-9][a-z0-9-]{0,62}$/;

/**
 * Why a slug is refused.
 *
 * - `empty`: nothing typed yet, which is an unfinished form rather than a mistake
 * - `charset`: a character outside lowercase letters, digits and hyphens, or a
 *   leading hyphen
 * - `length`: made of allowed characters, but more than `SSO_SLUG_MAX` of them
 */
export type SsoSlugProblem = "empty" | "charset" | "length";

/** What is wrong with `slug` exactly as typed, or null when the server accepts it. */
export function ssoSlugProblem(slug: string): SsoSlugProblem | null {
  if (SSO_SLUG_PATTERN.test(slug)) return null;
  if (slug === "") return "empty";
  if (!/^[a-z0-9-]*$/.test(slug) || slug.startsWith("-")) return "charset";
  return "length";
}

/**
 * A slug the server would accept that looks like what was typed, or null when
 * there is nothing sensible to offer.
 *
 * Accents fold to their base letter, case folds down, and every run of other
 * characters becomes one hyphen: `Acme Okta` is `acme-okta`. A value that
 * reduces to nothing (`日本語`) or to itself gets no suggestion.
 */
export function suggestSsoSlug(slug: string): string | null {
  const suggestion = slug
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+/, "")
    .slice(0, SSO_SLUG_MAX)
    .replace(/-+$/, "");
  return suggestion !== slug && SSO_SLUG_PATTERN.test(suggestion) ? suggestion : null;
}
