import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { ControlSkeleton } from "@/components/LoadingState";
import { Select } from "@/components/ui/select";
import {
  fetchProjects,
  fetchTeams,
  type ProjectRow,
  type TeamRow,
} from "@/lib/api";
import { cn } from "@/lib/utils";

/**
 * Picking any team or project in the org, not just the ones the scope switcher
 * currently has selected (#1249).
 *
 * `useScope()` is the switcher's own state: it lists every team in the org but
 * only the projects of the selected team, because that is the chain the switcher
 * walks. A control that *names* a scope — an SCIM or SSO group mapping, which
 * may grant anywhere inside its own org — needs the whole org instead, and
 * before this the only way to reach a project in another team was to move the
 * switcher first and come back.
 *
 * There is no org-wide projects endpoint (#1357), so the projects are
 * fanned out one query per team and grouped under the team they belong to —
 * two teams may each have a "prod", and an ungrouped flat list would offer the
 * operator two identical options.
 */

/** `""` is the org itself; anything else is `team:<id>` or `project:<id>` */
export type ScopeTarget = string;

export const ORG_TARGET: ScopeTarget = "";

export function teamTarget(id: string): ScopeTarget {
  return `team:${id}`;
}

export function projectTarget(id: string): ScopeTarget {
  return `project:${id}`;
}

/** the ids a `ScopeTarget` carries, as the mapping endpoints take them */
export function scopeTargetIds(target: ScopeTarget): {
  team_id?: string;
  project_id?: string;
} {
  const [kind, id] = target.split(":");
  return {
    team_id: kind === "team" ? id : undefined,
    project_id: kind === "project" ? id : undefined,
  };
}

export interface TeamProjects {
  team: TeamRow;
  projects: ProjectRow[];
}

export interface OrgScope {
  teams: TeamRow[];
  /** every team in the org with the projects under it, teams in list order */
  byTeam: TeamProjects[];
  isLoading: boolean;
  /** the teams request, or any of the per-team project requests, that failed */
  error: unknown;
  refetch: () => void;
  /**
   * The name a stored scope goes by, org-wide.
   *
   * `undefined` when neither id is set (the scope is the org itself) and the
   * raw id when it names something outside this org's chain — a project that
   * was deleted, or one this account cannot list — because showing the id is
   * still more than hiding the row's scope entirely.
   */
  nameFor: (scope: {
    team_id?: string | null;
    project_id?: string | null;
  }) => string | undefined;
}

// this key is `useScope()`'s as well, and a screen that mounts both gates its
// own skeleton on the scope query. so the picker subscribes without ever
// putting that query back in flight: `retryOnMount` in particular, because a
// failed team list would otherwise be retried the moment the panel mounts,
// which swaps the panel for the screen's skeleton, which unmounts the picker,
// which starts the retry again — a loop the operator sees as a frozen screen.
// an invalidation from a mutation still reaches both observers
const SHARED: {
  refetchOnMount: false;
  retryOnMount: false;
  retry: false;
} = { refetchOnMount: false, retryOnMount: false, retry: false };

/**
 * Every team in `orgId` and every project under those teams.
 *
 * Shares react-query's cache with `useScope()` — the teams query key is the
 * same — so a screen that mounts both pays for one teams request.
 */
export function useOrgScope(orgId: string | undefined): OrgScope {
  const teams = useQuery({
    queryKey: ["scope", "teams", orgId],
    queryFn: () => fetchTeams(orgId as string),
    enabled: !!orgId,
    ...SHARED,
  });

  const teamRows = teams.data ?? [];

  const projects = useQueries({
    queries: teamRows.map((team) => ({
      queryKey: ["scope", "projects", team.id],
      queryFn: () => fetchProjects(team.id),
      ...SHARED,
    })),
  });

  // plain values rather than memos: the fan-out is a handful of arrays, and a
  // memo over `useQueries` results would need the query data itself as its
  // dependency, which is a new array on every settle anyway
  const byTeam: TeamProjects[] = teamRows.map((team, i) => ({
    team,
    projects: projects[i]?.data ?? [],
  }));

  const nameFor = (scope: {
    team_id?: string | null;
    project_id?: string | null;
  }): string | undefined => {
    if (scope.project_id) {
      for (const entry of byTeam) {
        const project = entry.projects.find((p) => p.id === scope.project_id);
        if (project) return project.name;
      }
      return scope.project_id;
    }
    if (scope.team_id) {
      return (
        teamRows.find((team) => team.id === scope.team_id)?.name ?? scope.team_id
      );
    }
    return undefined;
  };

  return {
    teams: teamRows,
    byTeam,
    isLoading: teams.isLoading || projects.some((q) => q.isLoading),
    error: teams.error ?? projects.find((q) => q.error)?.error ?? null,
    refetch: () => {
      teams.refetch();
      for (const q of projects) q.refetch();
    },
    nameFor,
  };
}

/**
 * A select naming the org, any team in it, or any project in any of those
 * teams.
 *
 * The scope is part of a form row rather than a region of its own, so the three
 * states are control-sized: a `ControlSkeleton` while the fan-out is in flight,
 * a line under the select when the org has no teams to narrow to, and a
 * `LoadError` under a select that still offers the org — a failed team list
 * makes the narrower scopes unreachable, not the org-wide mapping the operator
 * was probably writing anyway.
 */
export function OrgScopePicker({
  orgId,
  value,
  onChange,
  label,
  className,
  disabled,
}: {
  orgId: string | undefined;
  value: ScopeTarget;
  onChange: (value: ScopeTarget) => void;
  /** already translated, e.g. "Where the role applies" */
  label: string;
  className?: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const scope = useOrgScope(orgId);

  if (scope.isLoading) {
    return <ControlSkeleton />;
  }

  const withProjects = scope.byTeam.filter((entry) => entry.projects.length > 0);

  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <Select
        className={cn("h-8 w-[196px]", className)}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label={label}
        disabled={disabled}
      >
        <option value={ORG_TARGET}>{t("scope.picker.org")}</option>
        {scope.teams.length > 0 && (
          <optgroup label={t("scope.picker.teams")}>
            {scope.teams.map((team) => (
              <option key={team.id} value={teamTarget(team.id)}>
                {team.name}
              </option>
            ))}
          </optgroup>
        )}
        {withProjects.map((entry) => (
          <optgroup
            key={entry.team.id}
            label={t("scope.picker.teamProjects", { team: entry.team.name })}
          >
            {entry.projects.map((project) => (
              <option key={project.id} value={projectTarget(project.id)}>
                {project.name}
              </option>
            ))}
          </optgroup>
        ))}
      </Select>
      {!scope.error && scope.teams.length === 0 && (
        <p className="text-xs text-muted-foreground">{t("scope.picker.empty")}</p>
      )}
      {!!scope.error && (
        <LoadError
          error={scope.error}
          resource={t("errors.resources.orgScope")}
          onRetry={scope.refetch}
        />
      )}
    </div>
  );
}
