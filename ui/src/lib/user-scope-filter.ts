import type { MembershipRow, OrgProjectRow, TeamRow } from "@/lib/api";
import { teamPeople } from "@/lib/team-people";

/** The scope the Users list is narrowed to: one team, or one project (#3127). */
export interface UserScopeFilter {
  kind: "team" | "project";
  id: string;
}

/** the query parameters a link sets, so a team's sheet can land on its people */
export const TEAM_PARAM = "team";
export const PROJECT_PARAM = "project";

/** The filter a URL carries; a project wins over a team when a link names both. */
export function readUserScopeFilter(params: URLSearchParams): UserScopeFilter | null {
  const project = params.get(PROJECT_PARAM)?.trim();
  if (project) return { kind: "project", id: project };
  const team = params.get(TEAM_PARAM)?.trim();
  return team ? { kind: "team", id: team } : null;
}

/** The URL after the filter changes, leaving every other parameter alone. */
export function writeUserScopeFilter(
  params: URLSearchParams,
  filter: UserScopeFilter | null,
): URLSearchParams {
  const next = new URLSearchParams(params);
  next.delete(TEAM_PARAM);
  next.delete(PROJECT_PARAM);
  if (filter) next.set(filter.kind === "team" ? TEAM_PARAM : PROJECT_PARAM, filter.id);
  return next;
}

/** The combobox value for a filter: the kind keeps a team and a project id apart. */
export const filterValue = (filter: UserScopeFilter | null): string =>
  filter ? `${filter.kind}:${filter.id}` : "";

export function parseFilterValue(value: string): UserScopeFilter | null {
  const at = value.indexOf(":");
  const kind = value.slice(0, at);
  const id = value.slice(at + 1);
  return (kind === "team" || kind === "project") && id ? { kind, id } : null;
}

/** Whether the org has the team or project the filter names. */
export function scopeExists(
  filter: UserScopeFilter,
  teams: TeamRow[],
  projects: OrgProjectRow[],
): boolean {
  return filter.kind === "team"
    ? teams.some((team) => team.id === filter.id)
    : projects.some((project) => project.id === filter.id);
}

/**
 * The accounts holding a role on the scope. A team counts the people of its
 * projects too, like the Teams card does; org-wide grants reach every team and
 * belong to none.
 */
export function usersInScope(
  filter: UserScopeFilter,
  memberships: MembershipRow[],
  projects: OrgProjectRow[],
): Set<string> {
  if (filter.kind === "project") {
    return new Set(memberships.filter((m) => m.project_id === filter.id).map((m) => m.user_id));
  }
  const own = projects.filter((p) => p.team_id === filter.id).map((p) => p.id);
  return new Set(teamPeople(memberships, filter.id, own).people);
}
