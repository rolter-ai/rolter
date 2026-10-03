// which providers a route or a group may use, given the project each is scoped
// to (#1919). the control plane enforces the same rule with a 409; this is the
// early answer, so a picker never offers a choice the save will refuse

import type { RowScope } from "@/lib/can";

/** anything that carries the optional project scope a provider or group has */
export interface Scoped {
  project_id?: string | null;
}

/**
 * The providers an owner scoped to `owner` may use.
 *
 * A route in project P, or a group scoped to P, may use P's own providers and
 * org-wide ones. An org-wide group (`owner` null or absent) may use org-wide
 * providers only, since a scoped member would be reachable from every project
 * through it.
 */
export function providersUsableFrom<T extends Scoped>(
  providers: T[],
  owner: string | null | undefined,
): T[] {
  return providers.filter((p) => !p.project_id || p.project_id === owner);
}

/** whether one provider may be used by an owner scoped to `owner` */
export function usableFrom(provider: Scoped | undefined, owner: string | null | undefined) {
  return !provider || !provider.project_id || provider.project_id === owner;
}

/**
 * The chain a provider or group row's actions are gated at (#2522).
 *
 * An org-wide row is gated at the org; a project row at its own project, with
 * the team the project sits under since the guard walks org + team + project.
 * `undefined` when the project is not in `byTeam` (deleted, or one this account
 * cannot list): the row then keeps the page's answer and the server's 403
 * stays the backstop.
 */
export function rowGateScope(
  row: Scoped,
  byTeam: { team: { id: string }; projects: { id: string }[] }[],
): RowScope | undefined {
  if (!row.project_id) return { projectId: null };
  const owner = byTeam.find((entry) => entry.projects.some((p) => p.id === row.project_id));
  return owner ? { projectId: row.project_id, teamId: owner.team.id } : undefined;
}
