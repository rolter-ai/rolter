import type { TFunction } from "i18next";

import { ROLES, type MeMembership } from "@/lib/api";
import { roleLabel } from "@/lib/roles";

/** where in the org → team → project chain a grant was made */
export type GrantLevel = "org" | "team" | "project";

/**
 * What the signed-in account is, in the scope the dashboard is looking at.
 *
 * - `superadmin`: the deployment-wide authority, which no membership narrows
 * - `grant`: the role the account holds here and the level it was granted at
 * - `role`: a role with no grant that covers this scope (the scope names an org
 *   the account holds only a sibling grant in); the role alone is all that is
 *   true to say
 * - `unknown`: nothing is known (an email-only open-mode session, or `/auth/me`
 *   has not answered)
 */
export type AccountRole =
  | { kind: "superadmin" }
  | { kind: "grant"; role: string; level: GrantLevel }
  | { kind: "role"; role: string }
  | { kind: "unknown" };

export interface ScopeIds {
  orgId?: string;
  teamId?: string;
  projectId?: string;
}

const LEVEL_DEPTH: Record<GrantLevel, number> = { org: 0, team: 1, project: 2 };

/** the level a membership was granted at, read off which of its ids is set */
export function grantLevel(m: MeMembership): GrantLevel | null {
  if (m.project_id) return "project";
  if (m.team_id) return "team";
  if (m.org_id) return "org";
  return null;
}

/** a role's rank, so the strongest grant wins; one this build has no name for ranks last */
function rank(role: string): number {
  const at = (ROLES as readonly string[]).indexOf(role);
  return at < 0 ? -1 : ROLES.length - at;
}

/** whether the grant reaches the scope in view: the same org, team or project */
function covers(m: MeMembership, level: GrantLevel, scope: ScopeIds): boolean {
  if (level === "project") return !!scope.projectId && m.project_id === scope.projectId;
  if (level === "team") return !!scope.teamId && m.team_id === scope.teamId;
  return !!scope.orgId && m.org_id === scope.orgId;
}

/**
 * The account's role where the dashboard is looking.
 *
 * A superadmin is one thing everywhere. Anyone else holds grants at an org, a
 * team or a project, and the one that explains what they can do in the current
 * scope is the strongest of those that reach it; where two are equally strong
 * the broader one is named, since it is the one that would still apply if the
 * narrower one went away. Before this the line read "Admin · acme" for a
 * project admin and a superadmin alike (#2805).
 */
export function accountRole(
  user: { is_superadmin: boolean } | null,
  memberships: MeMembership[],
  scope: ScopeIds,
): AccountRole {
  if (user?.is_superadmin) return { kind: "superadmin" };
  if (!user) return { kind: "unknown" };
  const reaching = memberships
    .flatMap((m) => {
      const level = grantLevel(m);
      return level && covers(m, level, scope) ? [{ role: m.role, level }] : [];
    })
    .sort((a, b) => rank(b.role) - rank(a.role) || LEVEL_DEPTH[a.level] - LEVEL_DEPTH[b.level]);
  const best = reaching[0];
  if (best) return { kind: "grant", ...best };
  const any = [...memberships].sort((a, b) => rank(b.role) - rank(a.role))[0];
  return any ? { kind: "role", role: any.role } : { kind: "unknown" };
}

/** the names of the org, team and project in view, for the line that names a grant's level */
export interface ScopeNames {
  org?: string;
  team?: string;
  project?: string;
}

/**
 * The role as a word: what the account card under the rail says.
 *
 * Short on purpose, since the card is one line wide; the scope is the header's
 * business, so the card does not repeat it.
 */
export function accountRoleName(t: TFunction, role: AccountRole): string {
  switch (role.kind) {
    case "superadmin":
      return t("shell.superadmin");
    case "grant":
    case "role":
      return roleLabel(t, role.role);
    default:
      return t("shell.role");
  }
}

/**
 * The role and where it applies: "Admin · org acme", "Member · team platform",
 * "Superadmin · whole deployment" (#2805).
 *
 * One catalog string per shape rather than a noun spliced into a sentence, so
 * a locale can decline the level the way its grammar wants.
 */
export function accountRoleLine(t: TFunction, role: AccountRole, names: ScopeNames): string {
  if (role.kind === "superadmin") return t("shell.roleLine.superadmin");
  if (role.kind === "grant") {
    return t(`shell.roleLine.${role.level}`, {
      role: roleLabel(t, role.role),
      name: names[role.level] ?? "",
    });
  }
  return accountRoleName(t, role);
}
