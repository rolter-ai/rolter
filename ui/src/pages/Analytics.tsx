import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { BarChart } from "@/components/ui/bar-chart";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { StatCard } from "@/components/ui/stat-card";
import { Table } from "@/components/ui/table";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsByModel,
  fetchAnalyticsSummary,
  fetchAnalyticsTimeseries,
} from "@/lib/api";

// window presets → number of days back; drives every analytics query's `since`
const WINDOWS: { value: string; label: string; days: number }[] = [
  { value: "24h", label: "Last 24h", days: 1 },
  { value: "7d", label: "Last 7 days", days: 7 },
  { value: "14d", label: "Last 14 days", days: 14 },
  { value: "30d", label: "Last 30 days", days: 30 },
];

const num = (v: number | string | undefined): number => Number(v ?? 0);
const money = (n: number) =>
  "$" + n.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });

function sinceIso(days: number): string {
  return new Date(Date.now() - days * 86_400_000).toISOString();
}

// analytics rides ClickHouse; when clickhouse_url is unset the control plane
// answers 503 and the fetchers throw AnalyticsUnavailableError — treat that as
// a calm "not configured" state, not a failure banner.
function isUnavailable(err: unknown): boolean {
  return err instanceof AnalyticsUnavailableError;
}

export default function Analytics() {
  const [win, setWin] = React.useState("14d");
  const days = WINDOWS.find((w) => w.value === win)?.days ?? 14;
  const window = React.useMemo(
    () => ({ since: sinceIso(days), bucket: days <= 1 ? "hour" : "day" }),
    [days],
  );

  const summary = useQuery({
    queryKey: ["analytics", "summary", win],
    queryFn: () => fetchAnalyticsSummary(window),
    retry: false,
  });
  const series = useQuery({
    queryKey: ["analytics", "timeseries", win],
    queryFn: () => fetchAnalyticsTimeseries(window),
    retry: false,
  });
  const byModel = useQuery({
    queryKey: ["analytics", "by-model", win],
    queryFn: () => fetchAnalyticsByModel(window),
    retry: false,
  });

  const unavailable =
    isUnavailable(summary.error) ||
    isUnavailable(series.error) ||
    isUnavailable(byModel.error);

  const header = (
    <PageHeader
      title="Analytics"
      description="Cost, latency and token usage across providers."
      actions={
        <Select
          value={win}
          onChange={(e) => setWin(e.target.value)}
          className="h-8 w-[150px] text-xs"
        >
          {WINDOWS.map((w) => (
            <option key={w.value} value={w.value}>
              {w.label}
            </option>
          ))}
        </Select>
      }
    />
  );

  if (unavailable) {
    return (
      <div className="space-y-6">
        {header}
        <div className="rounded-lg border border-[color:var(--border-default)]">
          <EmptyState
            title="Analytics not configured"
            description="Per-request usage and cost appear here once traffic flows through the gateway. Set clickhouse_url on the control plane to enable logging."
          />
        </div>
      </div>
    );
  }

  const s = summary.data;
  const requests = num(s?.requests);
  const errors = num(s?.errors);
  const errorRate = requests > 0 ? (errors / requests) * 100 : 0;

  const spendPoints = (series.data ?? []).map((p) => num(p.cost_usd));
  const spendLabels = (series.data ?? []).map((p) => p.bucket);

  return (
    <div className="space-y-6">
      {header}

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        {summary.isLoading ? (
          Array.from({ length: 4 }).map((_, i) => (
            <Skeleton key={i} height={92} radius={10} />
          ))
        ) : (
          <>
            <StatCard label="Spend" value={money(num(s?.cost_usd))} />
            <StatCard
              label="Requests"
              value={requests.toLocaleString()}
            />
            <StatCard
              label="Avg latency"
              value={Math.round(num(s?.avg_latency_ms)).toLocaleString()}
              unit="ms"
            />
            <StatCard
              label="Error rate"
              value={errorRate.toFixed(2)}
              unit="%"
              trend={errorRate > 1 ? "up" : "flat"}
              delta={errors > 0 ? `${errors} errors` : undefined}
            />
          </>
        )}
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Spend over time</CardTitle>
          <CardDescription>
            {WINDOWS.find((w) => w.value === win)?.label} · USD
          </CardDescription>
        </CardHeader>
        <CardContent>
          {series.isLoading ? (
            <Skeleton height={140} />
          ) : spendPoints.length === 0 ? (
            <p className="py-10 text-center text-sm text-muted-foreground">
              No requests in this window.
            </p>
          ) : (
            <BarChart
              data={spendPoints}
              labels={spendLabels}
              height={140}
              highlightLast
              unit="$"
            />
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">By model</CardTitle>
          <CardDescription>Cost, latency and reliability per model</CardDescription>
        </CardHeader>
        <CardContent>
          {byModel.isLoading ? (
            <Skeleton height={96} />
          ) : (
            <Table
              rowKey="model"
              columns={[
                { key: "model", header: "Model", mono: true },
                {
                  key: "requests",
                  header: "Requests",
                  align: "right",
                  mono: true,
                  render: (v) => num(v as number).toLocaleString(),
                },
                {
                  key: "tokens",
                  header: "Tokens",
                  align: "right",
                  mono: true,
                  render: (v) => num(v as number).toLocaleString(),
                },
                {
                  key: "cost_usd",
                  header: "Cost",
                  align: "right",
                  mono: true,
                  render: (v) => money(num(v as number)),
                },
                {
                  key: "p95_latency_ms",
                  header: "p95",
                  align: "right",
                  mono: true,
                  render: (v) => Math.round(num(v as number)) + "ms",
                },
                {
                  key: "errors",
                  header: "Errors",
                  align: "right",
                  mono: true,
                  render: (v) => num(v as number).toLocaleString(),
                },
              ]}
              data={(byModel.data ?? []) as unknown as Record<string, unknown>[]}
            />
          )}
        </CardContent>
      </Card>
    </div>
  );
}
