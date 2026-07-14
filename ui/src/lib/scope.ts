import { useQuery } from "@tanstack/react-query";

import { fetchOrgs, fetchProjects, fetchTeams } from "@/lib/api";

// there is no per-session org/team/project selection in the control plane
// yet (auth is a client-side email gate only, see lib/auth.tsx) — until
// ROL adds real multi-tenant session scoping, the dashboard resolves scope
// by taking the first org/team/project, which matches the single-tenant
// `rolter-seed` bootstrap flow every deployment currently uses
export interface ScopeResult {
  orgId?: string;
  projectId?: string;
  isLoading: boolean;
  error?: string;
}

export function useScope(): ScopeResult {
  const orgs = useQuery({ queryKey: ["scope", "orgs"], queryFn: fetchOrgs });
  const orgId = orgs.data?.[0]?.id;

  const teams = useQuery({
    queryKey: ["scope", "teams", orgId],
    queryFn: () => fetchTeams(orgId as string),
    enabled: !!orgId,
  });
  const teamId = teams.data?.[0]?.id;

  const projects = useQuery({
    queryKey: ["scope", "projects", teamId],
    queryFn: () => fetchProjects(teamId as string),
    enabled: !!teamId,
  });
  const projectId = projects.data?.[0]?.id;

  const isLoading = orgs.isLoading || teams.isLoading || projects.isLoading;

  let error: string | undefined;
  if (!isLoading) {
    if (orgs.error) error = "failed to load orgs";
    else if (!orgId) error = "no org configured — run rolter-seed to bootstrap one";
    else if (teams.error) error = "failed to load teams";
    else if (!teamId) error = "no team configured — run rolter-seed to bootstrap one";
    else if (projects.error) error = "failed to load projects";
    else if (!projectId)
      error = "no project configured — run rolter-seed to bootstrap one";
  }

  return { orgId, projectId, isLoading, error };
}
