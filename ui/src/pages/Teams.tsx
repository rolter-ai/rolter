import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building } from "lucide-react";
import * as React from "react";

import { PageBody } from "@/components/screen";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { createTeam, fetchBudgets, fetchMemberships, fetchTeams } from "@/lib/api";
import { useScope } from "@/lib/scope";

// teams from the design prototype: card per team with member count, the
// team-scoped budget (when one exists), and the team admin
export default function Teams() {
  const queryClient = useQueryClient();
  const scope = useScope();

  const teams = useQuery({
    queryKey: ["teams", scope.orgId],
    queryFn: () => fetchTeams(scope.orgId as string),
    enabled: !!scope.orgId,
  });
  const memberships = useQuery({
    queryKey: ["memberships", scope.orgId],
    queryFn: () => fetchMemberships(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const budgetQueries = useQueries({
    queries: (teams.data ?? []).map((t) => ({
      queryKey: ["budgets", "team", t.id],
      queryFn: () => fetchBudgets("team", t.id),
      retry: false,
    })),
  });

  const [addOpen, setAddOpen] = React.useState(false);
  const [name, setName] = React.useState("");

  const create = useMutation({
    mutationFn: () => createTeam(scope.orgId as string, { name }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["teams", scope.orgId] });
      setAddOpen(false);
      setName("");
    },
  });

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {teams.data?.length ?? 0} teams · group users, share budgets and access
        </span>
        <Button className="ml-auto" onClick={() => setAddOpen(true)} disabled={!scope.orgId}>
          + New team
        </Button>
      </div>

      {teams.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(320px,1fr))]">
        {(teams.data ?? []).map((t, i) => {
          const budget = budgetQueries[i]?.data?.[0];
          const members =
            memberships.data?.filter((m) => m.team_id === t.id) ?? [];
          const admin = members.find((m) => m.role === "admin");
          return (
            <div
              key={t.id}
              className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]">
                  <Building className="h-4 w-4" />
                </span>
                <div className="min-w-0">
                  <div className="font-mono text-sm font-semibold">{t.name}</div>
                  <div className="truncate text-xs text-muted-foreground">
                    created {t.created_at?.slice(0, 10)}
                  </div>
                </div>
              </div>
              <div className="grid grid-cols-2 gap-2.5">
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    Members
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {memberships.isError ? "—" : members.length}
                  </div>
                </div>
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    Budget
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {budget ? `$${budget.limit_usd} / ${budget.period}` : "—"}
                  </div>
                </div>
              </div>
              {admin && (
                <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                  <span className="text-xs text-[color:var(--text-subtle)]">admin</span>
                  <span className="ml-auto truncate font-mono text-xs text-[color:var(--text-secondary)]">
                    {admin.user_id}
                  </span>
                </div>
              )}
            </div>
          );
        })}
      </div>

      <Dialog open={addOpen} onOpenChange={setAddOpen}>
        <DialogHeader>
          <DialogTitle>New team</DialogTitle>
          <DialogDescription>Group users, share budgets and access.</DialogDescription>
        </DialogHeader>
        <Field label="Team name">
          <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="platform" />
        </Field>
        {create.isError && (
          <p className="mt-2 text-xs text-destructive">{(create.error as Error).message}</p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setAddOpen(false)}>
            Cancel
          </Button>
          <Button disabled={!name.trim() || create.isPending} onClick={() => create.mutate()}>
            Create
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}
