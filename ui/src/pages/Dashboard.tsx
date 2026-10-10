import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { PageBody } from "@/components/screen";
import { GettingStarted } from "@/components/GettingStarted";
import { IncompleteSpendNotice } from "@/components/IncompleteSpendNotice";
import { LoadError } from "@/components/LoadError";
import { SavedViews } from "@/components/SavedViews";
import { Combobox } from "@/components/ui/combobox";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Donut } from "@/components/ui/donut";
import { LineChart } from "@/components/ui/line-chart";
import { Skeleton } from "@/components/ui/skeleton";
import { ListSkeleton, LoadingRegion, StatGridSkeleton } from "@/components/LoadingState";
import { STAT_GRID, StatCard } from "@/components/ui/stat-card";
import { Table } from "@/components/ui/table";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsByModel,
  fetchAnalyticsSummary,
  fetchAnalyticsTimeseries,
  fetchInvocations,
  type AnalyticsByModelRow,
  type InvocationRow,
  type SavedViewFilters,
} from "@/lib/api";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { modelColor, rankedByRequests } from "@/lib/model-colors";
import { isAwaiting } from "@/lib/read-state";
import {
  DEFAULT_TIME_WINDOW,
  readTimeWindow,
  useAddressWindow,
  useTimeWindowOptions,
  windowBounds,
  type TimeWindow,
} from "@/lib/time-window";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const num = (v: number | string | undefined): number => Number(v ?? 0);

// the figures and charts read one window, by name: the last 24 hours unless the
// address names another (`?window=7d`). its bounds are worked out as each
// request leaves rather than once when the module loads, so a tab left open
// reads the window as it is now and not as it was when it was opened (#1975).
// the name is what the query keys carry
const readWindow = (name: TimeWindow) => ({ ...windowBounds(name), bucket: "hour" });
// "recent" is the latest requests, whatever window the figures are read over
const readRecent = () => windowBounds(DEFAULT_TIME_WINDOW);

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

// the bars show this many models. the donut keeps one more slice than that
// before it rolls the rest into "Other", so the models on the bars are always
// the donut's first slices, in the same colours
const BARS_SHOWN = 6;

// the models in the order every card colours them by (#1994): the read arrives
// by cost, and the bars used to re-sort it by requests while the donut kept it
function byRequests(models: AnalyticsByModelRow[]) {
  return rankedByRequests(models.map((m) => ({ model: m.model, requests: num(m.requests) })));
}

// the recent requests sit in a card that already has the frame, so the table
// drops its own, which drew a second border 24px inside the first. a phone has
// less room than four columns need: at 16px padding and with the model wrapping
// onto several lines, the `ms` column scrolled out of reach. so below `sm` the
// cells give up 8px of padding each, and the model column (the second) takes
// whatever the other three leave and truncates in it, its full name on hover,
// rather than wrapping. `relative` on the row is what the row link stretches to
// cover
const RECENT_TABLE = cn(
  "rounded-md border-0 [&_tbody_tr]:relative",
  "[&_th]:px-2 [&_td]:px-2 sm:[&_th]:px-4 sm:[&_td]:px-4",
  "[&_th:nth-child(2)]:w-full [&_td:nth-child(2)]:max-w-0 [&_td:nth-child(2)]:truncate",
);

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
// card keeps showing it, and `RefreshFailed` says it may be stale. a card that
// reads what another card reads points at that card's alert instead of
// repeating it: `failed` replaces its `LoadError` (#2342)
function CardRead<T>({
  read,
  resource,
  target,
  skeleton,
  failed,
  children,
}: {
  read: UseQueryResult<T>;
  resource: string;
  /** the card's region on the `error_state` UX event its `LoadError` records */
  target: string;
  skeleton: React.ReactNode;
  failed?: React.ReactNode;
  children: (data: T) => React.ReactNode;
}) {
  if (isAwaiting(read)) return <>{skeleton}</>;
  const data = read.data;
  if (data === undefined) {
    if (!read.isError) return null;
    return (
      failed ?? (
        <LoadError
          error={read.error}
          resource={resource}
          onRetry={() => void read.refetch()}
          target={target}
        />
      )
    );
  }
  return <>{children(data)}</>;
}

// a background refresh that failed over figures already on screen. they stay,
// since blanking a dashboard that had loaded on every blip would tell the
// reader less than the stale numbers do, and this line says they may be old.
// plain text rather than a live region: it is rewritten by every failed poll,
// and five of them announcing once a minute would be noise. it records an
// `error_state` under `target`, once each time it appears (#2640): a LoadError
// is not on screen, so nothing else would say this data is going stale
function RefreshFailed({
  read,
  target,
}: {
  read: Pick<UseQueryResult<unknown>, "isError" | "data" | "errorUpdatedAt">;
  /** the card's region on the UX event, suffixed `-stale` to tell it from a `LoadError` */
  target: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const stale = read.isError && read.data !== undefined;
  useErrorState(stale, target);
  if (!stale) return null;
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
// play can watch several of inside the story tests' per-story budget
export default function Dashboard({ pollMs }: { pollMs?: number }) {
  const { t } = useTranslation();
  // formatting follows the dashboard language, not the browser locale
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const money = (n: number) => fmt.currency(n, currency);
  const overviewPoll = pollEvery(pollMs ?? OVERVIEW_POLL_MS);
  const [timeWindow, setWindow] = useAddressWindow();
  const windowOptions = useTimeWindowOptions();
  const windowLabel = windowOptions.find((o) => o.value === timeWindow)?.label ?? timeWindow;
  // what a saved view keeps: the window. the buckets are fixed at an hour here
  const savedFilters: SavedViewFilters = { window: timeWindow };
  const summary = useQuery({
    queryKey: ["analytics", "summary", timeWindow],
    queryFn: () => fetchAnalyticsSummary(readWindow(timeWindow)),
    refetchInterval: overviewPoll,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `summary` is the query the user is actually waiting on for this screen
  useScreenReady(!summary.isLoading);
  const series = useQuery({
    queryKey: ["analytics", "timeseries", timeWindow],
    queryFn: () => fetchAnalyticsTimeseries(readWindow(timeWindow)),
    refetchInterval: overviewPoll,
    retry: false,
  });
  const byModel = useQuery({
    queryKey: ["analytics", "by-model", timeWindow],
    queryFn: () => fetchAnalyticsByModel(readWindow(timeWindow)),
    refetchInterval: overviewPoll,
    retry: false,
  });
  const recent = useQuery({
    queryKey: ["invocations", "recent"],
    queryFn: () => fetchInvocations({ ...readRecent(), limit: 8 }),
    refetchInterval: pollEvery(pollMs ?? RECENT_POLL_MS),
    retry: false,
  });
  const reads = [summary, series, byModel, recent];
  // every read failed holding nothing, so the error is one screen-level alert
  // and not one per card: five alerts on mount is five announcements of one
  // outage, two of them about the same endpoint (#2342). a read still out, or
  // one that answered, makes it a partial failure, where each failed card holds
  // its own alert
  const outage = reads.every(failedEmpty);
  // one `error_state` per error placeholder on screen, which each `LoadError`
  // records itself: the screen-level alert while there is one, else the card
  // that shows it (#2444)

  // a deployment with no analytics store answers every panel on this screen the
  // same way. It used to render as an empty state, which says "nothing happened
  // yet" about a control plane that was never asked to record anything (#1236),
  // and then as the red alert a 500 gets (#1976). it is a deployment shape, so
  // it is one panel for the screen, not one per card
  const unavailable =
    reads.find((q) => q.data === undefined && isUnavailable(q.error))?.error ?? null;

  // the Recent card says its refresh failed in its header, not in a line of its
  // own, and that is the same appearance (#2640). it is said only while the
  // cards are drawn, as the other lines are
  useErrorState(
    !unavailable && !outage && recent.isError && recent.data !== undefined,
    "dashboard-recent-stale",
  );

  if (unavailable || outage) {
    return (
      <PageBody>
        {/* a deployment with no analytics store still has a first run, and the
            checklist below reads rows rather than traffic (#1585). an outage
            leaves it as it is for the same reason */}
        <GettingStarted />
        {unavailable ? (
          <AnalyticsUnavailable error={unavailable} i18nKey="pages.dashboard.noAnalytics" />
        ) : (
          // the figures' error speaks for the screen: it is the read the screen
          // waits on. the retry asks for every read, since any one of them may
          // be the one that answers, and each goes back to a skeleton
          <LoadError
            error={summary.error}
            resource={t("errors.resources.analytics")}
            onRetry={() => reads.forEach((q) => void q.refetch())}
            target="dashboard-analytics"
          />
        )}
      </PageBody>
    );
  }

  const fmtK = fmt.compact;

  // "live" is said only while the last read of this card succeeded (#1984), so
  // not before the first one has answered either (#2341). a first load that
  // failed has stopped polling, and a failure with rows on screen is retried by
  // the next poll, so only the second one says retrying
  const recentFailedAt = fmt.time(recent.errorUpdatedAt);
  const recentFeed = recent.isError
    ? recent.data === undefined
      ? t("pages.dashboard.feed.loadFailed", { time: recentFailedAt })
      : t("pages.dashboard.feed.refreshFailedRetrying", { time: recentFailedAt })
    : recent.isSuccess
      ? t("pages.dashboard.live")
      : t("pages.dashboard.feed.loading");

  return (
    <PageBody className="gap-[18px]">
      <div className="flex flex-wrap items-center gap-2">
        <Combobox
          aria-label={t("common.timeWindow.label")}
          className="w-48"
          options={windowOptions}
          value={timeWindow}
          onChange={(next) => setWindow(readTimeWindow(next))}
        />
        <SavedViews
          surface="dashboard"
          current={savedFilters}
          onApply={(view) => setWindow(readTimeWindow(view.window))}
        />
      </div>
      <GettingStarted requests={summary.isSuccess ? num(summary.data?.requests) : undefined} />
      <div data-testid="dashboard-figures">
        <CardRead
          read={summary}
          resource={t("errors.resources.dashboardFigures")}
          target="dashboard"
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
              <div className={STAT_GRID}>
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
        <RefreshFailed read={summary} target="dashboard-stale" />
      </div>

      <IncompleteSpendNotice
        requests={num(summary.data?.unpriced_requests)}
        models={num(summary.data?.unpriced_models)}
      />

      <div className="grid gap-3.5 xl:grid-cols-[1.6fr_1fr]">
        <Card data-testid="dashboard-spend">
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {windowLabel}
            </CardDescription>
            <CardTitle>{t("pages.dashboard.spendTitle")}</CardTitle>
            {/* the amounts follow the deployment's currency, so the subtitle
                names it rather than claiming dollars (#1182) */}
            <CardDescription>{t("pages.dashboard.spendSub", { currency })}</CardDescription>
          </CardHeader>
          <CardContent>
            <CardRead
              read={series}
              resource={t("errors.resources.dashboardSpend")}
              target="dashboard-spend"
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
                    series={[{ name: t("pages.dashboard.spendTitle"), values: spendPoints }]}
                    labels={spendLabels}
                    height={220}
                    label={t("pages.dashboard.spendChartAria", { window: windowLabel })}
                    formatValue={(v) => money(v)}
                    emptyState={
                      <p className="text-sm text-muted-foreground">{t("analytics.noRowsYet")}</p>
                    }
                  />
                );
              }}
            </CardRead>
            <RefreshFailed read={series} target="dashboard-spend-stale" />
          </CardContent>
        </Card>
        <Card data-testid="dashboard-traffic">
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {windowLabel}
            </CardDescription>
            <CardTitle>{t("pages.dashboard.trafficTitle")}</CardTitle>
            <CardDescription>{t("pages.dashboard.trafficSub")}</CardDescription>
          </CardHeader>
          <CardContent>
            <CardRead
              read={byModel}
              resource={t("errors.resources.dashboardTrafficShare")}
              target="dashboard-traffic"
              skeleton={
                <LoadingRegion>
                  <Skeleton height={180} />
                </LoadingRegion>
              }
            >
              {(models) => {
                const traffic = byRequests(models).map((m, i) => ({
                  label: m.model,
                  value: m.requests,
                  color: modelColor(i),
                }));
                const total = traffic.reduce((a, m) => a + m.value, 0);
                return traffic.length === 0 ? (
                  <p className="py-16 text-center text-sm text-muted-foreground">
                    {t("pages.dashboard.noTraffic")}
                  </p>
                ) : (
                  <Donut
                    segments={traffic}
                    size={150}
                    maxSegments={BARS_SHOWN + 1}
                    centerLabel={fmtK(total)}
                    // the caption agrees with the count, and russian has four forms
                    centerSub={t("pages.dashboard.requests", { count: total })}
                  />
                );
              }}
            </CardRead>
            <RefreshFailed read={byModel} target="dashboard-traffic-stale" />
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-3.5 xl:grid-cols-2">
        <Card data-testid="dashboard-by-model">
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {windowLabel}
            </CardDescription>
            <CardTitle>{t("pages.dashboard.byModelTitle")}</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2">
            <CardRead
              read={byModel}
              resource={t("errors.resources.dashboardByModel")}
              target="dashboard-by-model"
              skeleton={<BarsSkeleton />}
              // the traffic share reads this endpoint and holds the alert with its
              // retry, so this card says where the failure is and adds none
              failed={
                <p className="py-10 text-center text-sm text-muted-foreground">
                  {t("pages.dashboard.sharedRead", {
                    card: t("pages.dashboard.trafficTitle"),
                    retry: t("errors.load.retry"),
                  })}
                </p>
              }
            >
              {(models) => {
                const ranked = byRequests(models);
                const barMax = Math.max(1, ...ranked.map((m) => m.requests));
                const bars = ranked.slice(0, BARS_SHOWN).map((m, i) => ({
                  label: m.model,
                  value: m.requests,
                  pct: (m.requests / barMax) * 100,
                  color: modelColor(i),
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
            <RefreshFailed read={byModel} target="dashboard-by-model-stale" />
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
            <CardTitle>{t("pages.dashboard.recentTitle")}</CardTitle>
          </CardHeader>
          <CardContent>
            <CardRead
              read={recent}
              resource={t("errors.resources.dashboardRecent")}
              target="dashboard-recent"
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
                    className={RECENT_TABLE}
                    columns={[
                      {
                        key: "t",
                        header: t("pages.dashboard.colTime"),
                        mono: true,
                        render: (_, row) => (
                          <RequestTime
                            ts={row.ts as string}
                            requestId={row.requestId as string}
                            model={row.model as string}
                          />
                        ),
                      },
                      {
                        key: "model",
                        header: t("pages.dashboard.colModel"),
                        mono: true,
                        render: (v) => <span title={v as string}>{v as string}</span>,
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
                        ts: r.ts,
                        requestId: r.request_id,
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

// when a request happened, and the way into it. the clock is always there; the
// day sits under it only on a row that is not from today, since a window of 24
// hours crosses midnight and a bare clock then reads as today's. the row opens
// the request in LLM Logs: the link is the time cell's, stretched over the row
// (`after:absolute`, against the `relative` row in `RECENT_TABLE`), and is the
// keyboard's and the screen reader's way in, named for the request it opens. a
// row with no request id has nothing to open, so it is the time alone
function RequestTime({ ts, requestId, model }: { ts: string; requestId: string; model: string }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const day = fmt.dayUnlessToday(ts);
  const when = (
    <>
      <span className="block">{fmt.time(ts)}</span>
      {day && <span className="block text-[0.6875rem] text-[color:var(--text-subtle)]">{day}</span>}
    </>
  );
  if (!requestId) return when;
  const stamp = fmt.dateTime(ts);
  return (
    <Link
      to={`/logs?request_id=${encodeURIComponent(requestId)}`}
      title={stamp}
      aria-label={t("pages.dashboard.openRequest", { model, time: stamp })}
      className="rounded-sm hover:underline focus-visible:outline-none focus-visible:after:ring-1 focus-visible:after:ring-inset focus-visible:after:ring-ring after:absolute after:inset-0"
    >
      {when}
    </Link>
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
