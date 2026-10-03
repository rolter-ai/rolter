import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { AlertTriangle } from "lucide-react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { ControlSkeleton } from "@/components/LoadingState";
import { Pill } from "@/components/screen";
import { Combobox } from "@/components/ui/combobox";
import { fetchOrgProjects, fetchTeams, type OrgProjectRow, type TeamRow } from "@/lib/api";
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
 * `GET /api/v1/orgs/{org_id}/projects` answers the whole org in one request
 * (#1357), and each project carries the team that owns it, so the options are
 * still grouped by team — two teams may each have a "prod", and an ungrouped
 * flat list would offer the operator two identical options.
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

/**
 * What a stored `team_id`/`project_id` turned out to name.
 *
 * `nameFor` collapses the last two cases into a string, which is fine for a
 * control that renders its own `LoadError` underneath and useless for a
 * read-only chip: a scope the account cannot resolve comes back as a uuid and
 * is drawn as if that uuid were the scope's name (#1671).
 */
export type ResolvedScope =
  /** neither id is set: the scope is the org itself */
  | { kind: "org" }
  | { kind: "named"; id: string; name: string }
  /** the row is gone, invisible to this account, or its list failed to load */
  | { kind: "unresolved"; id: string };

export interface TeamProjects {
  team: TeamRow;
  projects: OrgProjectRow[];
}

export interface OrgScope {
  teams: TeamRow[];
  /** every team in the org with the projects under it, teams in list order */
  byTeam: TeamProjects[];
  isLoading: boolean;
  /** whichever of the two requests failed */
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
  nameFor: (scope: { team_id?: string | null; project_id?: string | null }) => string | undefined;
  /**
   * The same lookup, keeping "no scope stored" apart from "could not resolve
   * it" — which is what a read-only surface needs to say so rather than print
   * the id as a name (#1671).
   */
  resolve: (scope: { team_id?: string | null; project_id?: string | null }) => ResolvedScope;
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
 * Every team in `orgId` and every project under those teams, in two requests
 * whatever the size of the org.
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

  // one request for the whole org, not one per team. its own key rather than
  // the per-team `["scope", "projects", teamId]` the switcher uses: the two
  // answer different questions and neither can be served from the other's cache
  const projects = useQuery({
    queryKey: ["scope", "orgProjects", orgId],
    queryFn: () => fetchOrgProjects(orgId as string),
    enabled: !!orgId,
    ...SHARED,
  });

  const projectRows = projects.data ?? [];

  // plain values rather than memos: these are two small arrays, and a memo over
  // query data would need that data as its dependency, which is a new array on
  // every settle anyway. teams carry the order, so a team with no project of
  // its own is still offered as a scope
  const byTeam: TeamProjects[] = teamRows.map((team) => ({
    team,
    projects: projectRows.filter((project) => project.team_id === team.id),
  }));

  const resolve = (scope: {
    team_id?: string | null;
    project_id?: string | null;
  }): ResolvedScope => {
    // the most specific id wins, exactly as the mapping endpoints resolve it
    const id = scope.project_id || scope.team_id;
    if (!id) return { kind: "org" };
    const name = scope.project_id
      ? projectRows.find((p) => p.id === scope.project_id)?.name
      : teamRows.find((team) => team.id === scope.team_id)?.name;
    return name ? { kind: "named", id, name } : { kind: "unresolved", id };
  };

  const nameFor = (scope: {
    team_id?: string | null;
    project_id?: string | null;
  }): string | undefined => {
    const resolved = resolve(scope);
    if (resolved.kind === "org") return undefined;
    return resolved.kind === "named" ? resolved.name : resolved.id;
  };

  return {
    teams: teamRows,
    byTeam,
    isLoading: teams.isLoading || projects.isLoading,
    error: teams.error ?? projects.error ?? null,
    refetch: () => {
      teams.refetch();
      projects.refetch();
    },
    nameFor,
    resolve,
  };
}

/**
 * The scope as a sentence fragment, for copy that interpolates it.
 *
 * The chip below carries the id in a tooltip; a confirmation body has nowhere
 * to put one, so the unresolved case names the id inline instead — a dialog
 * that says what is being withdrawn has to be quotable on its own (#1677).
 */
export function orgScopeText(
  t: TFunction,
  scope: OrgScope,
  value: { team_id?: string | null; project_id?: string | null },
): string {
  const resolved = scope.resolve(value);
  if (resolved.kind === "org") return t("scope.picker.org");
  if (resolved.kind === "named") return resolved.name;
  return t("scope.picker.unresolvedWithId", { id: resolved.id });
}

/**
 * The chip a read-only surface draws a stored scope as.
 *
 * Three surfaces read a mapping's or a profile's scope without a picker under
 * them, so the picker's `LoadError` and its retry are nowhere in sight. Drawing
 * an unresolved scope as its raw uuid there tells the operator nothing failed
 * and reads exactly like a scope that happens to be named that, so the
 * unresolved case gets its own copy and the warning tone, with the id kept in
 * the tooltip for a support conversation to quote (#1671).
 *
 * `format` is for a chip that says more than the scope — the access profile
 * card's "Admin on Gateway" — so the tone, the icon and the tooltip stay in one
 * place rather than being rebuilt per screen (#1677).
 */
export function OrgScopePill({
  scope,
  value,
  className,
  tint = "var(--surface-card)",
  format,
}: {
  scope: OrgScope;
  value: { team_id?: string | null; project_id?: string | null };
  className?: string;
  tint?: string;
  /** wrap the scope's own label in the sentence the chip is really about */
  format?: (scope: string) => string;
}) {
  const { t } = useTranslation();
  const resolved = scope.resolve(value);
  const label = (text: string) => (format ? format(text) : text);

  if (resolved.kind === "unresolved") {
    return (
      <Pill
        color="var(--status-warning-text)"
        tint={tint}
        className={className}
        title={t("scope.picker.unresolvedTitle", { id: resolved.id })}
      >
        <AlertTriangle className="h-3 w-3" aria-hidden="true" />
        {label(t("scope.picker.unresolved"))}
      </Pill>
    );
  }

  return (
    <Pill color="var(--text-secondary)" tint={tint} className={className}>
      {label(resolved.kind === "named" ? resolved.name : t("scope.picker.org"))}
    </Pill>
  );
}

/**
 * A select naming the org, any team in it, or any project in any of those
 * teams.
 *
 * The scope is part of a form row rather than a region of its own, so the three
 * states are control-sized: a `ControlSkeleton` while the two requests are in
 * flight, a line under the select when the org has no teams to narrow to, and a
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
      <Combobox
        className={cn("w-[196px]", className)}
        size="sm"
        value={value}
        onChange={onChange}
        aria-label={label}
        disabled={disabled}
        options={[
          { value: ORG_TARGET, label: t("scope.picker.org") },
          ...scope.teams.map((team) => ({
            value: teamTarget(team.id),
            label: team.name,
            group: t("scope.picker.teams"),
          })),
          ...withProjects.flatMap((entry) =>
            entry.projects.map((project) => ({
              value: projectTarget(project.id),
              label: project.name,
              group: t("scope.picker.teamProjects", { team: entry.team.name }),
            })),
          ),
        ]}
      />
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
