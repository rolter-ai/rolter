import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { PageBody, Pill } from "@/components/screen";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  fetchRouteComplexity,
  fetchRoutes,
  setRouteComplexity,
  type ComplexityTier,
  type RouteRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

// bounded input-size tiers per route: requests below each byte ceiling are
// re-routed to the tier's model; the catch-all tier (no ceiling) closes the
// policy. validated server-side against configured route names.
export default function ComplexityRouter() {
  const scope = useScope();
  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
  });

  const policyQueries = useQueries({
    queries: (routes.data ?? []).map((r) => ({
      queryKey: ["route-complexity", r.id],
      queryFn: () => fetchRouteComplexity(r.id),
    })),
  });

  const [editing, setEditing] = React.useState<RouteRow | null>(null);

  const withPolicy = (routes.data ?? []).map((r, i) => ({
    route: r,
    tiers: policyQueries[i]?.data?.tiers ?? [],
  }));
  const configured = withPolicy.filter((p) => p.tiers.length > 0);
  const unconfigured = withPolicy.filter((p) => p.tiers.length === 0);

  return (
    <PageBody>
      <span className="text-sm text-muted-foreground">
        {configured.length} of {withPolicy.length} routes have a complexity policy · requests are
        measured by input bytes and routed to the matching tier
      </span>

      {routes.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(380px,1fr))]">
        {configured.map(({ route, tiers }) => (
          <div
            key={route.id}
            className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
          >
            <div className="flex items-center gap-2.5">
              <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--red-folk)]">
                <ArrowLeftRight className="h-4 w-4" />
              </span>
              <span className="min-w-0 truncate font-mono text-sm font-semibold">
                {route.model}
              </span>
              <Pill
                className="ml-auto"
                color="var(--status-info)"
                tint="rgba(59,130,246,.14)"
              >
                {tiers.length} tiers
              </Pill>
            </div>
            <div className="flex flex-col gap-1.5">
              {tiers.map((t) => (
                <div
                  key={t.name}
                  className="flex items-center gap-2 rounded-[8px] bg-[color:var(--surface-subtle)] px-2.5 py-1.5 font-mono text-xs"
                >
                  <span className="text-[color:var(--text-secondary)]">{t.name}</span>
                  <span className="text-[color:var(--text-subtle)]">
                    {t.max_input_bytes === null || t.max_input_bytes === undefined
                      ? "catch-all"
                      : `≤ ${formatBytes(t.max_input_bytes)}`}
                  </span>
                  <span className="ml-auto truncate text-muted-foreground">→ {t.route}</span>
                </div>
              ))}
            </div>
            <div className="flex items-center justify-end border-t border-[color:var(--border-subtle)] pt-3">
              <Button size="sm" variant="outline" onClick={() => setEditing(route)}>
                Edit policy
              </Button>
            </div>
          </div>
        ))}
      </div>

      {unconfigured.length > 0 && (
        <>
          <div className="mt-2 text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
            No policy yet
          </div>
          <div className="flex flex-wrap gap-2.5">
            {unconfigured.map(({ route }) => (
              <button
                key={route.id}
                type="button"
                onClick={() => setEditing(route)}
                className="flex items-center gap-2 rounded-[8px] border border-[color:var(--border-subtle)] px-3 py-2 font-mono text-xs text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--border-default)] hover:text-foreground"
              >
                {route.model}
                <Plus className="h-3 w-3" />
              </button>
            ))}
          </div>
        </>
      )}

      {editing && (
        <PolicyDialog
          route={editing}
          allRoutes={(routes.data ?? []).map((r) => r.model)}
          onClose={() => setEditing(null)}
        />
      )}
    </PageBody>
  );
}

function formatBytes(bytes: number) {
  if (bytes >= 1_048_576) return `${(bytes / 1_048_576).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KiB`;
  return `${bytes} B`;
}

function PolicyDialog({
  route,
  allRoutes,
  onClose,
}: {
  route: RouteRow;
  allRoutes: string[];
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const existing = useQuery({
    queryKey: ["route-complexity", route.id],
    queryFn: () => fetchRouteComplexity(route.id),
  });

  const [tiers, setTiers] = React.useState<ComplexityTier[] | null>(null);
  React.useEffect(() => {
    if (existing.data && tiers === null) {
      setTiers(
        existing.data.tiers.length > 0
          ? existing.data.tiers
          : [
              { name: "simple", max_input_bytes: 4096, route: route.model },
              { name: "complex", max_input_bytes: null, route: route.model },
            ],
      );
    }
  }, [existing.data, tiers, route.model]);

  const save = useMutation({
    mutationFn: (next: ComplexityTier[]) => setRouteComplexity(route.id, { tiers: next }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["route-complexity", route.id] });
      onClose();
    },
  });
  const set = (i: number, patch: Partial<ComplexityTier>) =>
    setTiers((ts) => ts?.map((t, j) => (j === i ? { ...t, ...patch } : t)) ?? null);

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogHeader>
        <DialogTitle>
          Complexity policy — <span className="font-mono">{route.model}</span>
        </DialogTitle>
        <DialogDescription>
          Tiers are checked in order by input size; the last tier should have no ceiling to
          catch everything else.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-2.5">
        {(tiers ?? []).map((t, i) => (
          <div key={i} className="flex items-center gap-2">
            <Input
              className="w-[110px] font-mono text-xs"
              value={t.name}
              placeholder="tier name"
              onChange={(e) => set(i, { name: e.target.value })}
            />
            <Input
              className="w-[110px] font-mono text-xs"
              type="number"
              min={1}
              value={t.max_input_bytes ?? ""}
              placeholder="catch-all"
              onChange={(e) =>
                set(i, {
                  max_input_bytes: e.target.value === "" ? null : Number(e.target.value),
                })
              }
            />
            <Select
              className="min-w-0 flex-1 font-mono text-xs"
              value={t.route}
              onChange={(e) => set(i, { route: e.target.value })}
            >
              {allRoutes.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
            <button
              type="button"
              title="Remove tier"
              onClick={() => setTiers((ts) => ts?.filter((_, j) => j !== i) ?? null)}
              className="flex h-8 flex-none items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </button>
          </div>
        ))}
        <Button
          size="sm"
          variant="outline"
          onClick={() =>
            setTiers((ts) => [
              ...(ts ?? []),
              { name: `tier-${(ts?.length ?? 0) + 1}`, max_input_bytes: null, route: route.model },
            ])
          }
        >
          <Plus className="h-3.5 w-3.5" />
          Add tier
        </Button>
        {save.isError && (
          <p className="text-xs text-destructive">{(save.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Cancel
        </Button>
        <Button
          disabled={!tiers || tiers.length === 0 || save.isPending}
          onClick={() => tiers && save.mutate(tiers)}
        >
          Save
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
