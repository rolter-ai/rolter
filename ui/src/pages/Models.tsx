import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { StrategyBadge } from "@/components/StrategyBadge";
import { Boxes, ChevronRight, Lock, Pencil, Play, Tag } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { CacheReportingBadge } from "@/components/CacheReportingBadge";
import { CopyButton } from "@/components/CopyButton";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { ModelPriceCell } from "@/components/ModelPriceCell";
import { ModelSheet, type ModelSheetMode } from "@/components/ModelSheet";
import {
  GatewayConfigNote,
  gatewayConvergence,
  RouteTrafficCell,
  RouteVariants,
  TRAFFIC_WINDOW_HOURS,
  trafficFor,
  type RouteTraffic,
} from "@/components/RouteDetail";
import { RouteSheet } from "@/components/RouteSheet";
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
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { InfoHint } from "@/components/ui/info-hint";
import { Segmented } from "@/components/ui/segmented";
import { BreakerBadge } from "@/components/BreakerBadge";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  LabelChips,
  LabelFilterSelect,
  LabelSheet,
  useModelLabels,
  useSubjectLabels,
} from "@/components/Labels";
import { Tooltip } from "@/components/ui/tooltip";
import {
  AnalyticsUnavailableError,
  deleteModel,
  deleteRoute,
  fetchAnalyticsByModel,
  fetchClusterNodes,
  fetchModelPrices,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  fetchHealthTimeline,
  fetchUptime,
  useBreakerState,
  type EffectiveModelDto,
  type RouteRow,
  type RouteVariantRow,
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

// model · provider · strategy · targets · traffic · origin · price · actions.
// strategy is sized for `precise_cache_aware`, the longest identifier it has
// to hold, and the actions for a db row's three buttons in russian, the longer
// catalog: at 108px they spilled over the price cell, which the weight column
// beside it used to hide
const GRID = `${primaryColumn(1.25)} 0.8fr 1fr 0.8fr 0.9fr 0.85fr 0.95fr 168px`;

/** how often the gateway list is asked again while one has not caught up */
const NODES_POLL_MS = 10_000;

/**
 * Where a route is defined, which is what the catalog's count and its filter
 * are about: the scope's project, another project the caller can see, or the
 * gateway's config file. The catalog lists them all; Routing Rules listed the
 * first only, which is why the two counted differently (#3685).
 */
type Where = "project" | "other" | "config";

type WhereFilter = "all" | Where;

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
  /**
   * the stored route behind the row, read with its targets and variants. Only
   * the scope project's routes have one, since that is the list the scope can
   * read; it is what the route's edit, delete and labels act on
   */
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
  /**
   * the weighted variants the route routes through instead of its own targets
   * (#3343). Known for the scope project's routes only, the ones read with them
   */
  variants: RouteVariantRow[];
  strategy: string;
  origin: "config" | "db";
  where: Where;
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
  /** requests over the window, for sorting; `null` where the rollup has no figure for the row */
  traffic: RouteTraffic | null;
}

// model catalog: every route the gateway serves that the caller can see, one
// row per route, with search, a where-defined filter and sortable columns.
// a row opens beneath itself into the route: its targets or variants, health,
// labels and the ways to try or re-point it. Routing Rules used to show the
// same routes as cards; the two became this one screen (#3685)
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
    // the targets and variants ride along, so the project's routes cost one
    // request however many there are (#3031, #3343). the key sits under
    // ["routes", project], which every route mutation already invalidates
    queryKey: ["routes", scope.projectId, "targets"],
    queryFn: () => fetchRoutes(scope.projectId as string, ["targets", "variants"]),
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
  // a variant's target names its provider by id
  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.name ?? id.slice(0, 8);
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
  const inProject = (entry: EffectiveModelDto) =>
    !!entry.project_id && entry.project_id === scope.projectId;
  const hasProjectRoutes = (models.data ?? []).some(inProject);
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
  // requests and errors per public model over the last day, for the scope's
  // project. it needs ClickHouse, which a deployment may run without: that is
  // an answer the screen states once, never a zero on every row
  const traffic = useQuery({
    queryKey: ["route-traffic", scope.projectId, TRAFFIC_WINDOW_HOURS],
    queryFn: async () => {
      const since = new Date(Date.now() - TRAFFIC_WINDOW_HOURS * 3_600_000).toISOString();
      const rows = await fetchAnalyticsByModel({ since, project: scope.projectId as string });
      return Array.isArray(rows) ? rows : [];
    },
    enabled: hasProjectRoutes,
    retry: false,
  });
  // whether the gateways have picked a change up: the cluster inventory
  // Cluster reads, under its key. only a gateway that is behind is polled
  const nodes = useQuery({
    queryKey: ["cluster-nodes"],
    queryFn: fetchClusterNodes,
    enabled: hasModels,
    retry: false,
    refetchInterval: (query) =>
      Array.isArray(query.state.data) && gatewayConvergence(query.state.data).lagging > 0
        ? NODES_POLL_MS
        : false,
  });
  // the gateways' live breakers, drawn beside each target's health where one is
  // tripped; a failed read leaves the rows as they were
  const breakers = useBreakerState();
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(() => new Set());
  const toggleDetail = (id: string) =>
    setExpanded((open) => {
      const next = new Set(open);
      if (!next.delete(id)) next.add(id);
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
  // a route's own labels are a second set, keyed by the route's row id and
  // owned by its organization. they stay readable on the route's detail until
  // the two kinds are folded into one (#3685)
  const routeLabels = useSubjectLabels(scope.orgId, "route");
  const [labellingRoute, setLabellingRoute] = React.useState<RouteRow | null>(null);
  const [where, setWhere] = React.useState<WhereFilter>("all");
  const [unpricedOnly, setUnpricedOnly] = React.useState(false);
  const { sort, cycle, apply } = useSort<
    "name" | "provider" | "strategy" | "targets" | "traffic" | "origin"
  >();
  const [sheet, setSheet] = React.useState<{
    mode: ModelSheetMode;
    route?: RouteRow | null;
    configModel?: EffectiveModelDto | null;
    configTargets?: RouteTargetView[] | null;
  } | null>(null);
  // the route whose strategy, enabled flag, targets and variants are being
  // edited in the route sheet (#2134, #391)
  const [editingRoute, setEditingRoute] = React.useState<RouteRow | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<EffectiveModelDto | null>(null);
  // a route is the public name clients call; deleting one breaks them silently,
  // so it is confirmed by name before anything leaves (#1179)
  const [deleteRouteTarget, setDeleteRouteTarget] = React.useState<RouteRow | null>(null);

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
    const route = inProject(entry) ? (routeByModel.get(entry.model) ?? null) : null;
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
    const origin = entry.source === "config" ? "config" : "db";
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
      variants: route?.variants ?? [],
      strategy: entry.strategy,
      origin,
      where: origin === "config" ? "config" : inProject(entry) ? "project" : "other",
      locked: policy?.mode === "deny" || deny.length > 0,
      enabled: route?.enabled ?? true,
      // a price row carries the currency it was written in, so the cell says
      // what the operator actually configured rather than assuming dollars
      inPrice: price ? fmt.currency(Number(price.input_per_mtok), price.currency) : "—",
      outPrice: price ? fmt.currency(Number(price.output_per_mtok), price.currency) : "—",
      priced: pricesKnown ? !!price : null,
      traffic: inProject(entry) && traffic.isSuccess ? trafficFor(traffic.data, entry.model) : null,
    };
  });

  const q = search.trim().toLowerCase();
  const filtered = rows.filter(
    (r) =>
      (where === "all" || r.where === where) &&
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
    traffic: (r) => r.traffic?.requests ?? -1,
    origin: (r) => r.where,
  });

  const counts: Record<WhereFilter, number> = {
    all: rows.length,
    project: rows.filter((r) => r.where === "project").length,
    other: rows.filter((r) => r.where === "other").length,
    config: rows.filter((r) => r.where === "config").length,
  };

  const providerCount = new Set(rows.flatMap((r) => r.providerNames)).size;
  const unpricedCount = rows.filter((r) => r.priced === false).length;

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["models"] });
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
    queryClient.invalidateQueries({ queryKey: ["config"] });
    queryClient.invalidateQueries({ queryKey: ["cluster-nodes"] });
  };

  const removeModel = useMutation({
    mutationFn: (entry: EffectiveModelDto) => deleteModel(entry.model, entry.org_id),
    onSuccess: invalidate,
  });
  const removeRoute = useMutation({
    mutationFn: (id: string) => deleteRoute(id),
    onSuccess: invalidate,
  });

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  const filtersActive = !!q || where !== "all" || unpricedOnly || !!labelFilter;
  const clearFilters = () => {
    setSearch("");
    setWhere("all");
    setUnpricedOnly(false);
    setLabelFilter("");
  };

  return (
    <PageBody>
      <Toolbar>
        <SearchInput
          // full width on a phone rather than squeezed to a few characters
          // beside the origin filter (#3660)
          className="min-w-full max-w-full sm:min-w-[220px] sm:max-w-[320px]"
          placeholder={t("pages.models.searchPlaceholder")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <LabelFilterSelect value={labelFilter} onChange={setLabelFilter} options={labels.options} />
        {/* the tally waits for the catalog, which names each target's provider:
            unread, it would print a zero it has no evidence for (#1980). it
            counts routes, which is what a row is: one model name may be served
            by two organizations, and Routing Rules counted only this project's
            (#3685) */}
        <ListSummary data={models.data}>
          {() => (
            <>
              {t("pages.models.routeTally", { count: rows.length })}
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
        {/* a segment with nothing in it is not offered, apart from All: a
            project with no routes of its own still lists the config file's */}
        <Segmented<WhereFilter>
          ariaLabel={t("pages.models.originFilterAria")}
          value={where}
          onChange={setWhere}
          options={(["all", "project", "other", "config"] as const)
            .filter((key) => key === "all" || key === where || counts[key] > 0)
            .map((key) => ({
              value: key,
              label: models.data
                ? `${t(`pages.models.origin.${key}`)} ${counts[key]}`
                : t(`pages.models.origin.${key}`),
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

      {/* one line for what every unpriced row used to badge on its own (#3659) */}
      {unpricedCount > 0 && (
        <p className="text-xs text-muted-foreground">
          {t("pages.models.unpriced.summary", { count: unpricedCount })}{" "}
          <Link
            to="/pricing-overrides"
            className="text-[color:var(--text-secondary)] underline underline-offset-2 hover:text-foreground"
          >
            {t("pages.models.unpriced.setPrices")}
          </Link>
        </p>
      )}
      {nodes.isSuccess && Array.isArray(nodes.data) && <GatewayConfigNote nodes={nodes.data} />}
      {traffic.error instanceof AnalyticsUnavailableError && (
        <AnalyticsUnavailable error={traffic.error} i18nKey="pages.models.noAnalytics" />
      )}
      {traffic.isError && !(traffic.error instanceof AnalyticsUnavailableError) && (
        <p
          role="status"
          className="max-w-[72ch] text-xs leading-snug text-muted-foreground"
          data-testid="traffic-unavailable"
        >
          {t("pages.models.traffic.unavailable")}
          {traffic.error && (
            <span className="ml-1 font-mono text-[color:var(--text-subtle)]">
              {errorText(traffic.error, t)}
            </span>
          )}
        </p>
      )}
      {uptime.isError && (
        // one note for the whole screen, not "no health data" on every row; the
        // cause (no ClickHouse, a 403) is in the control plane's own words
        <p
          role="status"
          className="max-w-[72ch] text-xs leading-snug text-muted-foreground"
          data-testid="health-unavailable"
        >
          {t("pages.models.healthUnavailable")}
          {uptime.error && (
            <span className="ml-1 font-mono text-[color:var(--text-subtle)]">
              {errorText(uptime.error, t)}
            </span>
          )}
        </p>
      )}
      {models.error && (
        <LoadError
          error={models.error}
          resource={t("errors.resources.models")}
          onRetry={() => models.refetch()}
          target="models"
        />
      )}
      {/* the project's own routes are what its rows are edited, re-pointed
          and deleted through; unread, those rows still list but cannot act */}
      {routes.error && (
        // load-error-allow: the route list only adds edit and delete to the catalog's rows, whose own read pairs as models
        <LoadError
          error={routes.error}
          resource={t("errors.resources.routes")}
          onRetry={() => void routes.refetch()}
          target="routes"
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("common.scopeReadOnly", { reason: scopeMessage })}
        </p>
      )}

      <ListTable label={t("screens.model-catalog.title")} minWidth={1040}>
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
            label={t("pages.models.columns.traffic", { hours: TRAFFIC_WINDOW_HOURS })}
            col="traffic"
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
          const open = expanded.has(r.id);
          const detailId = `${detailPrefix}-${r.id}`;
          const hintKey = strategyHintKey(r.strategy);
          const withHealth = (r.targets ?? []).filter((tg) => tg.health);
          const failing = withHealth.filter((tg) => tg.health?.state === "breaching").length;
          const atRisk = withHealth.filter((tg) => tg.health?.state === "atRisk").length;
          const tripped = (r.targets ?? []).filter((tg) => tg.breaker);
          const openBreaker = tripped.find((tg) => tg.breaker?.state === "open")?.breaker;
          const viaVariants = r.variants.length > 0;
          return (
            <React.Fragment key={r.id}>
              <ListRow grid={GRID} className={open ? "border-b-0" : undefined}>
                <ListCell className="flex min-w-0 items-center gap-1.5">
                  {/* the row opens into the route beneath it, rather than a
                      popover the table's scroll frame would clip */}
                  <button
                    type="button"
                    aria-expanded={open}
                    aria-controls={open ? detailId : undefined}
                    aria-label={t("pages.models.detail.toggleAria", { model: r.name })}
                    onClick={() => toggleDetail(r.id)}
                    className="-ml-1 inline-flex h-6 w-6 flex-none items-center justify-center rounded-sm text-[color:var(--text-subtle)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  >
                    <ChevronRight
                      aria-hidden="true"
                      className={cn("h-3.5 w-3.5 transition-transform", open && "rotate-90")}
                    />
                  </button>
                  {/* health is the targets column's to say; this cell only
                      marks a model that is switched off (#3659) */}
                  <span className="flex min-w-0 flex-col gap-1">
                    <span className="truncate font-mono text-sm">{r.name}</span>
                    <LabelChips labels={labels.forModel(r.entry.org_id ?? "", r.name)} />
                    {!r.enabled && (
                      <Badge tone="outline" className="self-start">
                        {t("pages.models.disabled")}
                      </Badge>
                    )}
                  </span>
                  {r.locked && (
                    <InfoHint
                      icon={Lock}
                      label={t("pages.models.lockedLabel")}
                      text={t("pages.models.lockedHint")}
                      className="flex-none text-[color:var(--text-subtle)]"
                    />
                  )}
                </ListCell>
                <ListCell className="flex min-w-0 flex-col items-start gap-1">
                  <Tooltip
                    className="max-w-full truncate font-mono text-xs text-[color:var(--text-secondary)]"
                    content={r.providerNames.length > 1 ? r.providerNames.join("\n") : undefined}
                  >
                    {r.providerName}
                  </Tooltip>
                  {/* a verdict once one exists; "not probed yet" on every row
                      said nothing about the provider (#3659) */}
                  {r.cache && r.cache.status !== "unprobed" && (
                    <CacheReportingBadge state={r.cache} />
                  )}
                </ListCell>
                <ListCell className="flex min-w-0 flex-col items-start gap-0.5">
                  <StrategyBadge strategy={r.strategy} />
                  {/* the caveat's name is written in the row, and its sentence,
                      which opens with the targets below, is its tooltip */}
                  {hintKey && (
                    <Tooltip
                      className="truncate text-2xs text-[color:var(--status-warning)]"
                      content={t(hintKey)}
                    >
                      {t(CAVEAT_LABEL[hintKey] ?? "pages.models.strategyCaveat.other")}
                    </Tooltip>
                  )}
                </ListCell>
                <ListCell className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5">
                  {/* a route with variants routes through them and not its own
                      targets, so the count is theirs and the targets' health
                      is not the route's to report */}
                  {viaVariants ? (
                    <span className="font-mono text-xs text-[color:var(--text-secondary)]">
                      {t("pages.models.variants.count", { count: r.variants.length })}
                    </span>
                  ) : (
                    <>
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
                      {/* health where the rollup has it: a failing target is
                          named in the row, a clean bill is a dot, and no data
                          says nothing */}
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
                    </>
                  )}
                </ListCell>
                <ListCell className="grid">
                  <RouteTrafficCell
                    traffic={traffic}
                    inProject={r.where === "project"}
                    model={r.name}
                  />
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
                    // where the row is stored is not a status, so it takes no
                    // status hue, and it reads in the dashboard's language. it
                    // stays on one line, widening its column rather than
                    // breaking "this project" in two at the table's floor
                    <Pill
                      color="var(--text-secondary)"
                      tint="var(--surface-subtle)"
                      border="var(--border-default)"
                      className="whitespace-nowrap"
                    >
                      {t(`pages.models.where.${r.where}`)}
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
                  {/* the scope project's route is deleted as that route, the
                  same admin capability adding one is; another project's
                  stored model is forgotten outright, which is
                  deployment-wide (#1258). a project row whose route was not
                  read offers neither */}
                  {r.route ? (
                    <DeleteIconButton
                      gate="route:delete"
                      control="route-delete"
                      label={t("pages.models.deleteRouteAria", { model: r.name })}
                      pending={removeRoute.isPending && removeRoute.variables === r.route.id}
                      onClick={() => {
                        removeRoute.reset();
                        setDeleteRouteTarget(r.route);
                      }}
                    />
                  ) : (
                    r.where === "other" && (
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
                    )
                  )}
                </ListCell>
              </ListRow>
              {open && (
                <div
                  role="row"
                  className="border-b border-[color:var(--border-subtle)] pb-4 pl-[44px] pr-4 last:border-b-0"
                >
                  <div role="cell" id={detailId} className="flex flex-col gap-3">
                    <StrategyHint strategy={r.strategy} />
                    {viaVariants && r.route ? (
                      <RouteVariants
                        route={r.route}
                        variants={r.variants}
                        unusedTargets={r.route.targets?.length ?? 0}
                        providerName={providerName}
                      />
                    ) : r.targets === null ? (
                      <p className="text-xs text-[color:var(--text-subtle)]">
                        {t("pages.models.detail.targetsUnread")}
                      </p>
                    ) : r.targets.length === 0 ? (
                      <EmptyState
                        inline
                        uxTarget="route-targets"
                        description={t("pages.models.detail.noTargets")}
                      />
                    ) : (
                      <RouteTargetList
                        label={t("routeTargets.listLabel", { model: r.name })}
                        strategy={r.strategy}
                        targets={r.targets}
                        // a column of "no health data" says nothing a missing
                        // column does not, so it shows once any target has data
                        health={withHealth.length > 0}
                      />
                    )}
                    {r.route && <LabelChips labels={routeLabels.bySubject(r.route.id)} />}
                    {/* the actions are one cluster that wraps as a whole: one
                        by one, the last button fell onto its own line in
                        Russian (#3660) */}
                    <div className="flex flex-wrap items-center gap-2">
                      {/* the Playground opens on this model. a disabled route
                          is not served, and another project's route is not
                          the scope's key's to call, so neither offers it */}
                      {r.enabled && r.where !== "other" && (
                        <Link
                          to={`/playground?model=${encodeURIComponent(r.name)}`}
                          aria-label={t("pages.models.detail.tryRoute", { model: r.name })}
                          className={cn(
                            buttonVariants({ variant: "outline", size: "sm" }),
                            "h-[30px] gap-1.5",
                          )}
                        >
                          <Play className="h-3.5 w-3.5" />
                          {t("pages.models.detail.tryIt")}
                        </Link>
                      )}
                      <CopyButton
                        value={r.name}
                        label={t("pages.models.detail.copyModel")}
                        className="h-[30px] px-2"
                      />
                      {r.route && (
                        <>
                          {/* the label names the route, so a list of open rows
                              does not expose buttons a screen reader cannot
                              tell apart (#1214) */}
                          <GatedButton
                            gate="route:update"
                            control="route-edit"
                            size="sm"
                            variant="outline"
                            className="h-[30px]"
                            aria-label={t("pages.models.detail.editRoutingAria", {
                              model: r.name,
                            })}
                            onClick={() => setEditingRoute(r.route)}
                          >
                            <Pencil className="h-3.5 w-3.5" />
                            {t("pages.models.detail.editRouting")}
                          </GatedButton>
                          {scope.orgId && (
                            <Button
                              size="sm"
                              variant="outline"
                              className="h-[30px] gap-1.5"
                              aria-label={t("pages.models.detail.routeLabelsAria", {
                                model: r.name,
                              })}
                              onClick={() => setLabellingRoute(r.route)}
                            >
                              <Tag className="h-3.5 w-3.5" />
                              {t("pages.models.detail.routeLabels")}
                            </Button>
                          )}
                        </>
                      )}
                    </div>
                  </div>
                </div>
              )}
            </React.Fragment>
          );
        })}
        {/* "no rows" and "nothing matched the filters" are different answers:
            one wants a route created, the other wants the filter cleared */}
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

      {/* routes are project-scoped rows, but a label lives on the org that owns them */}
      {scope.orgId && labellingRoute && (
        <LabelSheet
          open
          onOpenChange={(open) => !open && setLabellingRoute(null)}
          orgId={scope.orgId}
          subjectType="route"
          subjectId={labellingRoute.id}
          subjectName={labellingRoute.model}
        />
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
        // the variants are edited in the route sheet, so a save refused for
        // them hands the route over to it
        onEditVariants={
          sheet?.route
            ? () => {
                const route = sheet.route ?? null;
                setSheet(null);
                setEditingRoute(route);
              }
            : undefined
        }
      />

      <RouteSheet
        open={!!editingRoute}
        onOpenChange={(open) => !open && setEditingRoute(null)}
        route={editingRoute}
        providers={providers.data ?? []}
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

      <ConfirmDialog
        name="route-delete"
        open={!!deleteRouteTarget}
        onOpenChange={(open) => {
          if (!open) {
            removeRoute.reset();
            setDeleteRouteTarget(null);
          }
        }}
        title={t("pages.models.confirm.routeTitle", { model: deleteRouteTarget?.model ?? "" })}
        description={t("pages.models.confirm.routeBody")}
        confirmLabel={t("pages.models.confirm.routeConfirm")}
        pending={removeRoute.isPending}
        error={removeRoute.error}
        onConfirm={() => {
          if (!deleteRouteTarget) return;
          const what = deleteRouteTarget.model;
          removeRoute.mutate(deleteRouteTarget.id, {
            onSuccess: () => {
              setDeleteRouteTarget(null);
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
