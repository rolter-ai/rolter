import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import {
  fetchOrgs,
  fetchProjects,
  fetchTeams,
  type OrgRow,
  type ProjectRow,
  type TeamRow,
} from "@/lib/api";

// persisted, user-selectable org/team/project scope. auth is still a
// client-side email gate only (see lib/auth.tsx), and there's no RBAC
// backend yet (Phase 3 in TODO.md), so this is scope selection, not
// permission enforcement — every signed-in user can see/pick any org.
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
  error?: string;
}

export function useScope(): ScopeResult {
  const [stored, setStored] = React.useState<StoredScope>(() => readStored());

  const orgs = useQuery({ queryKey: ["scope", "orgs"], queryFn: fetchOrgs });
  // prefer the stored id if it still exists in the fetched list, otherwise
  // fall back to the first org — this also self-heals a stale stored id
  // (e.g. the org was deleted from another session)
  const orgId =
    (stored.orgId && orgs.data?.some((o) => o.id === stored.orgId)
      ? stored.orgId
      : undefined) ?? orgs.data?.[0]?.id;

  const teams = useQuery({
    queryKey: ["scope", "teams", orgId],
    queryFn: () => fetchTeams(orgId as string),
    enabled: !!orgId,
  });
  const teamId =
    (stored.teamId && teams.data?.some((t) => t.id === stored.teamId)
      ? stored.teamId
      : undefined) ?? teams.data?.[0]?.id;

  const projects = useQuery({
    queryKey: ["scope", "projects", teamId],
    queryFn: () => fetchProjects(teamId as string),
    enabled: !!teamId,
  });
  const projectId =
    (stored.projectId && projects.data?.some((p) => p.id === stored.projectId)
      ? stored.projectId
      : undefined) ?? projects.data?.[0]?.id;

  const persist = React.useCallback((next: StoredScope) => {
    setStored(next);
    writeStored(next);
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

  let error: string | undefined;
  if (!isLoading) {
    if (orgs.error) error = "failed to load orgs";
    else if (!orgId) error = "no org configured — create one to get started";
    else if (teams.error) error = "failed to load teams";
    else if (!teamId) error = "no team configured — create one to get started";
    else if (projects.error) error = "failed to load projects";
    else if (!projectId)
      error = "no project configured — create one to get started";
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
    error,
  };
}
