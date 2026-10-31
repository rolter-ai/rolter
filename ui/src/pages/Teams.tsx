import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building, ChevronRight, Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { IconFrame } from "@/components/ui/icon-frame";
import { Overline } from "@/components/ui/overline";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useOrgScope } from "@/components/OrgScopePicker";
import { TeamSheet } from "@/components/TeamSheet";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { ListSummary, PageBody, Toolbar } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  AssignSubjectProfileSheet,
  SubjectProfiles,
  UnassignSubjectProfileDialog,
  type UnassignTarget,
} from "@/components/SubjectProfiles";
import { errorText } from "@/lib/error-copy";
import {
  createTeam,
  deleteTeam,
  fetchOrgBudgets,
  fetchMemberships,
  fetchTeams,
  fetchUsers,
  type TeamRow,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { teamPeople } from "@/lib/team-people";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

// teams: card per team with its people, projects, budget and admins; a card
// opens the team's detail sheet (#2072)
export default function Teams() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();

  const teams = useQuery({
    queryKey: ["teams", scope.orgId],
    queryFn: () => fetchTeams(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `teams` is the query the user is actually waiting on for this screen

  useScreenReady(!teams.isLoading);

  const memberships = useQuery({
    queryKey: ["memberships", scope.orgId],
    queryFn: () => fetchMemberships(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  // every team and project in the org, from the cache the pickers share
  const orgScope = useOrgScope(scope.orgId);
  // the accounts behind the memberships, so an admin is named by email rather
  // than by uuid (#2066). a superadmin also asks for accounts holding no role
  // anywhere; the flag is in the key so the plain list is not stood in for
  const callerIsSuperadmin = !!useOptionalAuth()?.user?.is_superadmin;
  const users = useQuery({
    queryKey: ["users", scope.orgId, { includeUnassigned: callerIsSuperadmin }],
    queryFn: () => fetchUsers(scope.orgId as string, { includeUnassigned: callerIsSuperadmin }),
    enabled: !!scope.orgId,
    retry: false,
  });
  const nameOf = (id: string) => {
    const user = users.data?.find((u) => u.id === id);
    return user ? user.display_name || user.email : id;
  };
  // one read of the org's budgets however many teams there are (#1996); the rows
  // are the ones the per-team read would have answered, grouped by team below
  const orgBudgets = useQuery({
    queryKey: ["budgets", "org", scope.orgId],
    queryFn: () => fetchOrgBudgets(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const budgetsOf = (teamId: string) => ({
    isError: orgBudgets.isError,
    data: orgBudgets.data?.filter((b) => b.scope_type === "team" && b.scope_id === teamId),
  });

  const [addOpen, setAddOpen] = React.useState(false);
  const [name, setName] = React.useState("");
  const [profileTeam, setProfileTeam] = React.useState<{ id: string; name: string } | null>(null);
  const [unassignProfile, setUnassignProfile] = React.useState<UnassignTarget | null>(null);
  const [openTeamId, setOpenTeamId] = React.useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<TeamRow | null>(null);
  const openTeam = teams.data?.find((team) => team.id === openTeamId);

  const create = useMutation({
    mutationFn: () => createTeam(scope.orgId as string, { name }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["teams", scope.orgId] });
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("toast.created", { what: name }) });
      setAddOpen(false);
      setName("");
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name }),
        detail: errorText(error, t),
      });
    },
  });

  const remove = useMutation({
    mutationFn: (team: TeamRow) => deleteTeam(team.id),
    onSuccess: (_void, team) => {
      // the sidebar's scope switcher lists the same teams under `scope`
      queryClient.invalidateQueries({ queryKey: ["teams", scope.orgId] });
      queryClient.invalidateQueries({ queryKey: ["scope"] });
      queryClient.invalidateQueries({ queryKey: ["memberships", scope.orgId] });
      toast.push({ tone: "success", title: t("toast.deleted", { what: team.name }) });
      setDeleteTarget(null);
      setOpenTeamId(null);
    },
    onError: (error, team) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: team.name }),
        detail: errorText(error, t),
      });
    },
  });
  // `byTeam` lists every team with an empty project list while the projects are
  // still in flight or failed, so a team's projects are only known once both
  // reads have answered; before that a card shows a skeleton, never "0"
  const projectsOf = (teamId: string) =>
    orgScope.isLoading || orgScope.error
      ? undefined
      : (orgScope.byTeam.find((entry) => entry.team.id === teamId)?.projects ?? []);
  const deleteProjects = deleteTarget ? projectsOf(deleteTarget.id)?.length : undefined;

  return (
    <PageBody>
      <Toolbar>
        <ListSummary data={teams.data}>
          {(rows) => t("pages.teams.summary", { count: rows.length })}
        </ListSummary>
        <GatedButton
          gate="team:create"
          control="team-new"
          className="ml-auto"
          onClick={() => setAddOpen(true)}
          disabled={!scope.orgId}
        >
          <Plus className="h-4 w-4" aria-hidden />
          {t("pages.teams.emptyAction")}
        </GatedButton>
      </Toolbar>

      {teams.isLoading && <CardGridSkeleton cards={3} height={260} min={320} />}
      {teams.error && (
        <LoadError
          error={teams.error}
          resource={t("errors.resources.teams")}
          onRetry={() => void teams.refetch()}
          target="teams"
        />
      )}
      {teams.isSuccess && teams.data.length === 0 && (
        <EmptyState
          thread
          uxTarget="teams"
          icon={<Building />}
          title={t("pages.teams.emptyTitle")}
          description={t("pages.teams.emptyBody")}
          actions={
            <GatedButton
              gate="team:create"
              control="team-new-empty"
              disabled={!scope.orgId}
              onClick={() => setAddOpen(true)}
            >
              {t("pages.teams.emptyAction")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(320px,100%),1fr))]">
        {(teams.data ?? []).map((team) => {
          const budgets = budgetsOf(team.id);
          const projects = projectsOf(team.id);
          const people = memberships.data
            ? teamPeople(
                memberships.data,
                team.id,
                (projects ?? []).map((p) => p.id),
              )
            : undefined;
          // the people count needs the project list too: without it a team's
          // project-scoped grants would be silently left out of the number
          const peopleKnown = !!people && !!projects;
          return (
            <div
              key={team.id}
              role="group"
              aria-label={team.name}
              className="flex flex-col gap-3.5 rounded-lg border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <IconFrame>
                  <Building className="h-4 w-4" />
                </IconFrame>
                <div className="min-w-0 flex-1">
                  <div className="truncate font-mono text-sm font-semibold">{team.name}</div>
                  <div className="truncate text-xs text-muted-foreground">
                    {t("pages.govTeams.created", { date: fmt.date(team.created_at ?? "") })}
                  </div>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  className="flex-none"
                  aria-label={t("pages.teams.open", { name: team.name })}
                  aria-haspopup="dialog"
                  onClick={() => setOpenTeamId(team.id)}
                >
                  {t("pages.teams.openAction")}
                  <ChevronRight className="h-4 w-4" aria-hidden />
                </Button>
              </div>
              <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3">
                <div>
                  <Overline>{t("pages.teams.members")}</Overline>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {memberships.isError || orgScope.error ? (
                      "—"
                    ) : peopleKnown ? (
                      people.people.length
                    ) : (
                      <Skeleton width={24} height={16} />
                    )}
                  </div>
                  <div className="text-2xs text-[color:var(--text-subtle)]">
                    {t("pages.teams.membersCaption")}
                  </div>
                </div>
                <div>
                  <Overline>{t("pages.teams.projects")}</Overline>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {orgScope.error ? (
                      "—"
                    ) : projects ? (
                      projects.length
                    ) : (
                      <Skeleton width={24} height={16} />
                    )}
                  </div>
                </div>
                <div className="col-span-2 sm:col-span-1">
                  <Overline>{t("pages.teams.budget")}</Overline>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {budgets?.isError ? (
                      // a failed read is not "no budget": say it could not be read
                      <span className="text-[color:var(--status-danger-text)]">
                        {t("pages.teams.budgetUnavailable")}
                      </span>
                    ) : budgets?.data ? (
                      budgets.data.length === 0 ? (
                        "—"
                      ) : (
                        <>
                          {fmt.currency(Number(budgets.data[0].limit_usd), currency)} /{" "}
                          {budgets.data[0].period}
                          {budgets.data.length > 1 && (
                            <span className="ml-1 text-xs text-muted-foreground">
                              {t("pages.teams.budgetMore", { count: budgets.data.length - 1 })}
                            </span>
                          )}
                        </>
                      )
                    ) : (
                      <Skeleton width={64} height={16} />
                    )}
                  </div>
                </div>
              </div>
              {scope.orgId && (
                <div className="flex flex-col gap-1.5 border-t border-[color:var(--border-subtle)] pt-3">
                  <Overline>{t("pages.teams.profiles")}</Overline>
                  <SubjectProfiles
                    orgId={scope.orgId}
                    subject={{ team_id: team.id }}
                    name={team.name}
                    onAssign={() => setProfileTeam({ id: team.id, name: team.name })}
                    onRemove={setUnassignProfile}
                  />
                </div>
              )}
              {people && (
                <div className="flex items-baseline gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                  <span className="text-xs text-[color:var(--text-subtle)]">
                    {t("pages.teams.admin", { count: people.admins.length })}
                  </span>
                  <span className="ml-auto min-w-0 truncate text-right font-mono text-xs text-[color:var(--text-secondary)]">
                    {people.admins.length === 0 ? (
                      t("pages.teams.noAdmin")
                    ) : (
                      <span title={people.admins.map(nameOf).join(", ")}>
                        {nameOf(people.admins[0])}
                        {people.admins.length > 1 &&
                          ` ${t("pages.teams.adminMore", { count: people.admins.length - 1 })}`}
                      </span>
                    )}
                  </span>
                </div>
              )}
            </div>
          );
        })}
      </div>

      {openTeam && (
        <TeamSheet
          team={openTeam}
          projects={{
            data:
              orgScope.isLoading || orgScope.error
                ? undefined
                : orgScope.byTeam.flatMap((e) => e.projects),
            isLoading: orgScope.isLoading,
            error: orgScope.error,
            refetch: orgScope.refetch,
          }}
          memberships={{
            data: memberships.data,
            isLoading: memberships.isLoading,
            error: memberships.error,
            refetch: () => void memberships.refetch(),
          }}
          users={users.data}
          onClose={() => setOpenTeamId(null)}
          onDelete={() => {
            remove.reset();
            // the sheet makes way for the question, so the confirmation stands alone
            setOpenTeamId(null);
            setDeleteTarget(openTeam);
          }}
        />
      )}

      <ConfirmDialog
        name="team-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (!open) {
            setDeleteTarget(null);
            // a refusal for this team must not greet the next one opened
            remove.reset();
          }
        }}
        title={t("pages.teams.confirm.title", { name: deleteTarget?.name ?? "" })}
        description={
          deleteProjects === undefined
            ? t("pages.teams.confirm.bodyUnknown")
            : t("pages.teams.confirm.body", { count: deleteProjects })
        }
        confirmLabel={t("scope.deleteTeam")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => deleteTarget && remove.mutate(deleteTarget)}
      />

      {profileTeam && scope.orgId && (
        <AssignSubjectProfileSheet
          orgId={scope.orgId}
          subject={{ team_id: profileTeam.id }}
          name={profileTeam.name}
          onClose={() => setProfileTeam(null)}
        />
      )}
      <UnassignSubjectProfileDialog
        target={unassignProfile}
        onClose={() => setUnassignProfile(null)}
      />

      <EditorSheet
        name="team-create"
        open={addOpen}
        onOpenChange={(open) => {
          setAddOpen(open);
          if (!open) setName("");
        }}
        title={t("pages.teams.emptyAction")}
        subtitle={t("pages.teams.addSubtitle")}
        dirty={name.trim() !== ""}
        errorMessage={create.isError ? errorText(create.error, t) : undefined}
        saveLabel={t("common.create")}
        canSave={!!name.trim()}
        saving={create.isPending}
        onSubmit={() => create.mutate()}
      >
        <Field label={t("pages.teams.teamName")} required>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t("pages.teams.teamNamePlaceholder")}
          />
        </Field>
      </EditorSheet>
    </PageBody>
  );
}
