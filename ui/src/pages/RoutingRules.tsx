import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { Pencil, Play, Route, Tag } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CopyButton } from "@/components/CopyButton";
import { GatedButton } from "@/components/GatedButton";
import { ModelSheet } from "@/components/ModelSheet";
import { RouteSheet } from "@/components/RouteSheet";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { Button, buttonVariants } from "@/components/ui/button";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, LoadingRegion } from "@/components/LoadingState";
import { PageBody, StatusDot, Toolbar } from "@/components/screen";
import { BreakerBadge } from "@/components/BreakerBadge";
import { FallbackBadge } from "@/components/TakesOverField";
import { healthPresentation } from "@/components/target-health";
import { EmptyState } from "@/components/ui/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { LabelChips, LabelFilterSelect, LabelSheet, useSubjectLabels } from "@/components/Labels";
import {
  AnalyticsUnavailableError,
  deleteRoute,
  fetchAnalyticsByModel,
  fetchClusterNodes,
  fetchHealthTimeline,
  useBreakerState,
  breakerFor,
  type BreakerStateDto,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  fetchUptime,
  type AnalyticsByModelRow,
  type ClusterNodeRow,
  type RouteRow,
  type RouteTargetRow,
  type RouteVariantRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { isRestricted } from "@/lib/failure-classes";
import { useFormat } from "@/lib/i18n/format";
import { HEALTH_SLA, targetHealth, type TargetHealth } from "@/lib/route-targets";
import { trafficSplit } from "@/lib/route-variants";
import { shareBarColor } from "@/lib/share-bars";
import { useScope } from "@/lib/scope";
import { StrategyBadge } from "@/components/StrategyBadge";
import { usesWeights } from "@/lib/strategies";
import { useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useScreenReady } from "@/lib/ux-react";

/** how far back a card counts requests, in hours */
const TRAFFIC_WINDOW_HOURS = 24;
/** how often the gateway list is asked again while one has not caught up */
const NODES_POLL_MS = 10_000;

/** one route's requests and errors over the window, from the by-model rollup */
interface RouteTraffic {
  requests: number;
  errors: number;
}

/**
 * the rollup keys on the public model name and is read with the route's
 * project filter (#3101), so two projects that define a route with one model
 * name each see their own figure; a route nothing called has no row. the name
 * alone is a safe key here even though two organizations may share it
 * (#1857): a project belongs to one organization, so the filtered read holds
 * no other organization's rows (#3460)
 */
function trafficFor(rows: AnalyticsByModelRow[], model: string): RouteTraffic {
  const row = rows.find((r) => r.model === model);
  return { requests: Number(row?.requests ?? 0), errors: Number(row?.errors ?? 0) };
}

/** the live gateways, and how many of them have not reached the latest config */
function gatewayConvergence(nodes: ClusterNodeRow[]): { live: number; lagging: number } {
  const live = nodes.filter(
    (n) => n.role === "gateway" && n.live && n.desired_state !== "draining",
  );
  return { live: live.length, lagging: live.filter((n) => !n.converged).length };
}

/**
 * What is known about one route's targets.
 *
 * They arrive inside the route list (`include=targets`), so a route that is on
 * screen already has its answer: the list's own loading and error states cover
 * the targets too, and no card waits on a read of its own. `unread` is a route
 * the control plane returned without the field, which only a control plane that
 * predates it does. Folding it into an empty list said "No targets yet" and
 * "0 targets" about a route whose targets were never read (#2133).
 */
type TargetsState = { kind: "unread" } | { kind: "loaded"; targets: RouteTargetRow[] };

// routing rules: one card per route with its strategy pill, its targets (and
// their weight shares where the strategy reads weights), and label/delete
// actions
export default function RoutingRules() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();
  // deleting a route is the same admin capability adding one is (#1258)

  const routes = useQuery({
    // the targets ride along, so the screen costs one request however many
    // routes the project has (#3031). the key sits under ["routes", project],
    // which every route mutation already invalidates
    queryKey: ["routes", scope.projectId, "targets"],
    // the variants ride along too (#3343): a route that has them routes through
    // them, and its own targets are not used
    queryFn: () => fetchRoutes(scope.projectId as string, ["targets", "variants"]),
    enabled: !!scope.projectId,
  });

  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  // the 7-day uptime rollup, under the key Model Catalog and Health share, so
  // the three screens read one cached answer. It needs ClickHouse and analytics
  // access; where either is missing the screen says so once and the cards
  // still render
  const uptime = useQuery({
    queryKey: ["health-uptime", HEALTH_SLA],
    queryFn: async () => (await fetchUptime(HEALTH_SLA)) ?? [],
    enabled: (routes.data?.length ?? 0) > 0,
    retry: false,
  });
  // the hourly timeline tells "within SLA" from "SLA at risk"; optional, so a
  // target is judged on the rollup alone where it cannot be read
  const timeline = useQuery({
    queryKey: ["health-timeline"],
    queryFn: async () => (await fetchHealthTimeline("hour").catch(() => [])) ?? [],
    enabled: uptime.isSuccess,
    retry: false,
  });

  // requests and errors per public model over the last day. it needs
  // ClickHouse, which a deployment may run without: that is an answer the
  // screen states, never a zero on every card
  const traffic = useQuery({
    queryKey: ["route-traffic", scope.projectId, TRAFFIC_WINDOW_HOURS],
    queryFn: async () => {
      const since = new Date(Date.now() - TRAFFIC_WINDOW_HOURS * 3_600_000).toISOString();
      const rows = await fetchAnalyticsByModel({ since, project: scope.projectId as string });
      return Array.isArray(rows) ? rows : [];
    },
    enabled: (routes.data?.length ?? 0) > 0,
    retry: false,
  });
  // whether the gateways have picked the change up: the cluster inventory
  // Cluster reads, under its key. only a gateway that is behind is polled
  const nodes = useQuery({
    queryKey: ["cluster-nodes"],
    queryFn: fetchClusterNodes,
    enabled: (routes.data?.length ?? 0) > 0,
    retry: false,
    refetchInterval: (query) =>
      Array.isArray(query.state.data) && gatewayConvergence(query.state.data).lagging > 0
        ? NODES_POLL_MS
        : false,
  });

  // the gateways' live breakers, beside each target's health where one is
  // tripped; a failed read leaves the cards as they were
  const breakers = useBreakerState();

  const targetsByRoute = new Map<string, TargetsState>();
  for (const r of routes.data ?? []) {
    targetsByRoute.set(
      r.id,
      r.targets ? { kind: "loaded", targets: r.targets } : { kind: "unread" },
    );
  }

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider.
  // the route list carries the targets, so it is the one thing the reader waits
  // on. the error signal is its `LoadError`'s own
  useScreenReady(!routes.isLoading);

  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.name ?? id.slice(0, 8);
  // a stored target names its provider by id, so its organization is known
  // exactly; another organization's provider of the same name has health of
  // its own (#3460)
  const providerOrg = (id: string) => providers.data?.find((p) => p.id === id)?.org_id;

  // a target's health is looked up by provider name, which the rollups key on,
  // so it waits for the provider list as well as for the rollup: a row judged
  // on an id fallback would read "no health data" about a target never asked
  // after
  const healthFor =
    uptime.isSuccess && providers.isSuccess && Array.isArray(uptime.data)
      ? (tg: RouteTargetRow, route: RouteRow): TargetHealth | null =>
          targetHealth(
            uptime.data,
            providerName(tg.provider_id),
            tg.upstream_model || route.model,
            timeline.data,
            providerOrg(tg.provider_id),
          )
      : undefined;

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
    queryClient.invalidateQueries({ queryKey: ["models"] });
    queryClient.invalidateQueries({ queryKey: ["config"] });
    queryClient.invalidateQueries({ queryKey: ["cluster-nodes"] });
  };

  const remove = useMutation({
    mutationFn: (id: string) => deleteRoute(id),
    onSuccess: invalidate,
  });

  // a route is created through the model sheet, the same one Model Catalog
  // opens, so the strategy, the targets and everything else about a new model
  // are set in one place rather than in two forms that disagreed (#1979). the
  // catalog is read only for the sheet's name-conflict check
  const [addOpen, setAddOpen] = React.useState(false);
  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels, enabled: addOpen });
  // a route is the public name clients call; deleting one breaks them silently,
  // so it is confirmed by name before anything leaves (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<RouteRow | null>(null);
  // the route whose strategy, enabled flag and targets are being edited (#2134)
  const [editing, setEditing] = React.useState<RouteRow | null>(null);
  const [labelFilter, setLabelFilter] = React.useState("");
  const [labelling, setLabelling] = React.useState<RouteRow | null>(null);
  // routes are project-scoped rows, but a label lives on the org that owns them
  const labels = useSubjectLabels(scope.orgId, "route");
  // reset first: an error left over from a previous failed delete would
  // otherwise greet the next route the operator picks
  const startDelete = (route: RouteRow) => {
    remove.reset();
    setDeleteTarget(route);
  };

  const shown = (routes.data ?? []).filter((r) => labels.matches(r.id, labelFilter));

  return (
    <PageBody>
      <Toolbar>
        {/* a count needs an answer: "0 routes" while the list was still out,
            or under a 403, stated an empty project nobody had seen (#2133) */}
        {routes.isSuccess && (
          <span className="text-sm text-muted-foreground">
            {t("pages.routing.summary", { count: routes.data.length })}
          </span>
        )}
        <LabelFilterSelect value={labelFilter} onChange={setLabelFilter} options={labels.options} />
        <GatedButton
          gate="route:create"
          control="route-new"
          className="ml-auto"
          onClick={() => setAddOpen(true)}
          disabled={!scope.projectId}
        >
          {t("pages.routing.add")}
        </GatedButton>
      </Toolbar>

      {/* pending, not loading, so a retry parked in a hidden tab keeps the
          skeleton; the project guard keeps it off a scope with no project,
          where the query never runs */}
      {routes.isPending && !!scope.projectId && (
        <CardGridSkeleton cards={3} height={196} min={360} />
      )}
      {nodes.isSuccess && Array.isArray(nodes.data) && <GatewayConfigNote nodes={nodes.data} />}
      {traffic.error instanceof AnalyticsUnavailableError && (
        <AnalyticsUnavailable error={traffic.error} i18nKey="pages.routing.noAnalytics" />
      )}
      {traffic.isError && !(traffic.error instanceof AnalyticsUnavailableError) && (
        <p
          role="status"
          className="max-w-[72ch] text-xs leading-snug text-muted-foreground"
          data-testid="traffic-unavailable"
        >
          {t("pages.routing.trafficUnavailable")}
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
          {t("pages.routing.healthUnavailable")}
          {uptime.error && (
            <span className="ml-1 font-mono text-[color:var(--text-subtle)]">
              {errorText(uptime.error, t)}
            </span>
          )}
        </p>
      )}
      {routes.error && (
        <LoadError
          error={routes.error}
          resource={t("errors.resources.routes")}
          onRetry={() => void routes.refetch()}
          target="routes"
        />
      )}
      {routes.isSuccess && shown.length === 0 && (
        // a label filter that matches nothing is not a project with no routes:
        // the copy blames the narrowing and offers to clear it rather than
        // offering to add the first route to a project that already has some
        <EmptyState
          thread
          uxTarget="routes"
          icon={<Route />}
          title={labelFilter ? t("pages.routing.noMatchTitle") : t("pages.routing.emptyTitle")}
          description={labelFilter ? t("pages.routing.noMatchBody") : t("pages.routing.emptyBody")}
          actions={
            labelFilter ? (
              <Button variant="outline" onClick={() => setLabelFilter("")}>
                {t("common.clearFilters")}
              </Button>
            ) : (
              <GatedButton
                gate="route:create"
                control="route-new-empty"
                disabled={!scope.projectId}
                onClick={() => setAddOpen(true)}
              >
                {t("pages.routing.emptyAction")}
              </GatedButton>
            )
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(360px,100%),1fr))]">
        {shown.map((r) => {
          const targets: TargetsState = targetsByRoute.get(r.id) ?? { kind: "unread" };
          return (
            <div
              key={r.id}
              className="flex flex-col gap-3.5 rounded-lg border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5">
                {/* the name engineers paste into clients; the badges wrap under
                    it instead of squeezing it to an ellipsis at 375px (#2139) */}
                <span className="flex min-w-0 max-w-full items-center gap-0.5">
                  <span className="min-w-0 truncate font-mono text-sm font-semibold">
                    {r.model}
                  </span>
                  <CopyButton
                    value={r.model}
                    label={t("pages.routing.copyModel")}
                    className="h-6 px-1"
                  />
                </span>
                <StrategyBadge strategy={r.strategy} />
                <LabelChips labels={labels.bySubject(r.id)} />
                {!r.enabled && (
                  <span className="rounded-sm bg-[color:var(--surface-subtle)] px-2 py-[3px] font-mono text-2xs uppercase text-[color:var(--text-subtle)]">
                    {t("pages.routing.disabledBadge")}
                  </span>
                )}
              </div>
              {r.variants && r.variants.length > 0 ? (
                <RouteVariants
                  route={r}
                  variants={r.variants}
                  unusedTargets={targets.kind === "loaded" ? targets.targets.length : 0}
                  providerName={providerName}
                />
              ) : (
                <RouteTargets
                  route={r}
                  state={targets}
                  providerName={providerName}
                  healthFor={healthFor}
                  breakers={breakers.data}
                  orgId={scope.orgId}
                />
              )}
              <RouteTrafficLine traffic={traffic} model={r.model} />
              <div className="flex flex-wrap items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                {/* the count needs an answer too: a pending or failed read is
                    not a route with no targets */}
                {r.variants && r.variants.length > 0 ? (
                  <span className="text-xs text-[color:var(--text-subtle)]">
                    {t("pages.routing.variantCount", { count: r.variants.length })}
                  </span>
                ) : (
                  targets.kind === "loaded" && (
                    <span className="text-xs text-[color:var(--text-subtle)]">
                      {t("pages.routing.targetCount", { count: targets.targets.length })}
                    </span>
                  )
                )}
                {/* the Playground opens on this route's model. a disabled route
                    is not served, so a request to it would only fail */}
                {r.enabled && (
                  <Link
                    to={`/playground?model=${encodeURIComponent(r.model)}`}
                    aria-label={t("pages.routing.tryRoute", { model: r.model })}
                    className={cn(
                      buttonVariants({ variant: "outline", size: "sm" }),
                      "ml-auto h-[30px] gap-1.5",
                    )}
                  >
                    <Play className="h-3.5 w-3.5" />
                    {t("pages.routing.tryIt")}
                  </Link>
                )}
                {/* the label names the route, so a grid of cards does not
                    expose N buttons a screen reader cannot tell apart (#1214) */}
                <GatedButton
                  gate="route:update"
                  control="route-edit"
                  size="sm"
                  variant="outline"
                  className={cn("h-[30px]", !r.enabled && "ml-auto")}
                  aria-label={t("pages.routing.editRoute", { model: r.model })}
                  onClick={() => setEditing(r)}
                >
                  <Pencil className="h-3.5 w-3.5" />
                  {t("pages.routing.edit")}
                </GatedButton>
                <Button
                  size="sm"
                  variant="outline"
                  className="h-[30px]"
                  aria-label={t("labels.labelsOf", { name: r.model })}
                  onClick={() => setLabelling(r)}
                >
                  <Tag className="h-3.5 w-3.5" />
                </Button>
                <DeleteIconButton
                  gate="route:delete"
                  control="route-delete"
                  label={t("pages.routing.deleteRoute", { model: r.model })}
                  pending={remove.isPending && remove.variables === r.id}
                  onClick={() => startDelete(r)}
                />
              </div>
            </div>
          );
        })}
      </div>
      {remove.isError && !deleteTarget && (
        <p className="text-xs text-[color:var(--status-danger-text)]">
          {errorText(remove.error, t)}
        </p>
      )}

      {scope.orgId && labelling && (
        <LabelSheet
          open
          onOpenChange={(open) => !open && setLabelling(null)}
          orgId={scope.orgId}
          subjectType="route"
          subjectId={labelling.id}
          subjectName={labelling.model}
        />
      )}

      <ConfirmDialog
        name="route-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.routing.confirm.title", { model: deleteTarget?.model })}
        description={t("pages.routing.confirm.body")}
        confirmLabel={t("pages.routing.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.model;
          remove.mutate(deleteTarget.id, {
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

      <RouteSheet
        open={!!editing}
        onOpenChange={(open) => !open && setEditing(null)}
        route={editing}
        providers={providers.data ?? []}
        onDone={invalidate}
      />

      <ModelSheet
        open={addOpen}
        mode="add"
        onOpenChange={setAddOpen}
        projectId={scope.projectId ?? null}
        orgId={scope.orgId ?? null}
        providers={providers.data ?? []}
        models={models.data ?? []}
        routes={routes.data ?? []}
        onDone={invalidate}
      />
    </PageBody>
  );
}

/**
 * Whether the gateways have the config the control plane holds, from the
 * cluster inventory (PRODUCT.md principle 5: say what was saved and when it
 * reached the gateway). A lagging gateway links Cluster, which names it; no
 * live gateway says nothing, since there is nothing to have caught up.
 */
function GatewayConfigNote({ nodes }: { nodes: ClusterNodeRow[] }) {
  const { t } = useTranslation();
  const { live, lagging } = gatewayConvergence(nodes);
  if (live === 0) return null;
  return (
    <p
      role="status"
      data-testid="gateway-config"
      className={cn(
        "max-w-[72ch] text-xs leading-snug",
        lagging > 0 ? "text-[color:var(--status-warning-text)]" : "text-muted-foreground",
      )}
    >
      {lagging > 0
        ? t("pages.routing.gatewaysBehind", { count: lagging, live })
        : t("pages.routing.gatewaysCurrent", { count: live })}{" "}
      <Link to="/cluster" className="underline underline-offset-2">
        {t("pages.routing.gatewaysLink")}
      </Link>
    </p>
  );
}

/**
 * A route's requests and error share over the last day. Absent while the
 * rollup is out or where it could not be read; a route nobody called says so
 * in words, since that is an answer the rollup gave.
 */
function RouteTrafficLine({
  traffic,
  model,
}: {
  traffic: UseQueryResult<AnalyticsByModelRow[]>;
  model: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (traffic.isPending) {
    return (
      <LoadingRegion testId="route-traffic-loading">
        <Skeleton width="60%" height={14} radius={4} />
      </LoadingRegion>
    );
  }
  if (!traffic.isSuccess || !traffic.data) return null;
  const { requests, errors } = trafficFor(traffic.data, model);
  return (
    <p data-testid="route-traffic" className="text-xs text-[color:var(--text-secondary)]">
      {requests === 0
        ? t("pages.routing.noTraffic", { hours: TRAFFIC_WINDOW_HOURS })
        : t("pages.routing.traffic", {
            requests: fmt.number(requests),
            count: requests,
            errors: fmt.percent(errors / requests, 1),
            hours: TRAFFIC_WINDOW_HOURS,
          })}
    </p>
  );
}

/**
 * One target's uptime against the SLA, in words beside the dot. The state is
 * the 7-day rollup's, not the gateway's live breaker, which is drawn beside it; a target nothing observed says so.
 */
function TargetHealthLine({ health }: { health: TargetHealth | null }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const view = healthPresentation(health);
  return (
    <div className={`flex items-center gap-1.5 text-xs ${view.textClass}`}>
      <StatusDot color={view.color} />
      <span className="min-w-0">
        {t(view.labelKey, { uptime: health ? fmt.percent(health.uptime, 2) : "" })}
      </span>
    </div>
  );
}

/**
 * The target half of a route card, for whichever answer the card holds.
 *
 * A target's health is drawn only where the uptime rollup was read; the dot
 * that stood here once was always green, so a target behind an open breaker
 * read as healthy (#2133). A share of the route is drawn only under a strategy that reads
 * weights; under any other the balancer never consults them, so the card says
 * that once instead of printing a split the gateway does not make.
 */
function RouteTargets({
  route,
  state,
  providerName,
  healthFor,
  breakers,
  orgId,
}: {
  route: RouteRow;
  state: TargetsState;
  providerName: (id: string) => string;
  /** absent until the rollup and the provider names are both known */
  healthFor?: (target: RouteTargetRow, route: RouteRow) => TargetHealth | null;
  /** the gateways' live breakers, matched on the route's model and the target's position */
  breakers?: Pick<BreakerStateDto, "targets">;
  /**
   * the organization the route belongs to: another organization's route of
   * the same name has breakers of its own (#3460)
   */
  orgId?: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const weighted = usesWeights(route.strategy);

  if (state.kind === "unread") {
    return (
      <p className="text-xs text-[color:var(--text-subtle)]">{t("pages.routing.targetsUnread")}</p>
    );
  }

  const { targets } = state;
  if (targets.length === 0) {
    return (
      <EmptyState inline uxTarget="route-targets" description={t("pages.routing.noTargets")} />
    );
  }
  // a fallback takes no ordinary traffic, so it neither has a share nor
  // dilutes the others' (#3319)
  const totalWeight =
    targets.filter((tg) => !isRestricted(tg.takes_over)).reduce((a, tg) => a + tg.weight, 0) || 1;
  return (
    <div className="flex flex-col gap-2.5">
      {!weighted && targets.length > 1 && (
        <p className="text-xs leading-snug text-muted-foreground">
          <Trans
            i18nKey="routeTargets.weightsIgnored"
            values={{ strategy: route.strategy }}
            components={[<span key="strategy" className="font-mono text-foreground" />]}
          />
        </p>
      )}
      <ul
        aria-label={t("routeTargets.listLabel", { model: route.model })}
        className="flex flex-col gap-2.5"
      >
        {targets.map((tg, i) => {
          const fallback = isRestricted(tg.takes_over);
          const share = tg.weight / totalWeight;
          return (
            <li key={tg.id} className="flex flex-col gap-[5px]">
              <div className="flex items-center gap-2 font-mono text-xs">
                <span className="text-[color:var(--text-secondary)]">
                  {providerName(tg.provider_id)}
                </span>
                <span className="text-[color:var(--text-subtle)]">→</span>
                <span className="min-w-0 truncate text-muted-foreground">
                  {tg.upstream_model || route.model}
                </span>
                {fallback ? (
                  <span className="ml-auto">
                    <FallbackBadge takesOver={tg.takes_over ?? []} />
                  </span>
                ) : (
                  weighted && (
                    <span className="ml-auto text-[color:var(--text-secondary)]">
                      {fmt.percent(share, 0)}
                    </span>
                  )
                )}
              </div>
              {healthFor && <TargetHealthLine health={healthFor(tg, route)} />}
              <BreakerBadge
                breaker={breakerFor(breakers, route.model, i, orgId)}
                className="self-start"
              />
              {weighted && !fallback && (
                <div
                  data-testid="target-share"
                  className="h-[5px] overflow-hidden rounded-full bg-[color:var(--surface-subtle)]"
                >
                  <div
                    className="h-full rounded-full"
                    style={{
                      // a CSS length, never a localized percentage
                      width: `${Math.round(share * 100)}%`,
                      // the nth of several targets takes the nth categorical
                      // token, so no card is red by default and every fill
                      // clears the shape floor (#2139)
                      background: shareBarColor(i),
                    }}
                  />
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * A route that routes through weighted variants (#3343, #391).
 *
 * The gateway samples one variant by weight and tries the others in the order
 * they are declared when it fails, and while a route has variants its own
 * target list is not used. So the card lists the variants in that fallback
 * order with the share each one's weight gives it (computed the way the
 * gateway samples, `trafficSplit`), each with the targets it tries, and says
 * once how many of the route's own targets are being set aside rather than
 * drawing a split the gateway does not make. A route with variants and no
 * targets of its own never reads as having none.
 */
function RouteVariants({
  route,
  variants,
  unusedTargets,
  providerName,
}: {
  route: RouteRow;
  variants: RouteVariantRow[];
  /** how many of the route's own targets the variants set aside */
  unusedTargets: number;
  providerName: (id: string) => string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const ordered = [...variants].sort((a, b) => a.position - b.position);
  const shares = trafficSplit(ordered.map((v) => v.weight));
  return (
    <div className="flex flex-col gap-2.5">
      {unusedTargets > 0 && (
        <p className="text-xs leading-snug text-muted-foreground">
          {t("pages.routing.targetsUnused", { count: unusedTargets })}
        </p>
      )}
      <ol
        aria-label={t("pages.routing.variantsLabel", { model: route.model })}
        className="flex flex-col gap-2.5"
      >
        {ordered.map((v, i) => (
          <li key={v.id} className="flex flex-col gap-[5px]">
            <div className="flex items-center gap-2 font-mono text-xs">
              <span className="min-w-0 truncate text-[color:var(--text-secondary)]">{v.name}</span>
              <span className="ml-auto text-[color:var(--text-secondary)]">
                {fmt.percent(shares[i] ?? 0, 0)}
              </span>
            </div>
            <div
              data-testid="variant-share"
              className="h-[5px] overflow-hidden rounded-full bg-[color:var(--surface-subtle)]"
            >
              <div
                className="h-full rounded-full"
                style={{
                  // a CSS length, never a localized percentage
                  width: `${Math.round((shares[i] ?? 0) * 100)}%`,
                  background: shareBarColor(i),
                }}
              />
            </div>
            <p className="truncate font-mono text-2xs text-[color:var(--text-subtle)]">
              {[...v.targets]
                .sort((a, b) => a.position - b.position)
                .map(
                  (tg) => `${providerName(tg.provider_id)} → ${tg.upstream_model || route.model}`,
                )
                .join(", ")}
            </p>
          </li>
        ))}
      </ol>
    </div>
  );
}
