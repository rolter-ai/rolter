import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { InvocationsPanel } from "@/components/InvocationsPanel";
import { LineChart } from "@/components/ui/line-chart";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Select } from "@/components/ui/select";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsByModel,
  fetchAnalyticsSummary,
  fetchAnalyticsTimeseries,
  type AnalyticsWindow,
} from "@/lib/api";

const RANGES = [
  { label: "Last 24h", hours: 24, bucket: "hour" },
  { label: "Last 7d", hours: 24 * 7, bucket: "day" },
  { label: "Last 30d", hours: 24 * 30, bucket: "day" },
] as const;

const BUCKETS = ["hour", "day", "week", "month"] as const;

function num(value: number | string | undefined): number {
  if (value === undefined) return 0;
  const n = Number(value);
  return Number.isFinite(n) ? n : 0;
}

function fmtInt(value: number | string | undefined): string {
  return num(value).toLocaleString();
}

function fmtUsd(value: number | string | undefined): string {
  return `$${num(value).toFixed(2)}`;
}

function fmtMs(value: number | string | undefined): string {
  return `${num(value).toFixed(0)} ms`;
}

/// true when `error` is the analytics-not-configured case (503), as opposed
/// to a real query failure (502) or network error
function isUnavailable(error: unknown): boolean {
  return error instanceof AnalyticsUnavailableError;
}

type Tab = "analytics" | "invocations";

export default function Logs() {
  const [tab, setTab] = React.useState<Tab>("analytics");
  const [rangeIdx, setRangeIdx] = React.useState(1);
  const [bucket, setBucket] = React.useState<string>(RANGES[1].bucket);

  const range = RANGES[rangeIdx];

  const window = React.useMemo<AnalyticsWindow>(() => {
    const since = new Date(Date.now() - range.hours * 60 * 60 * 1000).toISOString();
    return { since, bucket };
  }, [range.hours, bucket]);

  const summary = useQuery({
    queryKey: ["analytics-summary", window],
    queryFn: () => fetchAnalyticsSummary(window),
    retry: (failureCount, error) => !isUnavailable(error) && failureCount < 2,
  });

  const timeseries = useQuery({
    queryKey: ["analytics-timeseries", window],
    queryFn: () => fetchAnalyticsTimeseries(window),
    retry: (failureCount, error) => !isUnavailable(error) && failureCount < 2,
  });

  const byModel = useQuery({
    queryKey: ["analytics-by-model", window],
    queryFn: () => fetchAnalyticsByModel(window),
    retry: (failureCount, error) => !isUnavailable(error) && failureCount < 2,
  });

  const unavailable =
    isUnavailable(summary.error) ||
    isUnavailable(timeseries.error) ||
    isUnavailable(byModel.error);

  const hasRealError =
    !unavailable && (summary.isError || timeseries.isError || byModel.isError);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Logs &amp; cost</h1>
          <p className="text-sm text-muted-foreground">
            Usage, cost and latency rolled up from request logs.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <div className="flex gap-1">
            {RANGES.map((r, i) => (
              <button
                key={r.label}
                type="button"
                onClick={() => {
                  setRangeIdx(i);
                  setBucket(r.bucket);
                }}
                className={`rounded-md border px-2.5 py-1 text-xs ${
                  i === rangeIdx
                    ? "border-brand-folk bg-accent text-foreground"
                    : "border-border text-muted-foreground hover:text-foreground"
                }`}
              >
                {r.label}
              </button>
            ))}
          </div>
          {tab === "analytics" && (
            <Select
              className="w-28"
              value={bucket}
              onChange={(e) => setBucket(e.target.value)}
            >
              {BUCKETS.map((b) => (
                <option key={b} value={b}>
                  {b}
                </option>
              ))}
            </Select>
          )}
        </div>
      </div>

      <div className="flex gap-1 border-b border-border">
        {(
          [
            { id: "analytics", label: "Analytics" },
            { id: "invocations", label: "Invocations" },
          ] as const
        ).map((t) => (
          <button
            key={t.id}
            type="button"
            onClick={() => setTab(t.id)}
            className={`-mb-px border-b-2 px-3 py-1.5 text-sm ${
              tab === t.id
                ? "border-brand-folk font-medium text-foreground"
                : "border-transparent text-muted-foreground hover:text-foreground"
            }`}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === "analytics" && unavailable && (
        <Card>
          <CardContent className="py-6 text-sm text-muted-foreground">
            Analytics isn&apos;t configured for this deployment — set{" "}
            <code className="font-mono text-xs">clickhouse_url</code> on the control
            plane to enable the logs dashboard.
          </CardContent>
        </Card>
      )}

      {tab === "analytics" && hasRealError && (
        <p className="text-sm text-destructive">
          {(summary.error as Error | undefined)?.message ??
            (timeseries.error as Error | undefined)?.message ??
            (byModel.error as Error | undefined)?.message ??
            "Failed to load analytics."}
        </p>
      )}

      {tab === "invocations" && <InvocationsPanel window={window} />}

      {tab === "analytics" && !unavailable && (
        <>
          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-5">
            <StatCard
              label="Requests"
              value={summary.isLoading ? "…" : fmtInt(summary.data?.requests)}
            />
            <StatCard
              label="Cost"
              value={summary.isLoading ? "…" : fmtUsd(summary.data?.cost_usd)}
            />
            <StatCard
              label="Tokens"
              value={summary.isLoading ? "…" : fmtInt(summary.data?.tokens)}
            />
            <StatCard
              label="Errors"
              value={summary.isLoading ? "…" : fmtInt(summary.data?.errors)}
              tone={num(summary.data?.errors) > 0 ? "destructive" : undefined}
            />
            <StatCard
              label="Avg latency"
              value={summary.isLoading ? "…" : fmtMs(summary.data?.avg_latency_ms)}
            />
          </div>

          <Card>
            <CardHeader>
              <CardTitle>Requests &amp; cost over time</CardTitle>
              <CardDescription>
                Bucketed by {bucket}, {timeseries.data?.length ?? 0} points.
              </CardDescription>
            </CardHeader>
            <CardContent>
              {timeseries.isLoading && (
                <p className="text-sm text-muted-foreground">Loading…</p>
              )}
              {!timeseries.isLoading && (timeseries.data?.length ?? 0) === 0 && (
                <p className="text-sm text-muted-foreground">
                  No requests in this window.
                </p>
              )}
              {(timeseries.data?.length ?? 0) > 0 && (
                <div className="h-44">
                  <LineChart
                    labels={timeseries.data!.map((p) => p.bucket)}
                    series={[
                      {
                        name: "requests",
                        values: timeseries.data!.map((p) => num(p.requests)),
                        color: "var(--red-folk)",
                      },
                    ]}
                  />
                </div>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>By model</CardTitle>
              <CardDescription>Cost, error rate and latency per model.</CardDescription>
            </CardHeader>
            <CardContent>
              {byModel.isLoading && (
                <p className="text-sm text-muted-foreground">Loading…</p>
              )}
              {!byModel.isLoading && (byModel.data?.length ?? 0) === 0 && (
                <p className="text-sm text-muted-foreground">
                  No requests in this window.
                </p>
              )}
              {(byModel.data?.length ?? 0) > 0 && (
                <div className="overflow-x-auto">
                  <table className="w-full text-left text-sm">
                    <thead>
                      <tr className="border-b border-border text-xs text-muted-foreground">
                        <th className="py-1.5 pr-4 font-medium">Model</th>
                        <th className="py-1.5 pr-4 font-medium">Requests</th>
                        <th className="py-1.5 pr-4 font-medium">Cost</th>
                        <th className="py-1.5 pr-4 font-medium">Error rate</th>
                        <th className="py-1.5 pr-4 font-medium">p50</th>
                        <th className="py-1.5 pr-4 font-medium">p95</th>
                      </tr>
                    </thead>
                    <tbody>
                      {byModel.data!.map((row) => {
                        const requests = num(row.requests);
                        const errors = num(row.errors);
                        const errorRate = requests > 0 ? (errors / requests) * 100 : 0;
                        return (
                          <tr key={row.model} className="border-b border-border/50">
                            <td className="py-1.5 pr-4 font-mono text-xs">
                              {row.model}
                            </td>
                            <td className="py-1.5 pr-4">{fmtInt(row.requests)}</td>
                            <td className="py-1.5 pr-4">{fmtUsd(row.cost_usd)}</td>
                            <td
                              className={`py-1.5 pr-4 ${
                                errorRate > 0 ? "text-destructive" : ""
                              }`}
                            >
                              {errorRate.toFixed(1)}%
                            </td>
                            <td className="py-1.5 pr-4">{fmtMs(row.p50_latency_ms)}</td>
                            <td className="py-1.5 pr-4">{fmtMs(row.p95_latency_ms)}</td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )}
            </CardContent>
          </Card>
        </>
      )}
    </div>
  );
}

function StatCard({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: "destructive";
}) {
  return (
    <Card>
      <CardContent className="p-4">
        <p className="text-xs text-muted-foreground">{label}</p>
        <p
          className={`mt-1 font-mono text-xl font-semibold ${
            tone === "destructive" ? "text-destructive" : ""
          }`}
        >
          {value}
        </p>
      </CardContent>
    </Card>
  );
}
