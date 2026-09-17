import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { PageBody } from "@/components/screen";
import { GettingStarted } from "@/components/GettingStarted";
import { IncompleteSpendNotice } from "@/components/IncompleteSpendNotice";
import { LoadError } from "@/components/LoadError";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Donut } from "@/components/ui/donut";
import { LineChart } from "@/components/ui/line-chart";
import { Skeleton } from "@/components/ui/skeleton";
import {
  ListSkeleton,
  LoadingRegion,
  StatGridSkeleton,
} from "@/components/LoadingState";
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
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const num = (v: number | string | undefined): number => Number(v ?? 0);

const WINDOW = {
  since: new Date(Date.now() - 86_400_000).toISOString(),
  bucket: "hour",
};

// the shared categorical sequence (#1245), not a fifth hand-written list
const BAR_PALETTE = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
];

function isUnavailable(err: unknown): boolean {
  return err instanceof AnalyticsUnavailableError;
}

// live overview from the design prototype: 4 KPIs, hourly spend line, traffic
// donut, requests-by-provider bars, and a recent-requests mini table. all
// clickhouse-backed; renders a calm not-configured state when analytics is off.
export default function Dashboard() {
  const { t } = useTranslation();
  // formatting follows the dashboard language, not the browser locale
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const money = (n: number) => fmt.currency(n, currency);
  const summary = useQuery({
    queryKey: ["analytics", "summary", "24h"],
    queryFn: () => fetchAnalyticsSummary(WINDOW),
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `summary` is the query the user is actually waiting on for this screen
  useScreenReady(!summary.isLoading);
  useErrorState(!!summary.error, "dashboard");
  const series = useQuery({
    queryKey: ["analytics", "timeseries", "24h"],
    queryFn: () => fetchAnalyticsTimeseries(WINDOW),
    retry: false,
  });
  const byModel = useQuery({
    queryKey: ["analytics", "by-model", "24h"],
    queryFn: () => fetchAnalyticsByModel(WINDOW),
    retry: false,
  });
  const recent = useQuery({
    queryKey: ["invocations", "recent"],
    queryFn: () => fetchInvocations({ ...WINDOW, limit: 8 }),
    retry: false,
  });

  // a deployment with no analytics store answers every panel on this screen the
  // same way. It used to render as an empty state, which says "nothing happened
  // yet" about a control plane that was never asked to record anything (#1236)
  const unavailable =
    [summary.error, series.error, byModel.error].find(isUnavailable) ?? null;

  if (unavailable) {
    return (
      <PageBody>
        {/* a deployment with no analytics store still has a first run, and the
            checklist below reads rows rather than traffic (#1585) */}
        <GettingStarted />
        <LoadError error={unavailable} resource={t("errors.resources.analytics")} />
      </PageBody>
    );
  }

  // a failed summary is a failure, not a quiet day: without this branch a 5xx
  // or an expired session rendered "0 requests · $0.00" and nothing else
  const failed = summary.error ?? series.error ?? byModel.error;
  if (failed) {
    return (
      <PageBody>
        <LoadError
          error={failed}
          resource={t("errors.resources.analytics")}
          onRetry={() => {
            void summary.refetch();
            void series.refetch();
            void byModel.refetch();
          }}
        />
      </PageBody>
    );
  }

  const s = summary.data;
  const requests = num(s?.requests);
  const errors = num(s?.errors);
  const errorRate = requests > 0 ? (errors / requests) * 100 : 0;

  const spendPoints = (series.data ?? []).map((p) => num(p.cost_usd));
  const spendLabels = (series.data ?? []).map(
    (p) => fmt.timeShort(p.bucket) || p.bucket,
  );
  // buckets exist but every one of them is zero: the window had traffic that
  // was never priced, which is not the same as spend that happened to be zero
  const anySpend = spendPoints.some((v) => v > 0);

  const models = byModel.data ?? [];
  const traffic = models.map((m) => ({
    label: m.model,
    value: num(m.requests),
  }));
  const totalReq = traffic.reduce((a, t) => a + t.value, 0);
  const fmtK = fmt.compact;

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

  const recentRows = (recent.data ?? []).map((r: InvocationRow) => ({
    id: r.request_id || r.ts,
    t: fmt.time(r.ts),
    model: r.model,
    status: num(r.status),
    lat: Math.round(num(r.latency_ms)),
  }));

  return (
    <PageBody className="gap-[18px]">
      <GettingStarted requests={summary.isSuccess ? requests : undefined} />
      {summary.isLoading ? (
        // `Skeleton` is `aria-hidden`, so the four bare ones this used to
        // render were a loading state no screen reader could hear (#1605)
        <StatGridSkeleton cards={4} />
      ) : (
        <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2 xl:grid-cols-4">
          <StatCard
              label={t("pages.dashboard.statRequests")}
              value={fmt.number(requests)}
            />
            <StatCard
              label={t("pages.dashboard.statSpend")}
              value={money(num(s?.cost_usd))}
            />
            <StatCard
              label={t("pages.dashboard.statAvgLatency")}
              value={fmt.number(Math.round(num(s?.avg_latency_ms)))}
              unit={t("pages.dashboard.colMs")}
            />
            <StatCard
              label={t("pages.dashboard.statErrorRate")}
              value={fmt.number(errorRate, {
                minimumFractionDigits: 2,
                maximumFractionDigits: 2,
              })}
              unit="%"
              trend={errorRate > 1 ? "up" : "flat"}
              // russian needs four plural forms here where english needs two
              delta={
                errors > 0
                  ? t("pages.dashboard.errors", { count: errors })
                  : undefined
              }
          />
        </div>
      )}

      <IncompleteSpendNotice
        requests={num(s?.unpriced_requests)}
        models={num(s?.unpriced_models)}
      />

      <div className="grid gap-3.5 xl:grid-cols-[1.6fr_1fr]">
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.last24h")}
            </CardDescription>
            <CardTitle className="text-base">
              {t("pages.dashboard.spendTitle")}
            </CardTitle>
            <CardDescription>{t("pages.dashboard.spendSub")}</CardDescription>
          </CardHeader>
          <CardContent>
            {series.isLoading ? (
              <LoadingRegion>
                <Skeleton height={220} />
              </LoadingRegion>
            ) : spendPoints.length === 0 ? (
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
                  <p className="text-sm text-muted-foreground">
                    {t("analytics.noRowsYet")}
                  </p>
                }
              />
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.last24h")}
            </CardDescription>
            <CardTitle className="text-base">
              {t("pages.dashboard.trafficTitle")}
            </CardTitle>
            <CardDescription>{t("pages.dashboard.trafficSub")}</CardDescription>
          </CardHeader>
          <CardContent>
            {byModel.isLoading ? (
              <LoadingRegion>
                <Skeleton height={180} />
              </LoadingRegion>
            ) : traffic.length === 0 ? (
              <p className="py-16 text-center text-sm text-muted-foreground">
                {t("pages.dashboard.noTraffic")}
              </p>
            ) : (
              <Donut
                segments={traffic}
                size={150}
                centerLabel={fmtK(totalReq)}
                centerSub={t("pages.dashboard.requests")}
              />
            )}
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-3.5 xl:grid-cols-2">
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.last24h")}
            </CardDescription>
            <CardTitle className="text-base">
              {t("pages.dashboard.byModelTitle")}
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 px-0.5 py-1">
            {bars.length === 0 && (
              <p className="py-10 text-center text-sm text-muted-foreground">
                {t("pages.dashboard.noTraffic")}
              </p>
            )}
            {bars.map((b) => (
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
            ))}
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              {t("pages.dashboard.live")}
            </CardDescription>
            <CardTitle className="text-base">
              {t("pages.dashboard.recentTitle")}
            </CardTitle>
          </CardHeader>
          <CardContent>
            {recent.isLoading ? (
              <ListSkeleton rows={4} />
            ) : recentRows.length === 0 ? (
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
                data={recentRows as unknown as Record<string, unknown>[]}
              />
            )}
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
