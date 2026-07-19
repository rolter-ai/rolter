import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock, Trash2 } from "lucide-react";
import * as React from "react";

import { ModelSheet, type ModelSheetMode } from "@/components/ModelSheet";
import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  Pill,
  SearchInput,
  SortLabel,
  StatusDot,
  useSort,
} from "@/components/screen";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  deleteModel,
  fetchModelPrices,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  fetchRouteTargets,
  type EffectiveModelDto,
  type RouteRow,
  type RouteTargetRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";

const GRID = "1.5fr 0.95fr 0.95fr 0.9fr 1.05fr 0.55fr 108px";

type Origin = "all" | "config" | "db";

interface CatalogRow {
  name: string;
  entry: EffectiveModelDto;
  route: RouteRow | null;
  providerName: string;
  strategy: string;
  origin: "config" | "db";
  locked: boolean;
  enabled: boolean;
  inPrice: string;
  outPrice: string;
  weight: string;
}

// model catalog from the design prototype: search + origin chips over a
// sortable grid table with modality/origin pills, the param-lock tooltip, and
// the unified add/edit/view model sheet
export default function Models() {
  const queryClient = useQueryClient();
  const scope = useScope();

  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels });
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
  const prices = useQuery({
    queryKey: ["model-prices"],
    queryFn: fetchModelPrices,
    retry: false,
  });
  const targetQueries = useQueries({
    queries: (routes.data ?? []).map((r) => ({
      queryKey: ["route-targets", r.id],
      queryFn: () => fetchRouteTargets(r.id),
    })),
  });

  const [search, setSearch] = React.useState("");
  const [origin, setOrigin] = React.useState<Origin>("all");
  const { sort, cycle, apply } = useSort<"name" | "provider" | "origin" | "weight">();
  const [sheet, setSheet] = React.useState<{
    mode: ModelSheetMode;
    route?: RouteRow | null;
    configModel?: EffectiveModelDto | null;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<EffectiveModelDto | null>(null);

  const routeByModel = React.useMemo(() => {
    const map = new Map<string, RouteRow>();
    for (const route of routes.data ?? []) map.set(route.model, route);
    return map;
  }, [routes.data]);

  const targetsByRoute = React.useMemo(() => {
    const map = new Map<string, RouteTargetRow[]>();
    (routes.data ?? []).forEach((r, i) => map.set(r.id, targetQueries[i]?.data ?? []));
    return map;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routes.data, targetQueries.map((q) => q.dataUpdatedAt).join(",")]);

  const providerName = (id: string | undefined) =>
    (id && providers.data?.find((p) => p.id === id)?.name) || "—";

  const rows: CatalogRow[] = (models.data ?? []).map((entry) => {
    const route = routeByModel.get(entry.model) ?? null;
    const target = route ? targetsByRoute.get(route.id)?.[0] : undefined;
    const price = prices.data?.find((p) => p.model === entry.model);
    const policy = route?.param_policy as Record<string, unknown> | undefined;
    const deny = Array.isArray(policy?.deny) ? (policy.deny as unknown[]) : [];
    return {
      name: entry.model,
      entry,
      route,
      providerName: providerName(target?.provider_id),
      strategy: entry.strategy,
      origin: entry.source === "config" ? "config" : "db",
      locked: policy?.mode === "deny" || deny.length > 0,
      enabled: route?.enabled ?? true,
      inPrice: price ? `$${price.input_per_mtok}` : "—",
      outPrice: price ? `$${price.output_per_mtok}` : "—",
      weight: target ? String(target.weight) : "—",
    };
  });

  const q = search.trim().toLowerCase();
  const filtered = rows.filter(
    (r) =>
      (origin === "all" || r.origin === origin) &&
      (!q || r.name.toLowerCase().includes(q) || r.providerName.toLowerCase().includes(q)),
  );
  const sorted = apply(filtered, {
    name: (r) => r.name,
    provider: (r) => r.providerName,
    origin: (r) => r.origin,
    weight: (r) => (r.weight === "—" ? -1 : Number(r.weight)),
  });

  const counts = {
    all: rows.length,
    config: rows.filter((r) => r.origin === "config").length,
    db: rows.filter((r) => r.origin === "db").length,
  };

  const providerCount = new Set(rows.map((r) => r.providerName).filter((p) => p !== "—")).size;

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["models"] });
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
  };

  const removeModel = useMutation({
    mutationFn: (model: string) => deleteModel(model),
    onSuccess: invalidate,
  });

  const scopeBlocked = !scope.isLoading && !!scope.error;

  return (
    <PageBody>
      <div className="flex items-center gap-3">
        <SearchInput
          placeholder="Search models"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <span className="text-sm text-muted-foreground">
          {rows.length} models · {providerCount} providers
        </span>
        <Button
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.projectId}
        >
          + Add model
        </Button>
      </div>

      <div className="flex flex-wrap items-center gap-2.5">
        {(
          [
            ["all", "All"],
            ["db", "DB-managed"],
            ["config", "Config"],
          ] as [Origin, string][]
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            onClick={() => setOrigin(key)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-full border px-3 text-xs transition-colors",
              origin === key
                ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-foreground"
                : "border-[color:var(--border-subtle)] text-muted-foreground hover:text-foreground",
            )}
          >
            {label}
            <span className="font-mono text-[11px] text-[color:var(--text-subtle)]">
              {counts[key]}
            </span>
          </button>
        ))}
        <span className="ml-auto inline-flex items-center gap-[7px] text-xs text-muted-foreground">
          <Pill
            color="var(--text-secondary)"
            tint="var(--surface-subtle)"
            border="var(--border-default)"
          >
            <Lock className="h-3 w-3" />
            config
          </Pill>
          shipped in config · immutable
        </span>
      </div>

      {models.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {models.error && <p className="text-sm text-destructive">Failed to load models.</p>}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scope.error}. Read-only view still works.
        </p>
      )}

      <ListTable>
        <ListHeader grid={GRID}>
          <SortLabel label="Model" col="name" sort={sort} onCycle={(c) => cycle(c as never)} />
          <SortLabel
            label="Provider"
            col="provider"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <span>Strategy</span>
          <SortLabel label="Origin" col="origin" sort={sort} onCycle={(c) => cycle(c as never)} />
          <span className="text-right">In · out /Mtok</span>
          <SortLabel
            label="Weight"
            col="weight"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
            justify="flex-end"
          />
          <span />
        </ListHeader>
        {sorted.map((r) => (
          <ListRow key={r.name} grid={GRID}>
            <div className="flex min-w-0 items-center gap-2">
              <StatusDot
                color={r.enabled ? "var(--status-success)" : "var(--text-subtle)"}
              />
              <span className="truncate font-mono text-sm">{r.name}</span>
              {r.locked && (
                <span
                  className="flex-none cursor-help text-[color:var(--text-subtle)]"
                  title="Parameters locked — client overrides for the locked params are ignored; server-side values are enforced. Edit the model to unlock."
                >
                  <Lock className="h-3 w-3" />
                </span>
              )}
            </div>
            <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
              {r.providerName}
            </span>
            <div>
              <Pill color="var(--status-info)" tint="rgba(59,130,246,.14)">
                {r.strategy}
              </Pill>
            </div>
            <div>
              {r.origin === "config" ? (
                <Pill
                  color="var(--text-secondary)"
                  tint="var(--surface-subtle)"
                  border="var(--border-default)"
                >
                  <Lock className="h-3 w-3" />
                  read-only
                </Pill>
              ) : (
                <Pill
                  color="var(--status-success)"
                  tint="rgba(22,163,74,.14)"
                  border="color-mix(in srgb, var(--status-success) 32%, transparent)"
                >
                  db
                </Pill>
              )}
            </div>
            <span className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
              {r.inPrice} · {r.outPrice}
            </span>
            <span className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
              {r.weight}
            </span>
            <div className="flex items-center justify-end gap-1.5">
              <Button
                size="sm"
                variant="outline"
                className="h-[30px]"
                disabled={r.origin === "db" && !r.route}
                onClick={() =>
                  r.origin === "config"
                    ? setSheet({ mode: "view", configModel: r.entry })
                    : r.route && setSheet({ mode: "edit", route: r.route })
                }
              >
                {r.origin === "config" ? "View" : "Edit"}
              </Button>
              {r.origin === "db" && (
                <button
                  type="button"
                  title="Delete model"
                  onClick={() => setDeleteTarget(r.entry)}
                  className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger)]"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              )}
            </div>
          </ListRow>
        ))}
        {!models.isLoading && sorted.length === 0 && (
          <p className="px-4 py-8 text-center text-sm text-muted-foreground">
            No models match.
          </p>
        )}
      </ListTable>

      <ModelSheet
        open={!!sheet}
        mode={sheet?.mode ?? "add"}
        onOpenChange={(open) => !open && setSheet(null)}
        projectId={scope.projectId ?? null}
        orgId={scope.orgId ?? null}
        providers={providers.data ?? []}
        route={sheet?.route ?? null}
        configModel={sheet?.configModel ?? null}
        models={models.data ?? []}
        routes={routes.data ?? []}
        onDone={invalidate}
      />

      <Dialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete model</DialogTitle>
          <DialogDescription>
            This removes all routes and targets for{" "}
            <span className="font-mono">{deleteTarget?.model}</span>. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        {removeModel.isError && (
          <p className="text-xs text-destructive">{(removeModel.error as Error).message}</p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removeModel.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              removeModel.mutate(deleteTarget.model, {
                onSuccess: () => setDeleteTarget(null),
              });
            }}
          >
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}
