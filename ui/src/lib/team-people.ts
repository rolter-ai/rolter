import type { MembershipRow } from "@/lib/api";

/** Who holds a role on a team, and on each of its projects. */
export interface TeamPeople {
  /** grants made on the team itself */
  teamGrants: MembershipRow[];
  /** grants made on one of the team's projects */
  projectGrants: MembershipRow[];
  /** distinct accounts across both, which is what a team's head count means */
  people: string[];
  /** distinct accounts holding admin on the team itself, in grant order */
  admins: string[];
}

const distinct = (ids: string[]) => [...new Set(ids)];

/**
 * Read a team's people out of the org's memberships.
 *
 * A role is granted at the org, a team or a project, so counting only the grants
 * made on the team leaves out everyone who works at project scope: a team whose
 * people all hold a project role would read as empty. Org-wide grants are not
 * counted, since they reach every team and belong to none.
 */
export function teamPeople(
  memberships: MembershipRow[],
  teamId: string,
  projectIds: string[],
): TeamPeople {
  const projects = new Set(projectIds);
  const teamGrants = memberships.filter((m) => m.team_id === teamId && !m.project_id);
  const projectGrants = memberships.filter((m) => !!m.project_id && projects.has(m.project_id));
  return {
    teamGrants,
    projectGrants,
    people: distinct([...teamGrants, ...projectGrants].map((m) => m.user_id)),
    admins: distinct(teamGrants.filter((m) => m.role === "admin").map((m) => m.user_id)),
  };
}
