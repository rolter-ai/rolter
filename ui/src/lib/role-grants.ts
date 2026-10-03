import type { MembershipRow, MembershipScopeType } from "@/lib/api";

// what the users screen needs to say about a role grant before it changes one
// (#2053): where the grant sits, and which role applies at that scope once it
// is gone. the second half mirrors `resolve_role` in
// crates/rolter-control/src/rbac.rs — the most specific grant wins, and at one
// scope the higher role wins — so a confirmation can say what a person keeps
// instead of guessing

type ScopeIds = Pick<MembershipRow, "org_id" | "team_id" | "project_id">;

/** A grant's scope, by its most specific id — the way the control plane stores it. */
export function grantScope(grant: ScopeIds): { type: MembershipScopeType; id: string } {
  if (grant.project_id) return { type: "project", id: grant.project_id };
  if (grant.team_id) return { type: "team", id: grant.team_id };
  return { type: "org", id: grant.org_id ?? "" };
}

export function sameScope(a: ScopeIds, b: ScopeIds): boolean {
  const left = grantScope(a);
  const right = grantScope(b);
  return left.type === right.type && left.id === right.id;
}

/**
 * The body `POST /orgs/{org_id}/memberships` takes for a picked scope: the
 * project if one is named, else the team, else the org itself.
 */
export function membershipScope(
  picked: { team_id?: string; project_id?: string },
  orgId: string,
): { scope_type: MembershipScopeType; scope_id: string } {
  if (picked.project_id) return { scope_type: "project", scope_id: picked.project_id };
  if (picked.team_id) return { scope_type: "team", scope_id: picked.team_id };
  return { scope_type: "org", scope_id: orgId };
}

// `role_rank`; a role string this build does not know ranks below every role,
// and `resolve_role` skips it altogether
const RANK: Record<string, number> = { viewer: 0, member: 1, admin: 2 };

function rank(role: string): number | undefined {
  return RANK[role];
}

/** Of two roles held at one scope, the one that applies. */
export function higherRole(a: string, b: string): string {
  return (rank(b) ?? -1) > (rank(a) ?? -1) ? b : a;
}

interface ScopeChain {
  org: string;
  team?: string;
  project?: string;
}

/**
 * The org → team → project chain a grant's own scope sits in, or `undefined`
 * when a project grant's team is not known (its project list has not loaded or
 * failed) — a chain with the team missing would ignore the team grants that
 * reach the project and understate what the person keeps.
 */
function chainOf(
  grant: ScopeIds,
  orgId: string,
  teamOfProject: (projectId: string) => string | undefined,
): ScopeChain | undefined {
  if (grant.project_id) {
    const team = teamOfProject(grant.project_id);
    return team ? { org: orgId, team, project: grant.project_id } : undefined;
  }
  if (grant.team_id) return { org: orgId, team: grant.team_id };
  return { org: orgId };
}

// `scope_specificity`: how closely a grant matches the chain, by its most
// specific id, or undefined when it does not reach the chain at all
function specificity(grant: ScopeIds, chain: ScopeChain): number | undefined {
  if (grant.project_id) return grant.project_id === chain.project ? 3 : undefined;
  if (grant.team_id) return grant.team_id === chain.team ? 2 : undefined;
  if (grant.org_id) return grant.org_id === chain.org ? 1 : undefined;
  return undefined;
}

function resolveGrant(grants: MembershipRow[], chain: ScopeChain): MembershipRow | undefined {
  let best: { at: number; grant: MembershipRow } | undefined;
  for (const grant of grants) {
    const at = specificity(grant, chain);
    const role = rank(grant.role);
    if (at === undefined || role === undefined) continue;
    const held = best && rank(best.grant.role);
    if (!best || at > best.at || (at === best.at && role > (held ?? -1))) best = { at, grant };
  }
  return best?.grant;
}

export type AfterRevoke =
  /** no other grant of theirs reaches the revoked grant's scope */
  | { kind: "none" }
  /** this grant decides their role there once the revoked one is gone */
  | { kind: "fallback"; grant: MembershipRow }
  /** the scope's place in the org could not be worked out */
  | { kind: "unknown" };

/**
 * Which of a person's other grants decides their role at `target`'s scope once
 * `target` is revoked.
 *
 * `grants` are all of that person's grants in the org, `target` among them.
 * The answer can be a *wider* role than the one revoked: a project viewer who
 * is also an org admin is an admin on that project again once the narrower
 * grant goes, because the most specific grant is what the control plane reads.
 */
export function afterRevoke(
  target: MembershipRow,
  grants: MembershipRow[],
  orgId: string,
  teamOfProject: (projectId: string) => string | undefined,
): AfterRevoke {
  const chain = chainOf(target, orgId, teamOfProject);
  if (!chain) return { kind: "unknown" };
  const grant = resolveGrant(
    grants.filter((other) => other.id !== target.id),
    chain,
  );
  return grant ? { kind: "fallback", grant } : { kind: "none" };
}
