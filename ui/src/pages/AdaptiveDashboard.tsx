import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Activity, Network } from "lucide-react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton, StatGridSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { EmptyState } from "@/components/ui/empty-state";
import { StatCard } from "@/components/ui/stat-card";
import {
  fetchAdaptiveRoutingTelemetry,
  type AdaptiveDecisionCountsDto,
  type AdaptiveNodeTelemetryDto,
  type AdaptiveRouteTelemetryDto,
  type AdaptiveTargetTelemetryDto,
} from "@/lib/api";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

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

function formatCost(t: TFunction, fmt: Formatters, value?: number): string {
  if (!value || value <= 0) return t("pages.adaptiveDashboard.unknownCost");
  return fmt.number(value, DECIMAL);
}

function policySummary(
  t: TFunction,
  fmt: Formatters,
  node: AdaptiveNodeTelemetryDto,
): string {
  const policy = node.policy;
  const weight = (key: string, value: number) =>
    t(key, { value: fmt.number(value, DECIMAL) });
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
    weights.length > 0
      ? weights.join(" / ")
      : t("pages.adaptiveDashboard.policy.unavailable");
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

function nodeState(node: AdaptiveNodeTelemetryDto): "DISABLED" | "ROUTING" | "WARMING" {
  if (node.policy.enabled === false) return "DISABLED";
  return node.engaged ? "ROUTING" : "WARMING";
}

function targetName(target: AdaptiveTargetTelemetryDto): string {
  const provider = target.provider?.trim() || `target ${target.target + 1}`;
  const model = target.upstream_model?.trim();
  return model ? `${provider} / ${model}` : provider;
}

function AdaptiveDashboardScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const telemetry = useQuery({
    queryKey: ["adaptive-routing-telemetry"],
    queryFn: fetchAdaptiveRoutingTelemetry,
    retry: false,
    refetchInterval: 15_000,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `telemetry` is the query the user is actually waiting on for this screen
  useScreenReady(!telemetry.isLoading);
  useErrorState(!!telemetry.error, "adaptive-dashboard");

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
            </time>
            {" "}
            {t("pages.adaptiveDashboard.reportsExpire", {
              seconds: fmt.number(view.fresh_window_secs),
            })}
          </p>
        )}
      </div>

      {routes.length === 0 ? (
        <EmptyState
          uxTarget="adaptive-routes"
          icon={<Activity aria-hidden="true" />}
          title={t("pages.adaptiveDashboard.emptyTitle")}
          description={t("pages.adaptiveDashboard.emptyBody", {
            seconds: view?.fresh_window_secs ?? 60,
          })}
          actions={
            <a
              href="/routing-rules"
              className="text-sm font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {t("pages.adaptiveDashboard.emptyAction")}
            </a>
          }
        />
      ) : (
        <>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <StatCard label={t("pages.adaptiveDashboard.stats.adaptiveRoutes")} value={fmt.number(routes.length)} />
            <StatCard label={t("pages.adaptiveDashboard.stats.blendActive")} value={fmt.number(engaged)} />
            <StatCard label={t("pages.adaptiveDashboard.stats.reportingGateways")} value={fmt.number(nodes.size)} />
            <StatCard label={t("pages.adaptiveDashboard.stats.observedPicks")} value={fmt.number(observed)} />
          </div>

          <div className="flex flex-col gap-4">
            {routes.map((route) => (
              <section
                key={route.model}
                aria-label={t("pages.adaptiveDashboard.routeAria", { model: route.model })}
                className="overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]"
              >
                <div className="flex flex-wrap items-center justify-between gap-3 bg-[color:var(--surface-subtle)] px-4 py-3">
                  <div className="min-w-0">
                    <h2 className="break-words font-mono text-sm font-medium text-foreground">
                      {route.model}
                    </h2>
                    <p className="mt-1 text-xs text-muted-foreground">
                      {t("pages.adaptiveDashboard.reportingGateways", { count: route.nodes.length })}
                    </p>
                  </div>
                  <Badge
                    tone={route.engaged ? "success" : routeIsDisabled(route) ? "outline" : "warning"}
                    dot={!routeIsDisabled(route)}
                  >
                    {route.engaged
                      ? "BLEND ACTIVE"
                      : routeIsDisabled(route)
                        ? "DISABLED"
                        : "FALLBACK / WARMING"}
                  </Badge>
                </div>

                <div className="flex flex-col gap-3 p-3">
                  {route.nodes.map((node, nodeIndex) => {
                    const decisionTotal = totalDecisions(node.decisions);
                    return (
                      <details
                        key={node.node_id}
                        open={nodeIndex === 0}
                        className="group rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-card)]"
                      >
                        <summary className="flex cursor-pointer list-none flex-wrap items-center justify-between gap-3 rounded-lg px-4 py-3 transition-colors hover:bg-[color:var(--surface-hover)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
                          <span className="flex min-w-0 items-center gap-2">
                            <Network className="h-4 w-4 flex-none text-muted-foreground" aria-hidden="true" />
                            <span className="break-all font-mono text-sm font-medium">
                              {node.node_id}
                            </span>
                          </span>
                          <span className="flex flex-wrap items-center justify-end gap-2">
                            <Badge tone={node.engaged ? "success" : "outline"}>
                              {nodeState(node)}
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
                                className="bg-[color:var(--status-success)]"
                                style={{ width: barWidth(node.decisions.blend, decisionTotal) }}
                              />
                              <span
                                className="bg-[color:var(--status-info)]"
                                style={{ width: barWidth(node.decisions.exploration, decisionTotal) }}
                              />
                              <span
                                className="bg-[color:var(--text-subtle)]"
                                style={{ width: barWidth(node.decisions.fallback, decisionTotal) }}
                              />
                            </div>
                            <dl className="mt-2 grid grid-cols-1 gap-2 text-xs sm:grid-cols-3">
                              {([
                                [t("pages.adaptiveDashboard.modes.blend"), node.decisions.blend],
                                [
                                  t("pages.adaptiveDashboard.modes.exploration"),
                                  node.decisions.exploration,
                                ],
                                [t("pages.adaptiveDashboard.modes.fallback"), node.decisions.fallback],
                              ] as const).map(([label, value]) => (
                                <div key={label} className="flex items-baseline justify-between gap-2">
                                  <dt className="text-muted-foreground">{label}</dt>
                                  <dd className="font-mono tabular-nums text-foreground">
                                    {fmt.number(value)} · {share(fmt, value, decisionTotal)}
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
                            <div
                              tabIndex={0}
                              className="overflow-x-auto rounded-lg border border-[color:var(--border-subtle)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            >
                              <table className="w-full min-w-[760px] border-collapse text-sm">
                                <caption className="sr-only">
                                  {t("pages.adaptiveDashboard.tableCaption", {
                                    node: node.node_id,
                                  })}
                                </caption>
                                <thead className="bg-[color:var(--surface-subtle)] text-left text-[0.6875rem] uppercase tracking-[0.07em] text-muted-foreground">
                                  <tr>
                                    <th scope="col" className="px-3 py-2 font-medium">
                                      {t("pages.adaptiveDashboard.columns.upstream")}
                                    </th>
                                    <th scope="col" className="px-3 py-2 text-right font-medium">
                                      {t("pages.adaptiveDashboard.columns.blendScore")}
                                    </th>
                                    <th scope="col" className="px-3 py-2 text-right font-medium">
                                      {t("pages.adaptiveDashboard.columns.latency")}
                                    </th>
                                    <th scope="col" className="px-3 py-2 text-right font-medium">
                                      {t("pages.adaptiveDashboard.columns.cost")}
                                    </th>
                                    <th scope="col" className="px-3 py-2 text-right font-medium">
                                      {t("pages.adaptiveDashboard.columns.inFlight")}
                                    </th>
                                    <th scope="col" className="px-3 py-2 text-right font-medium">
                                      {t("pages.adaptiveDashboard.columns.samples")}
                                    </th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {node.targets.map((target) => (
                                    <tr
                                      key={target.target}
                                      className="border-t border-[color:var(--border-subtle)]"
                                    >
                                      <th scope="row" className="break-words px-3 py-2.5 text-left font-mono text-xs font-medium">
                                        {targetName(target)}
                                      </th>
                                      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums">
                                        {target.score == null ? "—" : fmt.number(target.score, DECIMAL)}
                                      </td>
                                      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums">
                                        {formatLatency(t, fmt, target.latency_ms)}
                                      </td>
                                      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums">
                                        {formatCost(t, fmt, target.cost_per_mtok)}
                                      </td>
                                      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums">
                                        {fmt.number(target.in_flight ?? 0)}
                                      </td>
                                      <td className="px-3 py-2.5 text-right font-mono text-xs tabular-nums">
                                        {fmt.number(target.samples ?? 0)}
                                      </td>
                                    </tr>
                                  ))}
                                </tbody>
                              </table>
                            </div>
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
export default superadminOnly(AdaptiveDashboardScreen, "errors.resources.adaptiveTelemetry");
