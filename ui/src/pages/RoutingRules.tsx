import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import * as React from "react";

import { PageBody, StatusDot } from "@/components/screen";
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
import { Select } from "@/components/ui/select";
import {
  createRoute,
  createRouteTarget,
  deleteRoute,
  fetchProviders,
  fetchRoutes,
  fetchRouteTargets,
  STRATEGIES,
  type RouteTargetRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

const STRATEGY_TONE: Record<string, [string, string]> = {
  cache_aware: ["var(--status-info)", "rgba(59,130,246,.14)"],
  weighted: ["var(--status-success)", "rgba(22,163,74,.14)"],
  round_robin: ["var(--text-secondary)", "var(--surface-subtle)"],
  consistent_hash: ["var(--status-warning)", "rgba(245,158,11,.14)"],
  power_of_two: ["var(--red-folk)", "var(--red-tint)"],
};

const TARGET_BARS = ["var(--red-folk)", "var(--zinc-400)", "var(--status-info)", "var(--status-success)"];

// routing rules from the design prototype: one card per route with its
// strategy pill, per-target weight bars, and edit/delete actions
export default function RoutingRules() {
  const queryClient = useQueryClient();
  const scope = useScope();

  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
  });
  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  const targetQueries = useQueries({
    queries: (routes.data ?? []).map((r) => ({
      queryKey: ["route-targets", r.id],
      queryFn: () => fetchRouteTargets(r.id),
    })),
  });
  const targetsByRoute = new Map<string, RouteTargetRow[]>();
  (routes.data ?? []).forEach((r, i) => {
    targetsByRoute.set(r.id, targetQueries[i]?.data ?? []);
  });

  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.name ?? id.slice(0, 8);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
    queryClient.invalidateQueries({ queryKey: ["models"] });
  };

  const remove = useMutation({
    mutationFn: (id: string) => deleteRoute(id),
    onSuccess: invalidate,
  });

  const [addOpen, setAddOpen] = React.useState(false);

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {routes.data?.length ?? 0} routes · public model names clients call, resolved to
          upstream targets
        </span>
        <Button className="ml-auto" onClick={() => setAddOpen(true)} disabled={!scope.projectId}>
          + Add route
        </Button>
      </div>

      {routes.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(360px,1fr))]">
        {(routes.data ?? []).map((r) => {
          const targets = targetsByRoute.get(r.id) ?? [];
          const totalWeight = targets.reduce((a, t) => a + t.weight, 0) || 1;
          const tone = STRATEGY_TONE[r.strategy] ?? STRATEGY_TONE.round_robin;
          return (
            <div
              key={r.id}
              className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <span className="min-w-0 truncate font-mono text-sm font-semibold">{r.model}</span>
                <span
                  className="whitespace-nowrap rounded-[6px] px-2 py-[3px] font-mono text-[0.6875rem] uppercase tracking-[0.04em]"
                  style={{ color: tone[0], background: tone[1] }}
                >
                  {r.strategy}
                </span>
                {!r.enabled && (
                  <span className="rounded-[6px] bg-[color:var(--surface-subtle)] px-2 py-[3px] font-mono text-[0.6875rem] uppercase text-[color:var(--text-subtle)]">
                    disabled
                  </span>
                )}
              </div>
              <div className="flex flex-col gap-2.5">
                {targets.length === 0 && (
                  <p className="text-xs text-muted-foreground">No targets yet.</p>
                )}
                {targets.map((t, i) => {
                  const pct = Math.round((t.weight / totalWeight) * 100);
                  return (
                    <div key={t.id} className="flex flex-col gap-[5px]">
                      <div className="flex items-center gap-2 font-mono text-xs">
                        <StatusDot color="var(--status-success)" className="h-1.5 w-1.5" />
                        <span className="text-[color:var(--text-secondary)]">
                          {providerName(t.provider_id)}
                        </span>
                        <span className="text-[color:var(--text-subtle)]">→</span>
                        <span className="min-w-0 truncate text-muted-foreground">
                          {t.upstream_model || r.model}
                        </span>
                        <span className="ml-auto text-[color:var(--text-secondary)]">{pct}%</span>
                      </div>
                      <div className="h-[5px] overflow-hidden rounded-full bg-[color:var(--surface-subtle)]">
                        <div
                          className="h-full rounded-full"
                          style={{
                            width: `${pct}%`,
                            background: TARGET_BARS[i % TARGET_BARS.length],
                          }}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <span className="text-xs text-[color:var(--text-subtle)]">
                  {targets.length} target(s)
                </span>
                <button
                  type="button"
                  title="Delete route"
                  onClick={() => remove.mutate(r.id)}
                  className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {remove.isError && (
        <p className="text-xs text-destructive">{(remove.error as Error).message}</p>
      )}

      {scope.projectId && (
        <AddRouteDialog
          open={addOpen}
          onOpenChange={setAddOpen}
          projectId={scope.projectId}
          providers={providers.data?.map((p) => ({ id: p.id, name: p.name })) ?? []}
          onDone={invalidate}
        />
      )}
    </PageBody>
  );
}

function AddRouteDialog({
  open,
  onOpenChange,
  projectId,
  providers,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  providers: { id: string; name: string }[];
  onDone: () => void;
}) {
  const [model, setModel] = React.useState("");
  const [strategy, setStrategy] = React.useState<string>(STRATEGIES[0]);
  const [providerId, setProviderId] = React.useState("");
  const [weight, setWeight] = React.useState("100");

  React.useEffect(() => {
    if (open) {
      setModel("");
      setStrategy(STRATEGIES[0]);
      setProviderId(providers[0]?.id ?? "");
      setWeight("100");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const create = useMutation({
    mutationFn: async () => {
      const route = await createRoute(projectId, { model, strategy });
      if (providerId) {
        await createRouteTarget(route.id, {
          provider_id: providerId,
          weight: Number(weight) || 1,
        });
      }
    },
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add route</DialogTitle>
        <DialogDescription>
          Public model name resolved to upstream targets by the chosen strategy.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Model name">
          <Input
            className="font-mono"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="gpt-4o"
          />
        </Field>
        <Field label="Strategy">
          <Select value={strategy} onChange={(e) => setStrategy(e.target.value)}>
            {STRATEGIES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="First target">
          <Select value={providerId} onChange={(e) => setProviderId(e.target.value)}>
            <option value="">none (attach later)</option>
            {providers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </Select>
        </Field>
        {providerId && (
          <Field label="Weight">
            <Input
              type="number"
              min={1}
              value={weight}
              onChange={(e) => setWeight(e.target.value)}
            />
          </Field>
        )}
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button disabled={!model.trim() || create.isPending} onClick={() => create.mutate()}>
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
