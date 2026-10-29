import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Activity, ChevronRight, Network, PowerOff } from "lucide-react";
import type * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { OwnerOrgPill, sharedNames, useOwnerLabel } from "@/components/OwnerOrgPill";
import { PanelSkeleton, StatGridSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { EmptyState, EmptyStateLink } from "@/components/ui/empty-state";
import { Table, type TableColumn } from "@/components/ui/table";
import { STAT_GRID, StatCard } from "@/components/ui/stat-card";
import {
  fetchAdaptiveRoutingPolicy,
  fetchAdaptiveRoutingTelemetry,
  type AdaptiveDecisionCountsDto,
  type AdaptiveNodeTelemetryDto,
  type AdaptiveRouteTelemetryDto,
  type AdaptiveTargetTelemetryDto,
} from "@/lib/api";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { useScreenReady } from "@/lib/ux-react";

// scores and weights are read against each other, so they keep three decimals
const DECIMAL: Intl.NumberFormatOptions = { maximumFractionDigits: 3 };

function totalDecisions(decisions: AdaptiveDecisionCountsDto): number {
  return decisions.blend + decisions.exploration + decisions.fallback;
}

// a CSS length for the decision bar — never a localized percentage
function barWidth(value: number, total: number): string {
  return `${total > 0 ? Math.round((value / total) * 100) : 0}%`;
}

function share(fmt: Formatters, value: number, total: number): string {
  return fmt.percent(total > 0 ? value / total : 0, 0);
}

function formatLatency(t: TFunction, fmt: Formatters, value?: number): string {
  if (!value || value <= 0) return t("pages.adaptiveDashboard.noSamples");
  return t("pages.adaptiveDashboard.latencyMs", { value: fmt.number(value, DECIMAL) });
}

// money per million tokens; the column header carries the "/ Mtok"
function formatCost(t: TFunction, fmt: Formatters, value?: number): string {
  if (!value || value <= 0) return t("pages.adaptiveDashboard.unknownCost");
  return fmt.currency(value);
}

function policySummary(t: TFunction, fmt: Formatters, node: AdaptiveNodeTelemetryDto): string {
  const policy = node.policy;
  const weight = (key: string, value: number) => t(key, { value: fmt.number(value, DECIMAL) });
  const weights = [
    policy.latency_weight != null
      ? weight("pages.adaptiveDashboard.policy.latencyWeight", policy.latency_weight)
      : null,
    policy.cost_weight != null
      ? weight("pages.adaptiveDashboard.policy.costWeight", policy.cost_weight)
      : null,
    policy.load_weight != null
      ? weight("pages.adaptiveDashboard.policy.loadWeight", policy.load_weight)
      : null,
  ].filter(Boolean);
  const details =
    weights.length > 0 ? weights.join(" / ") : t("pages.adaptiveDashboard.policy.unavailable");
  const summary =
    policy.min_samples == null
      ? details
      : t("pages.adaptiveDashboard.policy.withWarmUp", {
          details,
          count: policy.min_samples,
          value: fmt.number(policy.min_samples),
        });
  return policy.enabled === false
    ? t("pages.adaptiveDashboard.policy.disabled", { summary })
    : summary;
}

function routeIsDisabled(route: AdaptiveRouteTelemetryDto): boolean {
  return route.nodes.length > 0 && route.nodes.every((node) => node.policy.enabled === false);
}

function nodeState(t: TFunction, node: AdaptiveNodeTelemetryDto): string {
  if (node.policy.enabled === false) return t("pages.adaptiveDashboard.nodeState.disabled");
  return node.engaged
    ? t("pages.adaptiveDashboard.nodeState.routing")
    : t("pages.adaptiveDashboard.nodeState.warming");
}

function routeState(t: TFunction, route: AdaptiveRouteTelemetryDto): string {
  if (route.engaged) return t("pages.adaptiveDashboard.routeState.blend");
  return routeIsDisabled(route)
    ? t("pages.adaptiveDashboard.routeState.disabled")
    : t("pages.adaptiveDashboard.routeState.warming");
}

// the target the blend currently prefers: the one alone on the highest score.
// the balancer takes the argmax of the blend and breaks a tie at random, so a
// shared top score prefers nobody, and a node still warming up is not blending
function preferredTarget(node: AdaptiveNodeTelemetryDto): number | null {
  if (!node.engaged) return null;
  const scored = node.targets.filter((target) => target.score != null);
  if (scored.length < 2) return null;
  const best = Math.max(...scored.map((target) => target.score as number));
  const leaders = scored.filter((target) => target.score === best);
  return leaders.length === 1 ? leaders[0].target : null;
}

// swatch colours of the decision bar, shared with its legend
const MODE_SWATCH = {
  blend: "bg-[color:var(--status-success)]",
  exploration: "bg-[color:var(--status-info)]",
  fallback: "bg-[color:var(--text-subtle)]",
} as const;

/** one upstream of a route node, wrapped for the shared table */
type TargetRow = { id: number; target: AdaptiveTargetTelemetryDto };

/** the columns of a node's upstream table; the upstream's name is the row header */
function targetColumns(
  t: TFunction,
  fmt: Formatters,
  preferred: number | null,
): TableColumn<TargetRow>[] {
  const cell = (render: (target: AdaptiveTargetTelemetryDto) => React.ReactNode) => ({
    align: "right" as const,
    mono: true,
    render: (_: unknown, row: TargetRow) => (
      <span className="tabular-nums">{render(row.target)}</span>
    ),
  });
  return [
    {
      key: "upstream",
      header: t("pages.adaptiveDashboard.columns.upstream"),
      rowHeader: true,
      mono: true,
      render: (_, { target }) => (
        <>
          {targetName(t, target)}
          {target.target === preferred && (
            <Badge tone="success" className="ml-2 align-middle">
              {t("pages.adaptiveDashboard.preferred")}
            </Badge>
          )}
        </>
      ),
    },
    {
      key: "score",
      header: (
        <>
          {t("pages.adaptiveDashboard.columns.blendScore")}
          <span className="block font-normal">
            {t("pages.adaptiveDashboard.columns.blendScoreHint")}
          </span>
        </>
      ),
      ...cell((target) => (target.score == null ? "—" : fmt.number(target.score, DECIMAL))),
    },
    {
      key: "latency",
      header: t("pages.adaptiveDashboard.columns.latency"),
      ...cell((target) => formatLatency(t, fmt, target.latency_ms)),
    },
    {
      key: "cost",
      header: t("pages.adaptiveDashboard.columns.cost"),
      ...cell((target) => formatCost(t, fmt, target.cost_per_mtok)),
    },
    {
      key: "inFlight",
      header: t("pages.adaptiveDashboard.columns.inFlight"),
      ...cell((target) => fmt.number(target.in_flight ?? 0)),
    },
    {
      key: "samples",
      header: t("pages.adaptiveDashboard.columns.samples"),
      ...cell((target) => fmt.number(target.samples ?? 0)),
    },
  ];
}

function targetName(t: TFunction, target: AdaptiveTargetTelemetryDto): string {
  const provider =
    target.provider?.trim() ||
    t("pages.adaptiveDashboard.targetFallback", { n: target.target + 1 });
  const model = target.upstream_model?.trim();
  return model ? `${provider} / ${model}` : provider;
}

// the empty state's call to action, shared by both variants so the two never
// drift apart on screen
function AdaptiveDashboardScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const telemetry = useQuery({
    queryKey: ["adaptive-routing-telemetry"],
    queryFn: fetchAdaptiveRoutingTelemetry,
    retry: false,
    refetchInterval: 15_000,
  });
  // read only so the empty state can name the reason it is empty: an operator
  // sent to debug a gateway that is doing nothing wrong is the bug here
  // (#1648). It is never what the screen is waiting on, so a failure leaves
  // the generic copy rather than an error panel over working telemetry
  const policy = useQuery({
    queryKey: ["adaptive-routing-policy"],
    queryFn: fetchAdaptiveRoutingPolicy,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `telemetry` is the query the user is actually waiting on for this screen
  useScreenReady(!telemetry.isLoading);
  // two organizations' same-named routes are named apart (#3460); the hook sits
  // above the early returns, which a hook may not follow
  const owner = useOwnerLabel(
    sharedNames(
      (telemetry.data?.routes ?? []).map((r) => ({ name: r.model, orgId: r.org_id ?? "" })),
    ).size > 0,
  );

  if (telemetry.isLoading) {
    return (
      <PageBody>
        <StatGridSkeleton cards={4} />
        <PanelSkeleton panels={2} height={224} />
      </PageBody>
    );
  }

  if (telemetry.isError) {
    return (
      <PageBody>
        <LoadError
          error={telemetry.error}
          resource={t("errors.resources.adaptiveTelemetry")}
          onRetry={() => void telemetry.refetch()}
          target="adaptive-routes"
        />
      </PageBody>
    );
  }

  const view = telemetry.data;
  const routes = view?.routes ?? [];
  const nodes = new Set(routes.flatMap((route) => route.nodes.map((node) => node.node_id)));
  const observed = routes.reduce(
    (routeTotal, route) =>
      routeTotal + route.nodes.reduce((nodeTotal, node) => nodeTotal + node.observed, 0),
    0,
  );
  const engaged = routes.filter((route) => route.engaged).length;
  const shared = sharedNames(routes.map((r) => ({ name: r.model, orgId: r.org_id ?? "" })));
  // the deployment-wide kill switch, and only when the answer is in: a policy
  // that failed or has not landed must not accuse a switch of being off
  const switchedOff = policy.data?.enabled === false;
  const heldBack = policy.data?.affected_routes ?? [];

  return (
    <PageBody>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <p className="max-w-[65ch] text-sm leading-relaxed text-muted-foreground">
          {t("pages.adaptiveDashboard.lead")}
        </p>
        {view && (
          <p className="text-xs tabular-nums text-muted-foreground">
            <time dateTime={view.generated_at} title={fmt.dateTime(view.generated_at)}>
              {t("pages.adaptiveDashboard.updated", { time: fmt.time(view.generated_at) })}
            </time>{" "}
            {t("pages.adaptiveDashboard.reportsExpire", {
              seconds: fmt.number(view.fresh_window_secs),
            })}
          </p>
        )}
      </div>

      {routes.length === 0 ? (
        switchedOff ? (
          <EmptyState
            thread
            uxTarget="adaptive-routes-disabled"
            icon={<PowerOff aria-hidden="true" />}
            title={t("pages.adaptiveDashboard.disabledTitle")}
            description={
              <>
                {t("pages.adaptiveDashboard.disabledBody")}
                {/* the blast radius, named the way the settings screen names
                    it: these are the routes the switch is holding back */}
                {heldBack.length > 0 && (
                  <span className="mt-2 flex flex-wrap items-center justify-center gap-1.5">
                    <span>
                      {t("pages.adaptiveDashboard.disabledRoutes", {
                        count: heldBack.length,
                      })}
                    </span>
                    {heldBack.map((model) => (
                      <Badge key={model} tone="outline" className="font-mono">
                        {model}
                      </Badge>
                    ))}
                  </span>
                )}
              </>
            }
            actions={
              <EmptyStateLink to="/adaptive-settings">
                {t("pages.adaptiveDashboard.disabledAction")}
              </EmptyStateLink>
            }
          />
        ) : (
          <EmptyState
            thread
            uxTarget="adaptive-routes"
            icon={<Activity aria-hidden="true" />}
            title={t("pages.adaptiveDashboard.emptyTitle")}
            description={t("pages.adaptiveDashboard.emptyBody", {
              seconds: view?.fresh_window_secs ?? 60,
            })}
            actions={
              <EmptyStateLink to="/routing-rules">
                {t("pages.adaptiveDashboard.emptyAction")}
              </EmptyStateLink>
            }
          />
        )
      ) : (
        <>
          <div className={STAT_GRID}>
            <StatCard
              label={t("pages.adaptiveDashboard.stats.adaptiveRoutes")}
              value={fmt.number(routes.length)}
            />
            <StatCard
              label={t("pages.adaptiveDashboard.stats.blendActive")}
              value={fmt.number(engaged)}
            />
            <StatCard
              label={t("pages.adaptiveDashboard.stats.reportingGateways")}
              value={fmt.number(nodes.size)}
            />
            <StatCard
              label={t("pages.adaptiveDashboard.stats.observedPicks")}
              value={fmt.number(observed)}
            />
          </div>

          <div className="flex flex-col gap-4">
            {routes.map((route) => (
              <section
                // two organizations may each have a route of one name (#3458)
                key={`${route.org_id ?? ""}/${route.model}`}
                data-org={route.org_id ?? ""}
                aria-label={
                  shared.has(route.model)
                    ? t("pages.adaptiveDashboard.routeAriaOwned", {
                        model: route.model,
                        owner: owner(route.org_id ?? ""),
                      })
                    : t("pages.adaptiveDashboard.routeAria", { model: route.model })
                }
                className="overflow-hidden rounded-lg border border-[color:var(--border-subtle)]"
              >
                <div className="flex flex-wrap items-center justify-between gap-3 bg-[color:var(--surface-subtle)] px-4 py-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <h2 className="break-words font-mono text-sm font-medium text-foreground">
                        {route.model}
                      </h2>
                      {shared.has(route.model) && <OwnerOrgPill orgId={route.org_id ?? ""} />}
                    </div>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {t("pages.adaptiveDashboard.reportingGateways", {
                        count: route.nodes.length,
                      })}
                    </p>
                  </div>
                  <Badge
                    tone={
                      route.engaged ? "success" : routeIsDisabled(route) ? "outline" : "warning"
                    }
                    dot={!routeIsDisabled(route)}
                  >
                    {routeState(t, route)}
                  </Badge>
                </div>

                <div className="flex flex-col divide-y divide-[color:var(--border-subtle)]">
                  {route.nodes.map((node, nodeIndex) => {
                    const decisionTotal = totalDecisions(node.decisions);
                    return (
                      <details key={node.node_id} open={nodeIndex === 0} className="group">
                        <summary className="flex cursor-pointer list-none flex-wrap items-center justify-between gap-3 px-4 py-3 transition-colors hover:bg-[color:var(--surface-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
                          <span className="flex min-w-0 items-center gap-2">
                            <ChevronRight
                              className="h-4 w-4 flex-none text-muted-foreground transition-transform group-open:rotate-90 motion-reduce:transition-none"
                              aria-hidden="true"
                            />
                            <Network
                              className="h-4 w-4 flex-none text-muted-foreground"
                              aria-hidden="true"
                            />
                            <span className="break-all font-mono text-sm font-medium">
                              {node.node_id}
                            </span>
                          </span>
                          <span className="flex flex-wrap items-center justify-end gap-2">
                            <Badge tone={node.engaged ? "success" : "outline"}>
                              {nodeState(t, node)}
                            </Badge>
                            <span className="text-xs tabular-nums text-muted-foreground">
                              {t("pages.adaptiveDashboard.decisions", {
                                count: decisionTotal,
                                value: fmt.number(decisionTotal),
                              })}
                            </span>
                          </span>
                        </summary>

                        <div className="flex flex-col gap-4 border-t border-[color:var(--border-subtle)] p-4">
                          <div className="flex flex-wrap items-start justify-between gap-3">
                            <p className="max-w-[65ch] text-xs leading-relaxed text-muted-foreground">
                              {policySummary(t, fmt, node)}
                            </p>
                            <time
                              dateTime={node.reported_at}
                              title={fmt.dateTime(node.reported_at)}
                              className="text-xs tabular-nums text-muted-foreground"
                            >
                              {t("pages.adaptiveDashboard.reported", {
                                time: fmt.time(node.reported_at),
                              })}
                            </time>
                          </div>

                          <div
                            role="group"
                            aria-label={t("pages.adaptiveDashboard.decisionModesAria", {
                              node: node.node_id,
                            })}
                          >
                            <div
                              className="flex h-2 overflow-hidden rounded-full bg-[color:var(--surface-subtle)]"
                              aria-hidden="true"
                            >
                              <span
                                className={MODE_SWATCH.blend}
                                style={{ width: barWidth(node.decisions.blend, decisionTotal) }}
                              />
                              <span
                                className={MODE_SWATCH.exploration}
                                style={{
                                  width: barWidth(node.decisions.exploration, decisionTotal),
                                }}
                              />
                              <span
                                className={MODE_SWATCH.fallback}
                                style={{ width: barWidth(node.decisions.fallback, decisionTotal) }}
                              />
                            </div>
                            {/* the key to the bar: swatch, name and count sit together so
                                a value is never read against its neighbour's label */}
                            <dl className="mt-2 flex flex-wrap gap-x-6 gap-y-1.5 text-xs">
                              {(["blend", "exploration", "fallback"] as const).map((mode) => (
                                <div key={mode} className="flex items-center gap-2">
                                  <span
                                    aria-hidden="true"
                                    className={`h-2.5 w-2.5 flex-none rounded-full ${MODE_SWATCH[mode]}`}
                                  />
                                  <dt className="text-muted-foreground">
                                    {t(`pages.adaptiveDashboard.modes.${mode}`)}
                                  </dt>
                                  <dd className="font-mono tabular-nums text-foreground">
                                    {fmt.number(node.decisions[mode])} ·{" "}
                                    {share(fmt, node.decisions[mode], decisionTotal)}
                                  </dd>
                                </div>
                              ))}
                            </dl>
                          </div>

                          {node.targets.length === 0 ? (
                            <p className="text-sm text-muted-foreground">
                              {t("pages.adaptiveDashboard.noTargets")}
                            </p>
                          ) : (
                            <Table
                              caption={t("pages.adaptiveDashboard.tableCaption", {
                                node: node.node_id,
                              })}
                              rowKey="id"
                              hover={false}
                              columns={targetColumns(t, fmt, preferredTarget(node))}
                              data={node.targets.map((target) => ({ id: target.target, target }))}
                            />
                          )}
                        </div>
                      </details>
                    );
                  })}
                </div>
              </section>
            ))}
          </div>
        </>
      )}
    </PageBody>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(
  AdaptiveDashboardScreen,
  "errors.resources.adaptiveTelemetry",
  "adaptive_routing_telemetry:read",
);
