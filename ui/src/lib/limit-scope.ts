// the chain a budget or rate-limit row is gated at (#2529). a cap names the
// scope it throttles (scope_type + scope_id), and the control plane checks the
// write at exactly that scope, so a project admin may write their project's
// caps and its keys', never the team's or the org's above it

import type { RowScope } from "@/lib/can";

/** the two fields a budget and a rate limit both carry */
export interface CapRow {
  scope_type: string;
  scope_id: string;
}

export interface CapGateContext {
  /** every team with its projects, as `useOrgScope` lists them */
  byTeam: { team: { id: string }; projects: { id: string }[] }[];
  /** the project the page lists virtual keys of, which is the project a key cap sits under */
  keyProjectId?: string | null;
}

/**
 * The chain one cap row's Edit and Delete are asked at.
 *
 * An org cap is asked at the org alone, a team cap at org + team, a project cap
 * at org + team + project. A virtual-key cap is asked at the project the page
 * lists keys of. `undefined` keeps the page's answer: a business unit or a
 * customer (no project chain to map them to), and a project or key the
 * dashboard cannot place — the server's 403 stays the backstop.
 */
export function capGateScope(row: CapRow, ctx: CapGateContext): RowScope | undefined {
  switch (row.scope_type) {
    case "org":
      return { projectId: null };
    case "team":
      return { projectId: null, teamId: row.scope_id };
    case "project":
      return projectChain(row.scope_id, ctx.byTeam);
    case "virtual_key":
      return ctx.keyProjectId ? projectChain(ctx.keyProjectId, ctx.byTeam) : undefined;
    default:
      return undefined;
  }
}

function projectChain(projectId: string, byTeam: CapGateContext["byTeam"]): RowScope | undefined {
  const owner = byTeam.find((entry) => entry.projects.some((p) => p.id === projectId));
  return owner ? { projectId, teamId: owner.team.id } : undefined;
}
