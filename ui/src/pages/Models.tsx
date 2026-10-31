import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { StrategyBadge } from "@/components/StrategyBadge";
import { Boxes, ChevronRight, Lock, Tag } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { CacheReportingBadge } from "@/components/CacheReportingBadge";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { ModelPriceCell } from "@/components/ModelPriceCell";
import { ModelSheet, type ModelSheetMode } from "@/components/ModelSheet";
import { RouteTargetList } from "@/components/RouteTargetList";
import { StrategyHint } from "@/components/StrategyHint";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  Pill,
  SearchInput,
  SortLabel,
  StatusDot,
  useSort,
  Toolbar,
  primaryColumn,
} from "@/components/screen";
import { Button } from "@/components/ui/button";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { Segmented } from "@/components/ui/segmented";
import { BreakerBadge } from "@/components/BreakerBadge";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LabelChips, LabelFilterSelect, LabelSheet, useModelLabels } from "@/components/Labels";
import {
  deleteModel,
  fetchModelPrices,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  fetchHealthTimeline,
  fetchUptime,
  useBreakerState,
  type EffectiveModelDto,
  type RouteRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { HEALTH_SLA, targetViews, type RouteTargetView } from "@/lib/route-targets";
import { summarizeCacheReporting, type CacheReporting } from "@/lib/cache-reporting";
import { RowCapabilityScope } from "@/lib/can";
import { useScope } from "@/lib/scope";
import { strategyHintKey } from "@/lib/strategies";
import { useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useProviderCacheProbes } from "@/lib/use-provider-cache-probes";
import { useScreenReady } from "@/lib/ux-react";

// model · provider · strategy · targets · origin · price · actions. strategy
// is sized for `precise_cache_aware`, the longest identifier it has to hold,
// and the actions for a db row's three buttons in russian, the longer catalog:
// at 108px they spilled over the price cell, which the weight column beside it
// used to hide
const GRID = `${primaryColumn(1.25)} 0.8fr 1.05fr 0.85fr 1fr 0.95fr 168px`;

type Origin = "all" | "config" | "db";

// the few words a row has room for, per caveat `strategyHintKey` can name
const CAVEAT_LABEL: Record<string, string> = {
  "pages.routing.strategyHints.needsTelemetry": "pages.models.strategyCaveat.needsTelemetry",
  "pages.routing.strategyHints.deploymentWide": "pages.models.strategyCaveat.deploymentWide",
};

interface CatalogRow {
  /** the row's identity: the model name qualified by its organization, since two can share a name */
  id: string;
  name: string;
  entry: EffectiveModelDto;
  route: RouteRow | null;
  providerName: string;
  /**
   * every provider the route fans out to, deduplicated and in target order.
   *
   * the column only has room for the first name plus a count, so the full list
   * lives in a tooltip and backs search and the provider tally — a route's
   * second provider used to be invisible to both (#1202)
   */
  providerNames: string[];
  /**
   * whether those providers report prompt-cache metrics, by the rule of
   * `summarizeCacheReporting`. `null` when the verdicts could not be read or
   * the route's providers are unknown, so the column shows nothing
   */
  cache: CacheReporting | null;
  /**
   * where the route's traffic goes, carried by the catalog entry itself
   * (#2210). `null` only when the control plane predates that field, and then
   * just the count is known
   */
  targets: RouteTargetView[] | null;
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
}

// model catalog: search + origin chips over a
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
  const cacheProbes = useProviderCacheProbes(scope.orgId);
  // a target names its provider; the verdicts are keyed by id. a name this
  // org's provider list does not hold (or a list not read yet) maps to no id,
  // which is "never probed" once the verdicts themselves have been read
  const providerIdByName = new Map((providers.data ?? []).map((p) => [p.name, p.id]));
  const prices = useQuery({
    queryKey: ["model-prices"],
    queryFn: fetchModelPrices,
    retry: false,
  });
  // each catalog entry carries its own targets and the organization that owns
  // it (#2210), so there is no second read of the effective config to match by
  // name: two organizations that both serve a `gpt-4o` are two entries, each
  // with its own targets
  const hasModels = (models.data?.length ?? 0) > 0;
  // health is shown where it is known: the uptime rollup needs ClickHouse and
  // analytics access, and without either the targets simply carry no health.
  // the key is the Health screen's, so the two share an answer
  const uptime = useQuery({
    queryKey: ["health-uptime", HEALTH_SLA],
    queryFn: async () => (await fetchUptime(HEALTH_SLA)) ?? [],
    enabled: hasModels,
    retry: false,
  });
  const healthKnown = uptime.isSuccess && Array.isArray(uptime.data);
  // the hourly timeline tells "within SLA" from "SLA at risk" (#2256). It is
  // optional: where it cannot be read a target is judged on the rollup alone
  const timeline = useQuery({
    queryKey: ["health-timeline"],
    queryFn: async () => (await fetchHealthTimeline("hour").catch(() => [])) ?? [],
    enabled: hasModels && healthKnown,
    retry: false,
  });
  // the gateways' live breakers, drawn beside each target's health where one is
  // tripped; a failed read leaves the rows as they were
  const breakers = useBreakerState();
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(() => new Set());
  const toggleTargets = (model: string) =>
    setExpanded((open) => {
      const next = new Set(open);
      if (!next.delete(model)) next.add(model);
      return next;
    });
  const detailPrefix = React.useId();

  const [search, setSearch] = React.useState("");
  const [labelFilter, setLabelFilter] = React.useState("");
  // the model whose labels are open, with the organization that owns it
  // (#3476): two organizations' `gpt-4o` carry labels of their own
  const [labelling, setLabelling] = React.useState<{ orgId: string; model: string } | null>(null);
  // a model label is keyed by the model's name, not by a row id: the catalog is
  // deployment-wide and has no row of its own to point at
  const labels = useModelLabels();
  const [origin, setOrigin] = React.useState<Origin>("all");
  const [unpricedOnly, setUnpricedOnly] = React.useState(false);
  const { sort, cycle, apply } = useSort<"name" | "provider" | "strategy" | "targets" | "origin">();
  const [sheet, setSheet] = React.useState<{
    mode: ModelSheetMode;
    route?: RouteRow | null;
    configModel?: EffectiveModelDto | null;
    configTargets?: RouteTargetView[] | null;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<EffectiveModelDto | null>(null);

  const routeByModel = React.useMemo(() => {
    const map = new Map<string, RouteRow>();
    for (const route of routes.data ?? []) map.set(route.model, route);
    return map;
  }, [routes.data]);

  // an absent price row only means "unpriced" once the catalogue has actually
  // been read: while it is loading, or if the request failed, every model would
  // otherwise be reported as unpriced on no evidence
  const pricesKnown = prices.isSuccess;

  const rows: CatalogRow[] = (models.data ?? []).map((entry) => {
    // the scope's routes are one project's, and another organization may
    // serve the same name (#1857): only an entry of that project is its row
    const route =
      entry.project_id && entry.project_id === scope.projectId
        ? (routeByModel.get(entry.model) ?? null)
        : null;
    // likewise the provider ids are the scope organization's: another
    // organization's row is not judged on them (a config-file row, which has
    // no organization, reads as not probed, as before)
    const ownOrg = !entry.org_id || entry.org_id === scope.orgId;
    const targets = entry.route_targets
      ? targetViews(
          { model: entry.model, targets: entry.route_targets },
          healthKnown ? uptime.data : undefined,
          timeline.data,
          breakers.data,
          // the entry's own organization's breakers and providers, never
          // those of another organization's route of the same name (#3460)
          entry.org_id ?? "",
        )
      : null;
    // the first provider names the column; a route spread over several says so
    // instead of pretending it lives on one
    const providerNames = [...new Set((targets ?? []).map((tg) => tg.provider))];
    const price = prices.data?.find((p) => p.model === entry.model);
    const policy = route?.param_policy as Record<string, unknown> | undefined;
    const deny = Array.isArray(policy?.deny) ? (policy.deny as unknown[]) : [];
    return {
      // the model name is shared across organizations, so it cannot key a row
      id: `${entry.org_id ?? ""}/${entry.model}`,
      name: entry.model,
      entry,
      route,
      providerName:
        providerNames.length > 1
          ? t("pages.models.providerCount", {
              first: providerNames[0],
              count: providerNames.length - 1,
            })
          : (providerNames[0] ?? "—"),
      providerNames,
      // the verdicts are read per org, so a row is judged only on the scope's
      // own providers, and not before the provider list that maps names to ids
      // has been read
      cache:
        providers.isSuccess && ownOrg
          ? summarizeCacheReporting(
              providerNames.map((n) => providerIdByName.get(n)),
              cacheProbes.byProvider,
            )
          : null,
      targets,
      targetCount: targets?.length ?? entry.targets,
      strategy: entry.strategy,
      origin: entry.source === "config" ? "config" : "db",
      locked: policy?.mode === "deny" || deny.length > 0,
      enabled: route?.enabled ?? true,
      // a price row carries the currency it was written in, so the cell says
      // what the operator actually configured rather than assuming dollars
      inPrice: price ? fmt.currency(Number(price.input_per_mtok), price.currency) : "—",
      outPrice: price ? fmt.currency(Number(price.output_per_mtok), price.currency) : "—",
      priced: pricesKnown ? !!price : null,
    };
  });

  const q = search.trim().toLowerCase();
  const filtered = rows.filter(
    (r) =>
      (origin === "all" || r.origin === origin) &&
      (!unpricedOnly || r.priced === false) &&
      labels.matches(r.entry.org_id ?? "", r.name, labelFilter) &&
      (!q ||
        r.name.toLowerCase().includes(q) ||
        r.providerNames.some((n) => n.toLowerCase().includes(q))),
  );
  const sorted = apply(filtered, {
    name: (r) => r.name,
    provider: (r) => r.providerName,
    strategy: (r) => r.strategy,
    targets: (r) => r.targetCount,
    origin: (r) => r.origin,
  });

  const counts = {
    all: rows.length,
    config: rows.filter((r) => r.origin === "config").length,
    db: rows.filter((r) => r.origin === "db").length,
  };

  const providerCount = new Set(rows.flatMap((r) => r.providerNames)).size;
  const unpricedCount = rows.filter((r) => r.priced === false).length;

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["models"] });
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
  };

  const removeModel = useMutation({
    mutationFn: (entry: EffectiveModelDto) => deleteModel(entry.model, entry.org_id),
    onSuccess: invalidate,
  });

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  const filtersActive = !!q || origin !== "all" || unpricedOnly || !!labelFilter;
  const clearFilters = () => {
    setSearch("");
    setOrigin("all");
    setUnpricedOnly(false);
    setLabelFilter("");
  };

  return (
    <PageBody>
      <Toolbar>
        <SearchInput
          placeholder={t("pages.models.searchPlaceholder")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <LabelFilterSelect value={labelFilter} onChange={setLabelFilter} options={labels.options} />
        {/* the tally waits for the catalog, which names each target's provider:
            unread, it would print a zero it has no evidence for (#1980) */}
        <ListSummary data={models.data}>
          {() => (
            <>
              {t("pages.models.modelTally", { count: rows.length })}
              {" · "}
              {t("pages.models.providerTally", { count: providerCount })}
            </>
          )}
        </ListSummary>
        {/* adding a model creates a route, not a `model` row: the catalog is
            read-only apart from a superadmin's delete, so the create this
            button takes is the route's (#1258) */}
        <GatedButton
          gate="route:create"
          control="model-new"
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.projectId}
        >
          + {t("pages.models.emptyAction")}
        </GatedButton>
      </Toolbar>

      <div className="flex flex-wrap items-center gap-2.5">
        <Segmented<Origin>
          ariaLabel={t("pages.models.originFilterAria")}
          value={origin}
          onChange={setOrigin}
          options={(
            [
              ["all", t("pages.models.origin.all")],
              ["db", t("pages.models.origin.db")],
              ...(counts.config > 0 ? [["config", t("pages.models.origin.config")]] : []),
            ] as [Origin, string][]
          ).map(([key, label]) => ({
            value: key,
            label: models.data ? `${label} ${counts[key]}` : label,
          }))}
        />
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
            <span className="font-mono text-2xs text-[color:var(--text-subtle)]">
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
              {t("pages.models.readOnlyPill")}
            </Pill>
            {t("pages.models.configLegend")}
          </span>
        )}
      </div>

      {models.error && (
        <LoadError
          error={models.error}
          resource={t("errors.resources.models")}
          onRetry={() => models.refetch()}
          target="models"
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("common.scopeReadOnly", { reason: scopeMessage })}
        </p>
      )}

      <ListTable label={t("screens.model-catalog.title")} minWidth={940}>
        <ListHeader grid={GRID}>
          <SortLabel
            label={t("pages.models.columns.model")}
            col="name"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("common.provider")}
            col="provider"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.models.columns.strategy")}
            col="strategy"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.models.columns.targets")}
            col="targets"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.models.columns.origin")}
            col="origin"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <ListHeaderCell className="text-right">{t("pages.models.columns.price")}</ListHeaderCell>
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={models}>
          <ListSkeleton rows={5} className="p-3" />
        </ListLoadingRow>
        {sorted.map((r) => {
          const open = expanded.has(r.id) && !!r.targets?.length;
          const detailId = `${detailPrefix}-${r.id}`;
          const hintKey = strategyHintKey(r.strategy);
          const withHealth = (r.targets ?? []).filter((tg) => tg.health);
          const failing = withHealth.filter((tg) => tg.health?.state === "breaching").length;
          const atRisk = withHealth.filter((tg) => tg.health?.state === "atRisk").length;
          const tripped = (r.targets ?? []).filter((tg) => tg.breaker);
          const openBreaker = tripped.find((tg) => tg.breaker?.state === "open")?.breaker;
          return (
            <React.Fragment key={r.id}>
              <ListRow grid={GRID} className={open ? "border-b-0" : undefined}>
                <ListCell className="flex min-w-0 items-center gap-2">
                  <StatusDot color={r.enabled ? "var(--status-success)" : "var(--text-subtle)"} />
                  <span className="flex min-w-0 flex-col gap-1">
                    <span className="truncate font-mono text-sm">{r.name}</span>
                    <LabelChips labels={labels.forModel(r.entry.org_id ?? "", r.name)} />
                  </span>
                  {r.locked && (
                    <span
                      className="flex-none cursor-help text-[color:var(--text-subtle)]"
                      title={t("pages.models.lockedHint")}
                    >
                      <Lock className="h-3 w-3" />
                    </span>
                  )}
                </ListCell>
                <ListCell className="flex min-w-0 flex-col items-start gap-1">
                  <span
                    className={cn(
                      "max-w-full truncate font-mono text-xs text-[color:var(--text-secondary)]",
                      r.providerNames.length > 1 && "cursor-help",
                    )}
                    title={r.providerNames.length > 1 ? r.providerNames.join(", ") : undefined}
                  >
                    {r.providerName}
                  </span>
                  <CacheReportingBadge state={r.cache} />
                </ListCell>
                <ListCell className="flex min-w-0 flex-col items-start gap-0.5">
                  <StrategyBadge strategy={r.strategy} />
                  {/* the caveat is written in the row rather than behind a
                      tooltip, which the table's scroll frame clips on the last
                      rows; the sentence opens with the targets below */}
                  {hintKey && (
                    <span
                      className="truncate text-2xs text-[color:var(--status-warning)]"
                      title={t(hintKey)}
                    >
                      {t(CAVEAT_LABEL[hintKey] ?? "pages.models.strategyCaveat.other")}
                    </span>
                  )}
                </ListCell>
                <ListCell className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
                  {r.targets && r.targets.length > 0 ? (
                    <button
                      type="button"
                      aria-expanded={open}
                      aria-controls={open ? detailId : undefined}
                      aria-label={t("pages.models.targets.toggleAria", {
                        targets: t("routeTargets.count", { count: r.targetCount }),
                        model: r.name,
                      })}
                      onClick={() => toggleTargets(r.id)}
                      className="inline-flex items-center gap-1 rounded-sm font-mono text-xs text-[color:var(--text-secondary)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    >
                      <ChevronRight
                        aria-hidden="true"
                        className={cn(
                          "h-3 w-3 flex-none transition-transform",
                          open && "rotate-90",
                        )}
                      />
                      {t("routeTargets.count", { count: r.targetCount })}
                    </button>
                  ) : (
                    <span
                      className={cn(
                        "font-mono text-xs",
                        r.targetCount === 0
                          ? "text-[color:var(--status-warning)]"
                          : "text-[color:var(--text-secondary)]",
                      )}
                    >
                      {r.targetCount === 0
                        ? t("pages.models.targets.none")
                        : t("routeTargets.count", { count: r.targetCount })}
                    </span>
                  )}
                  {/* health where the rollup has it: a failing target is named in
                  the row, a clean bill is a dot, and no data says nothing */}
                  {tripped.length > 0 && (
                    <BreakerBadge breaker={openBreaker ?? tripped[0].breaker} />
                  )}
                  {failing > 0 ? (
                    <span className="inline-flex items-center gap-1 text-2xs text-[color:var(--status-danger-text)]">
                      <StatusDot color="var(--status-danger)" className="h-1.5 w-1.5" />
                      {t("pages.models.targets.failing", { count: failing })}
                    </span>
                  ) : atRisk > 0 ? (
                    <span className="inline-flex items-center gap-1 text-2xs text-[color:var(--status-warning-text)]">
                      <StatusDot color="var(--status-warning)" className="h-1.5 w-1.5" />
                      {t("pages.models.targets.atRisk", { count: atRisk })}
                    </span>
                  ) : (
                    withHealth.length > 0 && (
                      <span className="inline-flex items-center">
                        <StatusDot color="var(--status-success)" className="h-1.5 w-1.5" />
                        <span className="sr-only">{t("pages.models.targets.healthy")}</span>
                      </span>
                    )
                  )}
                </ListCell>
                <ListCell>
                  {r.origin === "config" ? (
                    <Pill
                      color="var(--text-secondary)"
                      tint="var(--surface-subtle)"
                      border="var(--border-default)"
                    >
                      <Lock className="h-3 w-3" />
                      {t("pages.models.readOnlyPill")}
                    </Pill>
                  ) : (
                    <Pill
                      tone="success"
                      border="color-mix(in srgb, var(--status-success) 32%, transparent)"
                    >
                      db
                    </Pill>
                  )}
                </ListCell>
                <ListCell className="grid">
                  <ModelPriceCell priced={r.priced} inPrice={r.inPrice} outPrice={r.outPrice} />
                </ListCell>
                <ListCell className="flex items-center justify-end gap-1.5">
                  {/* a config-file model opens read-only, so only the
                  editable half of this control is gated (#1258) */}
                  {r.origin === "config" ? (
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-[30px]"
                      aria-label={t("pages.models.viewAria", { model: r.name })}
                      onClick={() =>
                        setSheet({ mode: "view", configModel: r.entry, configTargets: r.targets })
                      }
                    >
                      {t("pages.models.view")}
                    </Button>
                  ) : (
                    <GatedButton
                      gate="route:update"
                      control="model-edit"
                      size="sm"
                      variant="outline"
                      className="h-[30px]"
                      aria-label={t("pages.models.editAria", { model: r.name })}
                      disabled={!r.route}
                      onClick={() => r.route && setSheet({ mode: "edit", route: r.route })}
                    >
                      {t("pages.models.edit")}
                    </GatedButton>
                  )}
                  <Button
                    size="sm"
                    variant="outline"
                    className="h-[30px]"
                    aria-label={t("labels.labelsOf", { name: r.name })}
                    onClick={() => setLabelling({ orgId: r.entry.org_id ?? "", model: r.name })}
                  >
                    <Tag className="h-3.5 w-3.5" />
                  </Button>
                  {/* a db-backed row's edit is its route, but forgetting a model
                  outright is deployment-wide: two capabilities on one row (#1258) */}
                  {r.origin === "db" && (
                    <DeleteIconButton
                      gate="model:delete"
                      control="model-delete"
                      label={t("pages.models.deleteAria", { model: r.name })}
                      pending={removeModel.isPending && deleteTarget === r.entry}
                      onClick={() => {
                        removeModel.reset();
                        setDeleteTarget(r.entry);
                      }}
                    />
                  )}
                </ListCell>
              </ListRow>
              {/* weights on demand: the row's own disclosure opens a row beneath
              it, one line per target, rather than a popover the table's
              scroll frame would clip */}
              {open && r.targets && (
                <div
                  role="row"
                  className="border-b border-[color:var(--border-subtle)] pb-3.5 pl-[31px] pr-4 last:border-b-0"
                >
                  <div role="cell" id={detailId}>
                    <StrategyHint strategy={r.strategy} />
                    <RouteTargetList
                      label={t("routeTargets.listLabel", { model: r.name })}
                      strategy={r.strategy}
                      targets={r.targets}
                      // a column of "no health data" says nothing a missing
                      // column does not, so it shows once any target has data
                      health={withHealth.length > 0}
                    />
                  </div>
                </div>
              )}
            </React.Fragment>
          );
        })}
        {/* "no rows" and "nothing matched the filters" are different answers:
            one wants a model created, the other wants the filter cleared */}
        <ListEmptyRow read={models} rows={sorted.length}>
          <EmptyState
            thread
            uxTarget="models"
            icon={<Boxes />}
            title={filtersActive ? t("pages.models.noMatchTitle") : t("pages.models.emptyTitle")}
            description={
              filtersActive ? t("pages.models.noMatchBody") : t("pages.models.emptyBody")
            }
            actions={
              filtersActive ? (
                <Button variant="outline" onClick={clearFilters}>
                  {t("common.clearFilters")}
                </Button>
              ) : (
                <GatedButton
                  gate="route:create"
                  control="model-new-empty"
                  disabled={scopeBlocked || !scope.projectId}
                  onClick={() => setSheet({ mode: "add" })}
                >
                  {t("pages.models.emptyAction")}
                </GatedButton>
              )
            }
          />
        </ListEmptyRow>
      </ListTable>

      {labelling && (
        // an organization's model is labelled under that organization's own
        // `label` capability (#3520), so the sheet asks at the row's org rather
        // than the switcher's; a deployment-wide one keeps the page's answer
        <RowCapabilityScope
          at={labelling.orgId ? { orgId: labelling.orgId, projectId: null } : undefined}
        >
          <LabelSheet
            open
            onOpenChange={(open) => !open && setLabelling(null)}
            orgId={labelling.orgId}
            subjectType="model"
            subjectId={labelling.model}
            subjectName={labelling.model}
          />
        </RowCapabilityScope>
      )}

      <ModelSheet
        open={!!sheet}
        mode={sheet?.mode ?? "add"}
        onOpenChange={(open) => !open && setSheet(null)}
        projectId={scope.projectId ?? null}
        orgId={scope.orgId ?? null}
        providers={providers.data ?? []}
        route={sheet?.route ?? null}
        configModel={sheet?.configModel ?? null}
        configTargets={sheet?.configTargets ?? null}
        models={models.data ?? []}
        routes={routes.data ?? []}
        onDone={invalidate}
      />

      <ConfirmDialog
        name="model-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (!open) {
            removeModel.reset();
            setDeleteTarget(null);
          }
        }}
        title={t("pages.models.confirm.deleteTitle", { model: deleteTarget?.model ?? "" })}
        description={t("pages.models.confirm.deleteBody")}
        confirmLabel={t("pages.models.confirm.deleteConfirm")}
        pending={removeModel.isPending}
        error={removeModel.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.model;
          removeModel.mutate(deleteTarget, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) => {
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorText(error, t),
              });
            },
          });
        }}
      />
    </PageBody>
  );
}
