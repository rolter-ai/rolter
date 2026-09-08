import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, Lock, Trash2, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { ModelPriceCell } from "@/components/ModelPriceCell";
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
  Toolbar,
} from "@/components/screen";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
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
import { useGate } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const GRID = "1.5fr 0.95fr 0.95fr 0.9fr 1.05fr 0.55fr 108px";

type Origin = "all" | "config" | "db";

interface CatalogRow {
  name: string;
  entry: EffectiveModelDto;
  route: RouteRow | null;
  providerName: string;
  targetCount: number;
  strategy: string;
  origin: "config" | "db";
  locked: boolean;
  enabled: boolean;
  inPrice: string;
  outPrice: string;
  /**
   * whether a price row applied to this model (#969).
   *
   * `null` means the price catalogue could not be read, which is not the same
   * as "this model has no price" — the screen must not claim the latter when
   * it only knows the former.
   */
  priced: boolean | null;
  weight: string;
}

// model catalog from the design prototype: search + origin chips over a
// sortable grid table with modality/origin pills, the param-lock tooltip, and
// the unified add/edit/view model sheet
export default function Models() {
  const { t } = useTranslation();
  const toast = useToast();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;

  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `models` is the query the user is actually waiting on for this screen

  useScreenReady(!models.isLoading);

  useErrorState(!!models.error, "models");
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
  const [unpricedOnly, setUnpricedOnly] = React.useState(false);
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

  // an absent price row only means "unpriced" once the catalogue has actually
  // been read: while it is loading, or if the request failed, every model would
  // otherwise be reported as unpriced on no evidence
  const pricesKnown = prices.isSuccess;

  const rows: CatalogRow[] = (models.data ?? []).map((entry) => {
    const route = routeByModel.get(entry.model) ?? null;
    const targets = route ? (targetsByRoute.get(route.id) ?? []) : [];
    // the first target names the provider column; a route spread over several
    // providers says so instead of pretending it lives on one
    const target = targets[0];
    const price = prices.data?.find((p) => p.model === entry.model);
    const policy = route?.param_policy as Record<string, unknown> | undefined;
    const deny = Array.isArray(policy?.deny) ? (policy.deny as unknown[]) : [];
    return {
      name: entry.model,
      entry,
      route,
      providerName:
        targets.length > 1
          ? t("pages.models.providerCount", { first: providerName(target?.provider_id), count: targets.length - 1 })
          : providerName(target?.provider_id),
      strategy: entry.strategy,
      origin: entry.source === "config" ? "config" : "db",
      locked: policy?.mode === "deny" || deny.length > 0,
      enabled: route?.enabled ?? true,
      // a price row carries the currency it was written in, so the cell says
      // what the operator actually configured rather than assuming dollars
      inPrice: price ? fmt.currency(Number(price.input_per_mtok), price.currency) : "—",
      outPrice: price ? fmt.currency(Number(price.output_per_mtok), price.currency) : "—",
      priced: pricesKnown ? !!price : null,
      weight: target ? String(target.weight) : "—",
      targetCount: targets.length,
    };
  });

  const q = search.trim().toLowerCase();
  const filtered = rows.filter(
    (r) =>
      (origin === "all" || r.origin === origin) &&
      (!unpricedOnly || r.priced === false) &&
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
  const unpricedCount = rows.filter((r) => r.priced === false).length;

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["models"] });
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
  };

  const removeModel = useMutation({
    mutationFn: (model: string) => deleteModel(model),
    onSuccess: invalidate,
  });

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  // a db-backed model row is really its route, and forgetting a model outright
  // is deployment-wide — two different capabilities on one row (#1258)
  const routeUpdateGate = useGate("route:update");
  const deleteGate = useGate("model:delete");
  const filtersActive = !!q || origin !== "all" || unpricedOnly;
  const clearFilters = () => {
    setSearch("");
    setOrigin("all");
    setUnpricedOnly(false);
  };

  return (
    <PageBody>
      <Toolbar>
        <SearchInput
          placeholder="Search models"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <span className="text-sm text-muted-foreground">
          {rows.length} models · {providerCount} providers
        </span>
        {/* adding a model creates a route, not a `model` row: the catalog is
            read-only apart from a superadmin's delete, so the create this
            button takes is the route's (#1258) */}
        <GatedButton
          gate="route:create"
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.projectId}
        >
          + Add model
        </GatedButton>
      </Toolbar>

      <div className="flex flex-wrap items-center gap-2.5">
        {(
          [
            ["all", "All"],
            ["db", "DB-managed"],
            // a deployment with nothing in rolter.toml has no config tier to
            // filter by; the chip and its legend appear once one exists
            ...(counts.config > 0 ? [["config", "Config"]] : []),
          ] as [Origin, string][]
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            onClick={() => setOrigin(key)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-full border px-3 text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
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
        {unpricedCount > 0 && (
          <button
            type="button"
            aria-pressed={unpricedOnly}
            onClick={() => setUnpricedOnly((on) => !on)}
            className={cn(
              "inline-flex h-7 items-center gap-1.5 rounded-full border px-3 text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              unpricedOnly
                ? "border-[color:var(--status-warning)] bg-[color:var(--red-tint)] text-foreground"
                : "border-[color:var(--border-subtle)] text-muted-foreground hover:text-foreground",
            )}
          >
            {t("pages.models.unpriced.filter")}
            <span className="font-mono text-[11px] text-[color:var(--text-subtle)]">
              {unpricedCount}
            </span>
          </button>
        )}
        {counts.config > 0 && (
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
        )}
      </div>

      {models.error && (
        <LoadError
          error={models.error}
          resource={t("errors.resources.models")}
          onRetry={() => models.refetch()}
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scopeMessage}. Read-only view still works.
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
        {models.isLoading && <ListSkeleton rows={5} className="p-3" />}
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
              <Pill color="var(--status-info-text)" tint="rgba(59,130,246,.14)">
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
                  color="var(--status-success-text)"
                  tint="rgba(22,163,74,.14)"
                  border="color-mix(in srgb, var(--status-success) 32%, transparent)"
                >
                  db
                </Pill>
              )}
            </div>
            <ModelPriceCell priced={r.priced} inPrice={r.inPrice} outPrice={r.outPrice} />
            <span className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
              {r.weight}
            </span>
            <div className="flex items-center justify-end gap-1.5">
              {/* a config-file model opens read-only, so only the
                  editable half of this control is gated (#1258) */}
              <Button
                size="sm"
                variant="outline"
                className="h-[30px]"
                aria-label={t(
                  r.origin === "config"
                    ? "pages.models.viewAria"
                    : "pages.models.editAria",
                  { model: r.name },
                )}
                title={r.origin === "config" ? undefined : routeUpdateGate.reason}
                disabled={
                  r.origin === "db" && (!r.route || routeUpdateGate.denied)
                }
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
                  title={
                    deleteGate.reason ??
                    t("pages.models.deleteAria", { model: r.name })
                  }
                  aria-label={t("pages.models.deleteAria", { model: r.name })}
                  disabled={
                    deleteGate.denied ||
                    (removeModel.isPending && deleteTarget?.model === r.entry.model)
                  }
                  onClick={() => setDeleteTarget(r.entry)}
                  className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {removeModel.isPending && deleteTarget?.model === r.entry.model ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
                </button>
              )}
            </div>
          </ListRow>
        ))}
        {!models.isLoading && sorted.length === 0 && (
          // "no rows" and "nothing matched the filters" are different answers:
          // one wants a model created, the other wants the filter cleared
          <EmptyState
            uxTarget="models"
            icon={<Boxes />}
            title={filtersActive ? t("pages.models.noMatchTitle") : t("pages.models.emptyTitle")}
            description={
              filtersActive ? t("pages.models.noMatchBody") : t("pages.models.emptyBody")
            }
            actions={
              filtersActive ? (
                <Button variant="outline" onClick={clearFilters}>
                  {t("common.clearSearch")}
                </Button>
              ) : (
                <GatedButton
                  gate="route:create"
                  disabled={scopeBlocked || !scope.projectId}
                  onClick={() => setSheet({ mode: "add" })}
                >
                  {t("pages.models.emptyAction")}
                </GatedButton>
              )
            }
          />
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
          <p className="text-xs text-[color:var(--status-danger-text)]">{(removeModel.error as Error).message}</p>
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
              const what = deleteTarget.model;
              removeModel.mutate(what, {
                onSuccess: () => {
                  setDeleteTarget(null);
                  toast.push({ tone: "success", title: t("toast.deleted", { what }) });
                },
                onError: (error) => {
                  toast.push({
                    tone: "error",
                    title: t("toast.deleteFailed", { what }),
                    detail: errorDetail(error),
                  });
                },
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
