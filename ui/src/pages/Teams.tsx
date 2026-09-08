import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody, Toolbar } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { createTeam, fetchBudgets, fetchMemberships, fetchTeams } from "@/lib/api";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// teams from the design prototype: card per team with member count, the
// team-scoped budget (when one exists), and the team admin
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

  useErrorState(!!teams.error, "teams");
  const memberships = useQuery({
    queryKey: ["memberships", scope.orgId],
    queryFn: () => fetchMemberships(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const budgetQueries = useQueries({
    queries: (teams.data ?? []).map((team) => ({
      queryKey: ["budgets", "team", team.id],
      queryFn: () => fetchBudgets("team", team.id),
      retry: false,
    })),
  });

  const [addOpen, setAddOpen] = React.useState(false);
  const [name, setName] = React.useState("");

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
        detail: errorDetail(error),
      });
    },
  });

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {t("pages.teams.summary", { count: teams.data?.length ?? 0 })}
        </span>
        <GatedButton gate="team:create" className="ml-auto" onClick={() => setAddOpen(true)} disabled={!scope.orgId}>
          + {t("pages.teams.emptyAction")}
        </GatedButton>
      </Toolbar>

      {teams.isLoading && <CardGridSkeleton cards={3} height={186} min={320} />}
      {teams.error && (
        <LoadError
          error={teams.error}
          resource={t("errors.resources.teams")}
          onRetry={() => void teams.refetch()}
        />
      )}
      {!teams.isLoading && !teams.error && (teams.data?.length ?? 0) === 0 && (
        <EmptyState
          uxTarget="teams"
          icon={<Building />}
          title={t("pages.teams.emptyTitle")}
          description={t("pages.teams.emptyBody")}
          actions={
            <GatedButton gate="team:create" disabled={!scope.orgId} onClick={() => setAddOpen(true)}>
              {t("pages.teams.emptyAction")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(320px,100%),1fr))]">
        {(teams.data ?? []).map((team, i) => {
          const budget = budgetQueries[i]?.data?.[0];
          const members =
            memberships.data?.filter((m) => m.team_id === team.id) ?? [];
          const admin = members.find((m) => m.role === "admin");
          return (
            <div
              key={team.id}
              className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]">
                  <Building className="h-4 w-4" />
                </span>
                <div className="min-w-0">
                  <div className="font-mono text-sm font-semibold">{team.name}</div>
                  <div className="truncate text-xs text-muted-foreground">
                    {t("pages.govTeams.created", { date: fmt.date(team.created_at ?? "") })}
                  </div>
                </div>
              </div>
              <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    {t("pages.teams.members")}
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {memberships.isError ? "—" : members.length}
                  </div>
                </div>
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    {t("pages.teams.budget")}
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {budget
                      ? `${fmt.currency(Number(budget.limit_usd), currency)} / ${budget.period}`
                      : "—"}
                  </div>
                </div>
              </div>
              {admin && (
                <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                  <span className="text-xs text-[color:var(--text-subtle)]">
                    {t("pages.teams.admin")}
                  </span>
                  <span className="ml-auto truncate font-mono text-xs text-[color:var(--text-secondary)]">
                    {admin.user_id}
                  </span>
                </div>
              )}
            </div>
          );
        })}
      </div>

      <EditorSheet
        open={addOpen}
        onOpenChange={(open) => {
          setAddOpen(open);
          if (!open) setName("");
        }}
        title={t("pages.teams.emptyAction")}
        subtitle={t("pages.teams.addSubtitle")}
        dirty={name.trim() !== ""}
        errorMessage={create.isError ? (create.error as Error).message : undefined}
        saveLabel={t("common.create")}
        canSave={!!name.trim()}
        saving={create.isPending}
        onSave={() => create.mutate()}
      >
        <Field label={t("pages.teams.teamName")}>
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="platform" />
        </Field>
      </EditorSheet>
    </PageBody>
  );
}
