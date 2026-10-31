import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Filter, FilterX } from "lucide-react";
import { Link, useLocation } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { PageBody } from "@/components/screen";
import { FleetHealthTile } from "@/components/FleetHealthTile";
import { ProviderHealthTile } from "@/components/ProviderHealthTile";
import { GettingStarted } from "@/components/GettingStarted";
import { IncompleteSpendNotice } from "@/components/IncompleteSpendNotice";
import { SampledLogNotice } from "@/components/SampledLogNotice";
import { WithheldSpendNotice } from "@/components/WithheldSpendNotice";
import { LoadError } from "@/components/LoadError";
import { OwnerOrgPill, sharedNames } from "@/components/OwnerOrgPill";
import { RowFilterPicker, useAddressRowFilters } from "@/components/RowFilters";
import { SavedViews } from "@/components/SavedViews";
import { HttpStatusBadge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { TimeWindowPicker } from "@/components/TimeWindowPicker";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { BarTrack } from "@/components/ui/bar-chart";
import { LineChart } from "@/components/ui/line-chart";
import { Skeleton } from "@/components/ui/skeleton";
import { ListSkeleton, LoadingRegion } from "@/components/LoadingState";
import { STAT_GRID, StatCard } from "@/components/ui/stat-card";
import { Table } from "@/components/ui/table";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsByModel,
  fetchAnalyticsSummary,
  fetchAnalyticsTimeseries,
  fetchInvocations,
  type AnalyticsByModelRow,
  type AnalyticsRowFilters,
  type AnalyticsSummary,
  type InvocationRow,
  type SavedViewFilters,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { modelColor, rankedByRequests } from "@/lib/model-colors";
import { isAwaiting } from "@/lib/read-state";
import {
  DEFAULT_TIME_WINDOW,
  chartBucket,
  useAddressSelection,
  useWindowLabel,
  previousWindowBounds,
  windowBounds,
  windowFilters,
  type WindowSelection,
  useWindowQueryKey,
} from "@/lib/time-window";
import { compare, toneOfChange, type Comparison } from "@/lib/compare";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";
import { staleTarget } from "@/lib/ux";

const num = (v: number | string | undefined): number => Number(v ?? 0);

// the figures and charts read one window, by name: the last 24 hours unless the
// address names another (`?window=7d`). its bounds are worked out as each
// request leaves rather than once when the module loads, so a tab left open
// reads the window as it is now and not as it was when it was opened (#1975).
// the name is what the query keys carry
const readWindow = (name: WindowSelection, filters: AnalyticsRowFilters) => ({
  ...windowBounds(name),
  bucket: chartBucket(name),
  ...filters,
});
// the same figures over the window before, for each tile's arrow (#2010)
const readPrevious = (name: WindowSelection, filters: AnalyticsRowFilters) => ({
  ...previousWindowBounds(name),
  bucket: chartBucket(name),
  ...filters,
});
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

// the by-model card lists this many models, busiest first: the length of the
// categorical colour sequence, so no two rows share a colour
const MODELS_SHOWN = 8;

// one organization's model (#3486): two organizations may each have a route of
// one name (#1857), and the read answers them as two rows, so every per-model
// map is keyed on both. a NUL cannot occur in either part
const modelKey = (m: AnalyticsByModelRow) => `${m.org_id ?? ""}\u0000${m.model}`;

// the models in the order the card colours them by (#1994): the read arrives
// by cost, and the card ranks it by requests
function byRequests(models: AnalyticsByModelRow[]) {
  return rankedByRequests(
    models.map((m) => ({
      key: modelKey(m),
      orgId: m.org_id ?? "",
      model: m.model,
      requests: num(m.requests),
    })),
  );
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

// the by-model card's table, in the card's frame like the recent one. the model
// column takes what the figures leave and truncates in it
const MODEL_TABLE = cn(
  "rounded-md border-0",
  "[&_th]:px-2 [&_td]:px-2 sm:[&_th]:px-4 sm:[&_td]:px-4",
  "[&_th:first-child]:w-full [&_td:first-child]:max-w-0 [&_td:first-child]:truncate",
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
  region,
}: {
  read: Pick<UseQueryResult<unknown>, "isError" | "data" | "errorUpdatedAt">;
  /** the card's region; the UX event's target is `staleTarget(region)`, told apart from a `LoadError` */
  region: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const stale = read.isError && read.data !== undefined;
  useErrorState(stale, staleTarget(region));
  if (!stale) return null;
  return (
    <p className="mt-2 text-xs text-[color:var(--status-danger-text)]">
      {t("pages.dashboard.feed.refreshFailedRetrying", { time: fmt.time(read.errorUpdatedAt) })}
    </p>
  );
}

// live overview, shaped by docs/dev-docs/development/dashboard-home.md: fleet
// health, p95, error rate and cache-hit rate as tiles, then requests and spend,
// an hourly spend line, one by-model table and the latest requests. each card
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
  const [timeWindow, setWindow] = useAddressSelection();
  // the window's days are the chart zone's, so a zone change reads it again (#3519)
  const windowKey = useWindowQueryKey(timeWindow);
  const windowLabel = useWindowLabel()(timeWindow);
  // the figures cover the rows the caller's roles reach, not the org picked in
  // the scope switcher, so the scope line says that and not the switcher's pick
  const auth = useOptionalAuth();
  const { search } = useLocation();
  // the row filters live in the address beside the window and go to every read
  // on the screen (#2725). the query keys carry them, so a pick refetches
  const rowFilters = useAddressRowFilters();
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const filterRegion = React.useId();
  const filterKey = JSON.stringify(rowFilters.query);
  const filtered = rowFilters.count > 0;
  // what a saved view keeps: the window and the row filters. the buckets are
  // fixed at an hour here
  const savedFilters: SavedViewFilters = { ...windowFilters(timeWindow), ...rowFilters.saved };
  const summary = useQuery({
    queryKey: ["analytics", "summary", ...windowKey, filterKey],
    queryFn: () => fetchAnalyticsSummary(readWindow(timeWindow, rowFilters.query)),
    refetchInterval: overviewPoll,
    retry: false,
  });

  // the window before, read beside the figures and never in their way: it has
  // no card, no loading state and no error of its own, and while it is out or
  // has failed a tile simply draws no arrow (#2010)
  const previous = useQuery({
    queryKey: ["analytics", "summary", "previous", ...windowKey, filterKey],
    queryFn: () => fetchAnalyticsSummary(readPrevious(timeWindow, rowFilters.query)),
    refetchInterval: overviewPoll,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `summary` is the query the user is actually waiting on for this screen
  useScreenReady(!summary.isLoading);
  const series = useQuery({
    queryKey: ["analytics", "timeseries", ...windowKey, filterKey],
    queryFn: () => fetchAnalyticsTimeseries(readWindow(timeWindow, rowFilters.query)),
    refetchInterval: overviewPoll,
    retry: false,
  });
  const byModel = useQuery({
    queryKey: ["analytics", "by-model", ...windowKey, filterKey],
    queryFn: () => fetchAnalyticsByModel(readWindow(timeWindow, rowFilters.query)),
    refetchInterval: overviewPoll,
    retry: false,
  });
  const recent = useQuery({
    queryKey: ["invocations", "recent", filterKey],
    queryFn: () => fetchInvocations({ ...readRecent(), ...rowFilters.query, limit: 8 }),
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
    staleTarget("dashboard-recent"),
  );

  if (unavailable || outage) {
    return (
      <PageBody>
        {/* a deployment with no analytics store still has a first run, and the
            checklist below reads rows rather than traffic (#1585). an outage
            leaves it as it is for the same reason */}
        <GettingStarted />
        {/* the gateways do not need analytics to be up or down, and an outage
            is when an operator opens this screen to find out */}
        <div className="max-w-sm">
          <FleetHealthTile />
        </div>
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

  // what the figures cover, stated beside them: whose requests, over which
  // window, narrowed by how many filters (#1978). "whose" waits for the session
  // rather than guess, since a member told "every request" would be misled
  const scopeLine = [
    auth?.status === "checking"
      ? undefined
      : auth?.user && !auth.user.is_superadmin
        ? t("pages.dashboard.scope.member")
        : t("pages.dashboard.scope.all"),
    windowLabel,
    filtered ? t("pages.dashboard.scope.filters", { count: rowFilters.count }) : undefined,
  ]
    .filter(Boolean)
    .join(" · ");

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

  // a filtered zero is the filter's, not a first run with nothing served yet
  const requestsServed = summary.isSuccess && !filtered ? num(summary.data?.requests) : undefined;
  // with traffic the telemetry leads and the checklist sits under it; without,
  // the checklist is what there is to look at
  const served = (requestsServed ?? 0) > 0;
  const gettingStarted = <GettingStarted requests={requestsServed} />;

  return (
    <PageBody className="gap-[18px]">
      <div className="flex flex-wrap items-center gap-2">
        <TimeWindowPicker
          aria-label={t("common.timeWindow.label")}
          className="w-48"
          value={timeWindow}
          onChange={setWindow}
        />
        <SavedViews surface="dashboard" current={savedFilters} onApply={rowFilters.apply} />
        <Button
          size="sm"
          variant="outline"
          aria-expanded={filtersOpen}
          aria-controls={filterRegion}
          onClick={() => setFiltersOpen((open) => !open)}
        >
          <Filter aria-hidden className="h-3.5 w-3.5" />
          {t("common.filters")}
          {filtered && ` · ${rowFilters.count}`}
        </Button>
      </div>
      <p data-testid="dashboard-scope" className="text-xs text-muted-foreground">
        <span className="sr-only">{t("pages.dashboard.scope.label")}: </span>
        {scopeLine}
      </p>
      {filtersOpen && (
        <section
          id={filterRegion}
          aria-label={t("common.filters")}
          className="flex flex-col gap-2 rounded-lg border border-[color:var(--border-subtle)] p-3"
        >
          <div className="grid gap-x-4 gap-y-1 sm:grid-cols-2 xl:grid-cols-4">
            <RowFilterPicker values={rowFilters.values} setters={rowFilters.setters} />
          </div>
          <Button
            variant="ghost"
            size="sm"
            disabled={!filtered}
            onClick={rowFilters.clear}
            className="w-fit px-2 text-muted-foreground"
          >
            <FilterX aria-hidden className="h-3.5 w-3.5" />
            {t("common.clearFilters")}
          </Button>
        </section>
      )}
      {!served && gettingStarted}
      <div data-testid="dashboard-figures">
        <div className={STAT_GRID}>
          {/* the fleet first: mid-incident, whether the gateways are up is the
              first thing asked, and it is not a figure of the window. it sits
              outside the summary's read, so a summary that is slow or failing
              (the heaviest read on the screen) does not take it away; the other
              five tiles, or their skeleton or alert, fill the rest of the grid */}
          <FleetHealthTile />
          <ProviderHealthTile pollMs={pollMs} />
          <CardRead
            read={summary}
            resource={t("errors.resources.dashboardFigures")}
            target="dashboard"
            // `Skeleton` is `aria-hidden`, so bare ones are a loading state no
            // screen reader could hear (#1605); the region lays out as the
            // tiles it stands in for
            skeleton={
              <LoadingRegion className="contents">
                {Array.from({ length: 6 }, (_, i) => (
                  <Skeleton key={i} height={96} radius={10} />
                ))}
              </LoadingRegion>
            }
            failed={
              <div className="sm:col-span-1 xl:col-span-2">
                <LoadError
                  error={summary.error}
                  resource={t("errors.resources.dashboardFigures")}
                  onRetry={() => void summary.refetch()}
                  target="dashboard"
                />
              </div>
            }
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
              // the window before, when it answered and held any traffic: a
              // first day compares with nothing rather than rising from zero
              const p = previous.isSuccess ? previous.data : undefined;
              const prevRequests = p ? num(p.requests) : 0;
              const before = prevRequests > 0 ? p : undefined;
              const was = (v: number | string | undefined) => (before ? num(v) : undefined);
              // the 95th percentile, not the mean: a mean hides the slow tail an
              // operator is looking for. an older control plane sends no p95, and
              // then the tile says it is not measured rather than showing the mean
              const p95 = measured && s?.p95_latency_ms != null ? num(s.p95_latency_ms) : undefined;
              const hits = s?.cache_hit_requests != null ? num(s.cache_hit_requests) : undefined;
              const hitRate = measured && hits !== undefined ? (hits / requests) * 100 : undefined;
              const prevHitRate =
                before && before.cache_hit_requests != null
                  ? (num(before.cache_hit_requests) / prevRequests) * 100
                  : undefined;
              // the provider's prompt cache, apart from rolter's response cache: the
              // signal cache-aware balancing exists to improve (#3356). The share is
              // over the prompt tokens of the requests whose provider stated its cache
              // reads (#3410), so a provider that does not report them neither dilutes
              // it nor hides a real 0%. An older control plane sends no such count, and
              // there a window in which no provider reported a cached token reads as
              // unmeasured, since a provider that does not report it looks the same as
              // one that never hits
              const promptTokens = num(s?.prompt_tokens);
              const cacheRead = s?.cache_read_tokens != null ? num(s.cache_read_tokens) : undefined;
              const cacheShareOf = (summary: AnalyticsSummary) => {
                const read = num(summary.cache_read_tokens);
                if (summary.cache_reported_prompt_tokens != null) {
                  const reported = num(summary.cache_reported_prompt_tokens);
                  return {
                    read,
                    prompt: reported,
                    share: reported > 0 ? read / reported : undefined,
                  };
                }
                const prompt = num(summary.prompt_tokens);
                return {
                  read,
                  prompt,
                  share:
                    summary.cache_read_tokens != null && read > 0 && prompt > 0
                      ? read / prompt
                      : undefined,
                };
              };
              const cacheShare = s ? cacheShareOf(s) : undefined;
              const readShare =
                cacheShare?.share !== undefined ? cacheShare.share * 100 : undefined;
              const prevShare = before ? cacheShareOf(before).share : undefined;
              const prevReadShare = prevShare !== undefined ? prevShare * 100 : undefined;
              const cacheUnmeasured =
                s?.cache_unmeasured_requests != null ? num(s.cache_unmeasured_requests) : 0;
              // the read and prompt tokens the share is over, and the requests it left out
              const cacheDetail = () => {
                const detail = t("pages.dashboard.promptCacheTokens", {
                  read: fmt.compact(cacheShare?.read ?? 0),
                  prompt: fmt.compact(cacheShare?.prompt ?? 0),
                });
                return cacheUnmeasured > 0
                  ? t("pages.dashboard.withChange", {
                      detail,
                      change: t("pages.dashboard.promptCacheUnmeasured", {
                        count: cacheUnmeasured,
                      }),
                    })
                  : detail;
              };
              const changes = {
                readShare: compare(readShare, prevReadShare),
                requests: compare(requests, was(before?.requests)),
                spend: compare(num(s?.cost_usd), was(before?.cost_usd)),
                latency: compare(
                  p95,
                  before?.p95_latency_ms != null ? num(before.p95_latency_ms) : undefined,
                ),
                hitRate: compare(hitRate, prevHitRate),
                errorRate: compare(
                  errorRate,
                  before ? (num(before.errors) / prevRequests) * 100 : undefined,
                ),
              };
              const said = (c: Comparison) =>
                t("pages.dashboard.vsPrevious", {
                  change: fmt.number(c.ratio, {
                    style: "percent",
                    signDisplay: "exceptZero",
                    maximumFractionDigits: 1,
                  }),
                });
              // the percentage tiles show one decimal at most
              const percent = (v: number) =>
                fmt.number(v, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
              return (
                <>
                  <StatCard
                    label={t("pages.dashboard.statP95")}
                    value={p95 !== undefined ? fmt.number(Math.round(p95)) : unmeasured}
                    unit={p95 !== undefined ? t("pages.dashboard.colMs") : undefined}
                    trend={changes.latency?.trend}
                    tone={changes.latency ? toneOfChange(changes.latency.trend, "bad") : undefined}
                    delta={measured ? (changes.latency ? said(changes.latency) : undefined) : why}
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
                    // an arrow only for a change measured against the window
                    // before (#2010). with nothing to compare, above 1% of
                    // requests the count reads as the problem it is rather than
                    // as growth (#1974)
                    trend={changes.errorRate?.trend}
                    tone={
                      changes.errorRate
                        ? toneOfChange(changes.errorRate.trend, "bad")
                        : errorRate !== undefined && errorRate > 1
                          ? "bad"
                          : "neutral"
                    }
                    // russian needs four plural forms here where english needs two
                    delta={
                      errorRate === undefined
                        ? why
                        : errors > 0 && changes.errorRate
                          ? t("pages.dashboard.withChange", {
                              detail: t("pages.dashboard.errors", { count: errors }),
                              change: said(changes.errorRate),
                            })
                          : errors > 0
                            ? t("pages.dashboard.errors", { count: errors })
                            : changes.errorRate
                              ? said(changes.errorRate)
                              : undefined
                    }
                  />
                  <StatCard
                    label={t("pages.dashboard.statCacheHit")}
                    value={hitRate !== undefined ? percent(hitRate) : unmeasured}
                    unit={hitRate !== undefined ? "%" : undefined}
                    // more hits is the point of cache-aware balancing
                    trend={changes.hitRate?.trend}
                    tone={changes.hitRate ? toneOfChange(changes.hitRate.trend, "good") : undefined}
                    delta={
                      !measured
                        ? why
                        : hits === undefined
                          ? undefined
                          : changes.hitRate
                            ? t("pages.dashboard.withChange", {
                                detail:
                                  hits === 0
                                    ? t("pages.dashboard.cacheHitsNone")
                                    : t("pages.dashboard.cacheHits", { count: hits }),
                                change: said(changes.hitRate),
                              })
                            : hits === 0
                              ? t("pages.dashboard.cacheHitsNone")
                              : t("pages.dashboard.cacheHits", { count: hits })
                    }
                  />
                  <StatCard
                    data-testid="dashboard-prompt-cache"
                    label={t("pages.dashboard.statPromptCache")}
                    value={readShare !== undefined ? percent(readShare) : unmeasured}
                    unit={readShare !== undefined ? "%" : undefined}
                    // more of the prompt served from the cache is what cache-aware routing is for
                    trend={changes.readShare?.trend}
                    tone={
                      changes.readShare ? toneOfChange(changes.readShare.trend, "good") : undefined
                    }
                    delta={
                      promptTokens === 0
                        ? why
                        : cacheRead === undefined
                          ? undefined
                          : readShare === undefined
                            ? t("pages.dashboard.promptCacheNone")
                            : changes.readShare
                              ? t("pages.dashboard.withChange", {
                                  detail: cacheDetail(),
                                  change: said(changes.readShare),
                                })
                              : cacheDetail()
                    }
                  />
                  <StatCard
                    label={t("pages.dashboard.statRequests")}
                    value={fmt.number(requests)}
                    trend={changes.requests?.trend}
                    tone={
                      changes.requests ? toneOfChange(changes.requests.trend, "good") : undefined
                    }
                    delta={changes.requests ? said(changes.requests) : undefined}
                  />
                  <StatCard
                    label={t("pages.dashboard.statSpend")}
                    value={money(num(s?.cost_usd))}
                    // more spend is not bad news on its own: it follows traffic
                    trend={changes.spend?.trend}
                    tone="neutral"
                    delta={changes.spend ? said(changes.spend) : undefined}
                  />
                </>
              );
            }}
          </CardRead>
        </div>
        <RefreshFailed read={summary} region="dashboard" />
      </div>

      <SampledLogNotice kind="figures" />
      <IncompleteSpendNotice
        requests={num(summary.data?.unpriced_requests)}
        models={num(summary.data?.unpriced_models)}
      />
      <WithheldSpendNotice
        withheld={num(summary.data?.withheld_requests)}
        withheldCost={num(summary.data?.withheld_cost)}
        unknownUsage={num(summary.data?.usage_unknown_requests)}
        search={search}
      />

      {served && gettingStarted}

      <div className="grid gap-3.5 xl:grid-cols-[1.6fr_1fr]">
        <Card data-testid="dashboard-spend">
          <CardHeader>
            <CardDescription className="text-2xs uppercase tracking-[0.07em]">
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
                // an hourly bucket is named by its clock, a daily one (a long
                // custom range) by its date, which the clock cannot tell apart
                const daily = chartBucket(timeWindow) === "day";
                const spendLabels = points.map((p) =>
                  daily
                    ? fmt.calendarDay(String(p.bucket).slice(0, 10))
                    : fmt.timeShort(p.bucket) || p.bucket,
                );
                // buckets exist but every one of them is zero: the window had
                // traffic that was never priced, which is not the same as spend
                // that happened to be zero
                const anySpend = spendPoints.some((v) => v > 0);
                return spendPoints.length === 0 ? (
                  <p className="py-16 text-center text-sm text-muted-foreground">
                    {filtered ? t("pages.dashboard.noMatch") : t("analytics.noRowsYet")}
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
                    // one series, so it takes the neutral zinc rather than the
                    // first (red) series colour: a red spend line reads as an
                    // alarm, and red is the dashboard's one voice (#1991)
                    series={[
                      {
                        name: t("pages.dashboard.spendTitle"),
                        values: spendPoints,
                        color: "var(--chart-2)",
                      },
                    ]}
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
            <RefreshFailed read={series} region="dashboard-spend" />
          </CardContent>
        </Card>
        <Card data-testid="dashboard-by-model">
          <CardHeader>
            <CardDescription className="text-2xs uppercase tracking-[0.07em]">
              {windowLabel}
            </CardDescription>
            <CardTitle>{t("pages.dashboard.byModelTitle")}</CardTitle>
            <CardDescription>{t("pages.dashboard.byModelSub")}</CardDescription>
          </CardHeader>
          <CardContent>
            <CardRead
              read={byModel}
              resource={t("errors.resources.dashboardByModel")}
              target="dashboard-by-model"
              skeleton={<ListSkeleton rows={4} />}
            >
              {(models) => {
                const ranked = byRequests(models);
                const barMax = Math.max(1, ...ranked.map((m) => m.requests));
                const p95Of = new Map(models.map((m) => [modelKey(m), m.p95_latency_ms]));
                const costOf = new Map(models.map((m) => [modelKey(m), m.cost_usd]));
                // a name two organizations share is told apart by its owner
                const shared = sharedNames(ranked.map((m) => ({ name: m.model, orgId: m.orgId })));
                const rows = ranked.slice(0, MODELS_SHOWN).map((m, i) => ({
                  key: m.key,
                  model: m.model,
                  orgId: m.orgId,
                  shared: shared.has(m.model),
                  color: modelColor(i),
                  requests: m.requests,
                  pct: (m.requests / barMax) * 100,
                  p95: p95Of.get(m.key),
                  cost: costOf.get(m.key),
                }));
                const hidden = ranked.length - rows.length;
                return rows.length === 0 ? (
                  <p className="py-10 text-center text-sm text-muted-foreground">
                    {filtered ? t("pages.dashboard.noMatch") : t("pages.dashboard.noTraffic")}
                  </p>
                ) : (
                  <>
                    <Table
                      rowKey="key"
                      className={MODEL_TABLE}
                      columns={[
                        {
                          key: "model",
                          header: t("pages.dashboard.colModel"),
                          mono: true,
                          render: (v, row) => (
                            <span className="inline-flex max-w-full items-center gap-2">
                              <span
                                aria-hidden
                                className="h-2 w-2 flex-none rounded-[2px]"
                                style={{ background: row.color as string }}
                              />
                              <span className="truncate" title={v as string}>
                                {v as string}
                              </span>
                              {row.shared ? <OwnerOrgPill orgId={row.orgId as string} /> : null}
                            </span>
                          ),
                        },
                        {
                          key: "requests",
                          header: t("pages.dashboard.colRequests"),
                          align: "right",
                          mono: true,
                          render: (v, row) => (
                            <span className="inline-flex items-center justify-end gap-2">
                              {/* the bar is the share of the busiest model; the
                                  number beside it is what a reader takes in */}
                              <BarTrack
                                share={(row.pct as number) / 100}
                                color={row.color as string}
                                className="hidden h-2 w-16 sm:block"
                              />
                              <span>{fmtK(v as number)}</span>
                            </span>
                          ),
                        },
                        {
                          key: "p95",
                          header: t("pages.dashboard.colP95"),
                          align: "right",
                          mono: true,
                          render: (v) =>
                            v == null ? (
                              t("pages.dashboard.notMeasured")
                            ) : (
                              <>
                                {fmt.number(Math.round(num(v as number | string)))}{" "}
                                <span className="text-muted-foreground">
                                  {t("pages.dashboard.colMs")}
                                </span>
                              </>
                            ),
                        },
                        {
                          key: "cost",
                          header: t("pages.dashboard.colSpend"),
                          align: "right",
                          mono: true,
                          render: (v) => money(num(v as number | string | undefined)),
                        },
                      ]}
                      data={rows as unknown as Record<string, unknown>[]}
                    />
                    {hidden > 0 && (
                      <p className="mt-2 text-xs text-muted-foreground">
                        {t("pages.dashboard.byModelMore", { count: hidden })}
                      </p>
                    )}
                  </>
                );
              }}
            </CardRead>
            <RefreshFailed read={byModel} region="dashboard-by-model" />
          </CardContent>
        </Card>
      </div>

      <div>
        <Card data-testid="dashboard-recent">
          <CardHeader>
            <CardDescription
              className={cn(
                "text-2xs uppercase tracking-[0.07em]",
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
                    {filtered ? t("pages.dashboard.noMatch") : t("pages.dashboard.nothingLogged")}
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
                        render: (v) => <HttpStatusBadge status={v as number} />,
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
            {/* the whole log, not the eight rows: the address carries the window
                and the filters the figures above were read with, under the
                names LLM Logs reads them by (#1978) */}
            {recent.data && recent.data.length > 0 && (
              <Link
                to={{ pathname: "/logs", search }}
                data-testid="dashboard-recent-all"
                className="mt-3 inline-block rounded-sm text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t("pages.dashboard.recentAll")}
              </Link>
            )}
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
      {day && <span className="block text-2xs text-[color:var(--text-subtle)]">{day}</span>}
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
