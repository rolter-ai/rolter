import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { PageBody } from "@/components/screen";
import { GettingStarted } from "@/components/GettingStarted";
import { IncompleteSpendNotice } from "@/components/IncompleteSpendNotice";
import { LoadError } from "@/components/LoadError";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Donut } from "@/components/ui/donut";
import { LineChart } from "@/components/ui/line-chart";
import { Skeleton } from "@/components/ui/skeleton";
import { ListSkeleton, LoadingRegion, StatGridSkeleton } from "@/components/LoadingState";
import { StatCard } from "@/components/ui/stat-card";
import { Table } from "@/components/ui/table";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsByModel,
  fetchAnalyticsSummary,
  fetchAnalyticsTimeseries,
  fetchInvocations,
  type InvocationRow,
} from "@/lib/api";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { isAwaiting } from "@/lib/read-state";
import { windowBounds, type TimeWindow } from "@/lib/time-window";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const num = (v: number | string | undefined): number => Number(v ?? 0);

// the screen reads one window, by name. its bounds are worked out as each
// request leaves rather than once when the module loads, so a tab left open
// reads the last 24 hours as they are now and not every hour since it was
// opened (#1975). the name is what the query keys carry
const WINDOW_NAME: TimeWindow = "24h";
const readWindow = () => ({ ...windowBounds(WINDOW_NAME), bucket: "hour" });

// the recent requests card says "live", so it asks again every 15s, the pace
// of the other polled screens (Cluster, Adaptive Routing). react-query holds
// an interval while the tab is hidden and asks again when it comes back. the
// figures and charts are aggregates over a day: a minute keeps them, and the
// window they are read over, from going stale on a tab left open
const RECENT_POLL_MS = 15_000;
const OVERVIEW_POLL_MS = 60_000;

// a query that has never held data goes back to pending on every refetch, which
// unmounts its error, so polling one that failed would swap the alert for a
// skeleton and back each cycle. it waits for the retry instead (#1984). a
// failure with data on screen keeps that data and is retried by the next poll
const pollEvery =
  (ms: number) =>
  (query: { state: { status: string; data: unknown } }): number | false =>
    query.state.status === "error" && query.state.data === undefined ? false : ms;

// the shared categorical sequence (#1245), not a fifth hand-written list
const BAR_PALETTE = ["var(--chart-1)", "var(--chart-2)", "var(--chart-3)", "var(--chart-4)"];

function isUnavailable(err: unknown): boolean {
  return err instanceof AnalyticsUnavailableError;
}

// a read that failed holding nothing: the state a card answers with its own
// error. it is not the no-analytics deployment, which is one calm panel for the
// whole screen and no error at all
const failedEmpty = (q: UseQueryResult<unknown>) =>
  q.isError && q.data === undefined && !isUnavailable(q.error);

// what a card shows for its own read (#1976): a skeleton while the read is
// awaited, its own error with a retry for that read alone when it failed holding
// nothing, and `children` once it holds data. the render prop is handed data
// only then, so a card cannot say "no traffic yet" about a read that has not
// answered or that failed. data a failed refresh left behind still counts: the
// card keeps showing it, and `RefreshFailed` says it may be stale
function CardRead<T>({
  read,
  resource,
  skeleton,
  children,
}: {
  read: UseQueryResult<T>;
  resource: string;
  skeleton: React.ReactNode;
  children: (data: T) => React.ReactNode;
}) {
  if (isAwaiting(read)) return <>{skeleton}</>;
  const data = read.data;
  if (data === undefined) {
    return read.isError ? (
      <LoadError error={read.error} resource={resource} onRetry={() => void read.refetch()} />
    ) : null;
  }
  return <>{children(data)}</>;
}

// a background refresh that failed over figures already on screen. they stay,
// since blanking a dashboard that had loaded on every blip would tell the
// reader less than the stale numbers do, and this line says they may be old.
// plain text rather than a live region: it is rewritten by every failed poll,
// and five of them announcing once a minute would be noise
function RefreshFailed({
  read,
}: {
  read: Pick<UseQueryResult<unknown>, "isError" | "data" | "errorUpdatedAt">;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (!read.isError || read.data === undefined) return null;
  return (
    <p className="mt-2 text-xs text-[color:var(--status-danger-text)]">
      {t("pages.dashboard.feed.refreshFailedRetrying", { time: fmt.time(read.errorUpdatedAt) })}
    </p>
  );
}

// the by-model bars while they are coming: rows the height of the bars
function BarsSkeleton() {
  return (
    <LoadingRegion className="flex flex-col gap-2">
      {Array.from({ length: 4 }, (_, i) => (
        <Skeleton key={i} height={16} radius={3} />
      ))}
    </LoadingRegion>
  );
}

// live overview: 4 KPIs, hourly spend line, traffic donut, requests-by-model
// bars, and a recent-requests mini table. all clickhouse-backed. each card
// reads its own query and owns its loading, empty and error state, so one slow
// or failing endpoint leaves the rest of the screen up (#1976). a deployment
// with analytics off gets one calm panel instead of the cards.
// `pollMs` is only ever set by a story, which swaps both intervals for one a
// play can watch several of inside the test-runner's per-story budget
export default function Dashboard({ pollMs }: { pollMs?: number }) {
  const { t } = useTranslation();
  // formatting follows the dashboard language, not the browser locale
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const money = (n: number) => fmt.currency(n, currency);
  const overviewPoll = pollEvery(pollMs ?? OVERVIEW_POLL_MS);
  const summary = useQuery({
    queryKey: ["analytics", "summary", WINDOW_NAME],
    queryFn: () => fetchAnalyticsSummary(readWindow()),
    refetchInterval: overviewPoll,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `summary` is the query the user is actually waiting on for this screen
  useScreenReady(!summary.isLoading);
  const series = useQuery({
    queryKey: ["analytics", "timeseries", WINDOW_NAME],
    queryFn: () => fetchAnalyticsTimeseries(readWindow()),
    refetchInterval: overviewPoll,
    retry: false,
  });
  const byModel = useQuery({
    queryKey: ["analytics", "by-model", WINDOW_NAME],
    queryFn: () => fetchAnalyticsByModel(readWindow()),
    refetchInterval: overviewPoll,
    retry: false,
  });
  const recent = useQuery({
    queryKey: ["invocations", "recent"],
    queryFn: () => fetchInvocations({ ...readWindow(), limit: 8 }),
    refetchInterval: pollEvery(pollMs ?? RECENT_POLL_MS),
    retry: false,
  });
  // one signal per error placeholder, named for the card that shows it
  useErrorState(failedEmpty(summary), "dashboard");
  useErrorState(failedEmpty(series), "dashboard-spend");
  useErrorState(failedEmpty(byModel), "dashboard-traffic");
  useErrorState(failedEmpty(recent), "dashboard-recent");

  // a deployment with no analytics store answers every panel on this screen the
  // same way. It used to render as an empty state, which says "nothing happened
  // yet" about a control plane that was never asked to record anything (#1236),
  // and then as the red alert a 500 gets (#1976). it is a deployment shape, so
  // it is one panel for the screen, not one per card
  const unavailable =
    [summary, series, byModel, recent].find((q) => q.data === undefined && isUnavailable(q.error))
      ?.error ?? null;

  if (unavailable) {
    return (
      <PageBody>
        {/* a deployment with no analytics store still has a first run, and the
            checklist below reads rows rather than traffic (#1585) */}
        <GettingStarted />
        <AnalyticsUnavailable error={unavailable} i18nKey="pages.dashboard.noAnalytics" />
      </PageBody>
    );
  }

  const fmtK = fmt.compact;

  // "live" is said only while the last read of this card succeeded (#1984). a
  // first load that failed has stopped polling, and a failure with rows on
  // screen is retried by the next poll, so only the second one says retrying
  const recentFailedAt = fmt.time(recent.errorUpdatedAt);
  const recentFeed = !recent.isError
    ? t("pages.dashboard.live")
    : recent.data === undefined
      ? t("pages.dashboard.feed.loadFailed", { time: recentFailedAt })
      : t("pages.dashboard.feed.refreshFailedRetrying", { time: recentFailedAt });

  return (
    <PageBody className="gap-[18px]">
      <GettingStarted requests={summary.isSuccess ? num(summary.data?.requests) : undefined} />
      <div data-testid="dashboard-figures">
        <CardRead
          read={summary}
          resource={t("errors.resources.dashboardFigures")}
          // `Skeleton` is `aria-hidden`, so the four bare ones this used to
          // render were a loading state no screen reader could hear (#1605)
          skeleton={<StatGridSkeleton cards={4} />}
        >
          {(s) => {
            const requests = num(s?.requests);
            const errors = num(s?.errors);
            // an average and a rate over no requests are undefined, not zero:
            // "0 ms" and "0.00 %" read as a measurement of a quiet deployment
            const measured = requests > 0;
            const errorRate = measured ? (errors / requests) * 100 : undefined;
            const unmeasured = t("pages.dashboard.notMeasured");
            const why = t("pages.dashboard.noRequestsInWindow");
            return (
              <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2 xl:grid-cols-4">
                <StatCard label={t("pages.dashboard.statRequests")} value={fmt.number(requests)} />
                <StatCard label={t("pages.dashboard.statSpend")} value={money(num(s?.cost_usd))} />
                <StatCard
                  label={t("pages.dashboard.statAvgLatency")}
                  value={measured ? fmt.number(Math.round(num(s?.avg_latency_ms))) : unmeasured}
                  unit={measured ? t("pages.dashboard.colMs") : undefined}
                  delta={measured ? undefined : why}
                />
                <StatCard
                  label={t("pages.dashboard.statErrorRate")}
                  value={
                    errorRate === undefined
                      ? unmeasured
                      : fmt.number(errorRate, {
                          minimumFractionDigits: 2,
                          maximumFractionDigits: 2,
                        })
                  }
                  unit={errorRate === undefined ? undefined : "%"}
                  // no `trend`: the summary is one window with nothing earlier to
                  // compare against, so an arrow would claim a movement nobody
                  // measured. above 1% of requests the count reads as the problem it
                  // is rather than as growth (#1974)
                  tone={errorRate !== undefined && errorRate > 1 ? "bad" : "neutral"}
                  // russian needs four plural forms here where english needs two
                  delta={
                    errorRate === undefined
                      ? why
                      : errors > 0
                        ? t("pages.dashboard.errors", { count: errors })
                        : undefined
                  }
                />
              </div>
            );
          }}
        </CardRead>
        <RefreshFailed read={summary} />
      </div>

      <IncompleteSpendNotice
        requests={num(summary.data?.unpriced_requests)}
        models={num(summary.data?.unpriced_models)}
      />

      <div className="grid gap-3.5 xl:grid-cols-[1.6fr_1fr]">
        <Card data-testid="dashboard-spend">
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.last24h")}
            </CardDescription>
            <CardTitle className="text-base">{t("pages.dashboard.spendTitle")}</CardTitle>
            {/* the amounts follow the deployment's currency, so the subtitle
                names it rather than claiming dollars (#1182) */}
            <CardDescription>{t("pages.dashboard.spendSub", { currency })}</CardDescription>
          </CardHeader>
          <CardContent>
            <CardRead
              read={series}
              resource={t("errors.resources.dashboardSpend")}
              skeleton={
                <LoadingRegion>
                  <Skeleton height={220} />
                </LoadingRegion>
              }
            >
              {(points) => {
                const spendPoints = points.map((p) => num(p.cost_usd));
                const spendLabels = points.map((p) => fmt.timeShort(p.bucket) || p.bucket);
                // buckets exist but every one of them is zero: the window had
                // traffic that was never priced, which is not the same as spend
                // that happened to be zero
                const anySpend = spendPoints.some((v) => v > 0);
                return spendPoints.length === 0 ? (
                  <p className="py-16 text-center text-sm text-muted-foreground">
                    {t("analytics.noRowsYet")}
                  </p>
                ) : !anySpend ? (
                  // requests were served but nothing was priced. drawing a flat
                  // line along zero here says "spend is zero", which is a
                  // different fact from "no spend was recorded" — and on a cost
                  // dashboard that difference is the whole point (#960)
                  <p className="py-16 text-center text-sm text-muted-foreground">
                    {t("pages.dashboard.noSpendRecorded")}
                  </p>
                ) : (
                  <LineChart
                    series={[{ name: "spend", values: spendPoints }]}
                    labels={spendLabels}
                    height={220}
                    label={t("pages.dashboard.spendChartAria")}
                    formatValue={(v) => money(v)}
                    emptyState={
                      <p className="text-sm text-muted-foreground">{t("analytics.noRowsYet")}</p>
                    }
                  />
                );
              }}
            </CardRead>
            <RefreshFailed read={series} />
          </CardContent>
        </Card>
        <Card data-testid="dashboard-traffic">
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.last24h")}
            </CardDescription>
            <CardTitle className="text-base">{t("pages.dashboard.trafficTitle")}</CardTitle>
            <CardDescription>{t("pages.dashboard.trafficSub")}</CardDescription>
          </CardHeader>
          <CardContent>
            <CardRead
              read={byModel}
              resource={t("errors.resources.dashboardTrafficShare")}
              skeleton={
                <LoadingRegion>
                  <Skeleton height={180} />
                </LoadingRegion>
              }
            >
              {(models) => {
                const traffic = models.map((m) => ({ label: m.model, value: num(m.requests) }));
                return traffic.length === 0 ? (
                  <p className="py-16 text-center text-sm text-muted-foreground">
                    {t("pages.dashboard.noTraffic")}
                  </p>
                ) : (
                  <Donut
                    segments={traffic}
                    size={150}
                    centerLabel={fmtK(traffic.reduce((a, m) => a + m.value, 0))}
                    centerSub={t("pages.dashboard.requests")}
                  />
                );
              }}
            </CardRead>
            <RefreshFailed read={byModel} />
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-3.5 xl:grid-cols-2">
        <Card data-testid="dashboard-by-model">
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.last24h")}
            </CardDescription>
            <CardTitle className="text-base">{t("pages.dashboard.byModelTitle")}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2">
            <CardRead
              read={byModel}
              resource={t("errors.resources.dashboardByModel")}
              skeleton={<BarsSkeleton />}
            >
              {(models) => {
                const barMax = Math.max(1, ...models.map((m) => num(m.requests)));
                const bars = [...models]
                  .sort((a, b) => num(b.requests) - num(a.requests))
                  .slice(0, 6)
                  .map((m, i) => ({
                    label: m.model,
                    value: num(m.requests),
                    pct: (num(m.requests) / barMax) * 100,
                    color: BAR_PALETTE[i] ?? "var(--chart-5)",
                  }));
                return bars.length === 0 ? (
                  <p className="py-10 text-center text-sm text-muted-foreground">
                    {t("pages.dashboard.noTraffic")}
                  </p>
                ) : (
                  bars.map((b) => (
                    <div key={b.label} className="flex items-center gap-2.5">
                      <span className="w-[110px] flex-none truncate text-right font-mono text-xs text-muted-foreground">
                        {b.label}
                      </span>
                      <div className="h-4 flex-1 overflow-hidden rounded-[3px] bg-[color:var(--surface-subtle)]">
                        <div
                          className="h-full rounded-[3px]"
                          style={{ width: `${b.pct}%`, background: b.color }}
                        />
                      </div>
                      <span className="w-[52px] flex-none font-mono text-xs text-[color:var(--text-secondary)]">
                        {fmtK(b.value)}
                      </span>
                    </div>
                  ))
                );
              }}
            </CardRead>
            <RefreshFailed read={byModel} />
          </CardContent>
        </Card>
        <Card data-testid="dashboard-recent">
          <CardHeader>
            <CardDescription
              className={cn(
                "text-[0.6875rem] uppercase tracking-[0.07em]",
                recent.isError && "text-[color:var(--status-danger-text)]",
              )}
            >
              {recentFeed}
            </CardDescription>
            <CardTitle className="text-base">{t("pages.dashboard.recentTitle")}</CardTitle>
          </CardHeader>
          <CardContent>
            <CardRead
              read={recent}
              resource={t("errors.resources.dashboardRecent")}
              skeleton={<ListSkeleton rows={4} />}
            >
              {(rows) =>
                rows.length === 0 ? (
                  <p className="py-10 text-center text-sm text-muted-foreground">
                    {t("pages.dashboard.nothingLogged")}
                  </p>
                ) : (
                  <Table
                    rowKey="id"
                    columns={[
                      {
                        key: "t",
                        header: t("pages.dashboard.colTime"),
                        mono: true,
                        width: "92px",
                      },
                      {
                        key: "model",
                        header: t("pages.dashboard.colModel"),
                        mono: true,
                      },
                      {
                        key: "status",
                        header: t("pages.dashboard.colStatus"),
                        render: (v) => <StatusBadge status={v as number} />,
                      },
                      {
                        key: "lat",
                        header: t("pages.dashboard.colMs"),
                        align: "right",
                        mono: true,
                      },
                    ]}
                    data={
                      rows.map((r: InvocationRow) => ({
                        id: r.request_id || r.ts,
                        t: fmt.time(r.ts),
                        model: r.model,
                        status: num(r.status),
                        lat: Math.round(num(r.latency_ms)),
                      })) as unknown as Record<string, unknown>[]
                    }
                  />
                )
              }
            </CardRead>
          </CardContent>
        </Card>
      </div>
    </PageBody>
  );
}

function StatusBadge({ status }: { status: number }) {
  const tone =
    status < 400
      ? ["var(--status-success-text)", "rgba(22,163,74,.14)"]
      : status === 429
        ? ["var(--status-warning-text)", "rgba(245,158,11,.14)"]
        : ["var(--status-danger-text)", "rgba(229,57,53,.14)"];
  return (
    <span
      className="inline-flex items-center rounded-[6px] px-[7px] py-0.5 font-mono text-[11px] font-semibold"
      style={{ color: tone[0], background: tone[1] }}
    >
      {status}
    </span>
  );
}
