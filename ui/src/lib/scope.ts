import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import {
  fetchOrgs,
  fetchProjects,
  fetchTeams,
  type MeMembership,
  type OrgRow,
  type ProjectRow,
  type TeamRow,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";

// persisted, user-selectable org/team/project scope. the lists come from the
// control plane already narrowed to what the account can reach (#1846), so a
// pick is always one of the account's own scopes; the server still decides
// what each screen may read there
const STORAGE_KEY = "rolter.scope";

interface StoredScope {
  orgId?: string;
  teamId?: string;
  projectId?: string;
}

function readStored(): StoredScope {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as StoredScope;
  } catch {
    return {};
  }
}

function writeStored(scope: StoredScope) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(scope));
  } catch {
    // localStorage unavailable (private mode, quota, ...) — scope just
    // won't persist across reloads, not worth surfacing to the user
  }
}

// every mounted `useScope` keeps its own copy of the selection, so a pick made
// in the switcher used to reach only the switcher: the components that were
// already mounted — the shell, and since #1183 the capability query that has to
// re-ask when the org changes — kept the previous scope until they remounted.
// the write is broadcast instead
const listeners = new Set<(scope: StoredScope) => void>();

/** The org, team and project of the account's most specific membership. */
function ownScope(
  memberships: MeMembership[] | undefined,
): { orgId?: string; teamId?: string; projectId?: string } | undefined {
  const depth = (m: MeMembership) => (m.project_id ? 2 : m.team_id ? 1 : 0);
  const best = memberships?.reduce<MeMembership | undefined>(
    (pick, m) => (pick && depth(pick) >= depth(m) ? pick : m),
    undefined,
  );
  if (!best) return undefined;
  return {
    orgId: best.org_id ?? best.scope_org_id ?? undefined,
    teamId: best.team_id ?? best.scope_team_id ?? undefined,
    projectId: best.project_id ?? undefined,
  };
}

export interface ScopeResult {
  orgId?: string;
  teamId?: string;
  projectId?: string;
  orgs: OrgRow[];
  teams: TeamRow[];
  projects: ProjectRow[];
  setOrgId: (id: string) => void;
  setTeamId: (id: string) => void;
  setProjectId: (id: string) => void;
  isLoading: boolean;
  /** catalog key for why the scope is unusable — the caller runs it through `t` */
  errorKey?: string;
}

export function useScope(): ScopeResult {
  const [stored, setStored] = React.useState<StoredScope>(() => readStored());
  React.useEffect(() => {
    listeners.add(setStored);
    return () => {
      listeners.delete(setStored);
    };
  }, []);

  const orgs = useQuery({ queryKey: ["scope", "orgs"], queryFn: fetchOrgs });
  // the account's own scope, from /auth/me: an org it is a member of (#1196),
  // else the org, team and project of its most specific membership, so a
  // project member lands on their project rather than on whatever each list
  // happens to start with (#1846). optional because scope is also read outside
  // a session — and outside the provider entirely, in stories
  const memberships = useOptionalAuth()?.memberships;
  const own = ownScope(memberships);
  const memberOrgId = memberships?.find((m) => m.org_id)?.org_id ?? own?.orgId;
  // prefer the stored id if it still exists in the fetched list, then the org
  // the account belongs to, and only then the first org the control plane
  // happened to return — this also self-heals a stale stored id (e.g. the org
  // was deleted from another session)
  const orgId =
    (stored.orgId && orgs.data?.some((o) => o.id === stored.orgId) ? stored.orgId : undefined) ??
    (memberOrgId && orgs.data?.some((o) => o.id === memberOrgId) ? memberOrgId : undefined) ??
    orgs.data?.[0]?.id;

  const teams = useQuery({
    queryKey: ["scope", "teams", orgId],
    queryFn: () => fetchTeams(orgId as string),
    enabled: !!orgId,
  });
  const teamId =
    (stored.teamId && teams.data?.some((t) => t.id === stored.teamId)
      ? stored.teamId
      : undefined) ??
    (own?.orgId === orgId && own?.teamId && teams.data?.some((t) => t.id === own.teamId)
      ? own.teamId
      : undefined) ??
    teams.data?.[0]?.id;

  const projects = useQuery({
    queryKey: ["scope", "projects", teamId],
    queryFn: () => fetchProjects(teamId as string),
    enabled: !!teamId,
  });
  const projectId =
    (stored.projectId && projects.data?.some((p) => p.id === stored.projectId)
      ? stored.projectId
      : undefined) ??
    (own?.teamId === teamId && own?.projectId && projects.data?.some((p) => p.id === own.projectId)
      ? own.projectId
      : undefined) ??
    projects.data?.[0]?.id;

  const persist = React.useCallback((next: StoredScope) => {
    writeStored(next);
    for (const listener of listeners) listener(next);
  }, []);

  const setOrgId = React.useCallback(
    (id: string) => {
      // switching org resets team/project so we don't carry a mismatched pick
      persist({ orgId: id });
    },
    [persist],
  );

  const setTeamId = React.useCallback(
    (id: string) => {
      persist({ orgId, teamId: id });
    },
    [persist, orgId],
  );

  const setProjectId = React.useCallback(
    (id: string) => {
      persist({ orgId, teamId, projectId: id });
    },
    [persist, orgId, teamId],
  );

  const isLoading = orgs.isLoading || teams.isLoading || projects.isLoading;

  // the hook has no `t` of its own — it is called from contexts that are not
  // components — so it names the catalog key and the caller translates it
  let errorKey: string | undefined;
  if (!isLoading) {
    if (orgs.error) errorKey = "scope.errors.orgsFailed";
    else if (!orgId) errorKey = "scope.errors.noOrg";
    else if (teams.error) errorKey = "scope.errors.teamsFailed";
    else if (!teamId) errorKey = "scope.errors.noTeam";
    else if (projects.error) errorKey = "scope.errors.projectsFailed";
    else if (!projectId) errorKey = "scope.errors.noProject";
  }

  return {
    orgId,
    teamId,
    projectId,
    orgs: orgs.data ?? [],
    teams: teams.data ?? [],
    projects: projects.data ?? [],
    setOrgId,
    setTeamId,
    setProjectId,
    isLoading,
    errorKey,
  };
}
