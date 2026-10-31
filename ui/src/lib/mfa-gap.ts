import type { MfaPolicy, OrgMemberMfa } from "@/lib/api";

/**
 * Who a second-factor policy would ask to enrol: how many of the members it
 * binds have no armed factor, out of how many it binds.
 *
 * `required_superadmin` binds the superadmin accounts only
 * (`is_superadmin` rows of the roster), `required_all` binds every member.
 * `off` and `optional` bind nobody, so they have no gap to report.
 */
export function mfaGap(
  roster: OrgMemberMfa[],
  policy: MfaPolicy,
): { missing: number; total: number } | null {
  if (policy !== "required_superadmin" && policy !== "required_all") return null;
  const bound = policy === "required_all" ? roster : roster.filter((row) => row.is_superadmin);
  return { missing: bound.filter((row) => !row.enabled).length, total: bound.length };
}
