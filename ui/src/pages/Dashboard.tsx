import { useQuery } from "@tanstack/react-query";

import { PageBody } from "@/components/screen";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Donut } from "@/components/ui/donut";
import { EmptyState } from "@/components/ui/empty-state";
import { LineChart } from "@/components/ui/line-chart";
import { Skeleton } from "@/components/ui/skeleton";
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

const num = (v: number | string | undefined): number => Number(v ?? 0);
const money = (n: number) =>
  "$" + n.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });

const WINDOW = { since: new Date(Date.now() - 86_400_000).toISOString(), bucket: "hour" };

const BAR_PALETTE = [
  "var(--red-folk)",
  "var(--zinc-400)",
  "var(--status-info)",
  "var(--status-success)",
];

function isUnavailable(err: unknown): boolean {
  return err instanceof AnalyticsUnavailableError;
}

// live overview from the design prototype: 4 KPIs, hourly spend line, traffic
// donut, requests-by-provider bars, and a recent-requests mini table. all
// clickhouse-backed; renders a calm not-configured state when analytics is off.
export default function Dashboard() {
  const summary = useQuery({
    queryKey: ["analytics", "summary", "24h"],
    queryFn: () => fetchAnalyticsSummary(WINDOW),
    retry: false,
  });
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

  const unavailable =
    isUnavailable(summary.error) || isUnavailable(series.error) || isUnavailable(byModel.error);

  if (unavailable) {
    return (
      <PageBody>
        <div className="rounded-lg border border-[color:var(--border-default)]">
          <EmptyState
            title="Analytics not configured"
            description="Traffic, spend, and latency appear here once requests flow through the gateway. Set clickhouse_url on the control plane to enable logging."
          />
        </div>
      </PageBody>
    );
  }

  const s = summary.data;
  const requests = num(s?.requests);
  const errors = num(s?.errors);
  const errorRate = requests > 0 ? (errors / requests) * 100 : 0;

  const spendPoints = (series.data ?? []).map((p) => num(p.cost_usd));
  const spendLabels = (series.data ?? []).map((p) => p.bucket.slice(11, 16) || p.bucket);

  const models = byModel.data ?? [];
  const traffic = models.map((m) => ({ label: m.model, value: num(m.requests) }));
  const totalReq = traffic.reduce((a, t) => a + t.value, 0);
  const fmtK = (v: number) => (v >= 1000 ? (v / 1000).toFixed(1) + "k" : String(v));

  const barMax = Math.max(1, ...models.map((m) => num(m.requests)));
  const bars = [...models]
    .sort((a, b) => num(b.requests) - num(a.requests))
    .slice(0, 6)
    .map((m, i) => ({
      label: m.model,
      value: num(m.requests),
      pct: (num(m.requests) / barMax) * 100,
      color: BAR_PALETTE[i] ?? "var(--status-warning)",
    }));

  const recentRows = (recent.data ?? []).map((r: InvocationRow) => ({
    id: r.request_id || r.ts,
    t: (r.ts ?? "").slice(11, 19),
    model: r.model,
    status: num(r.status),
    lat: Math.round(num(r.latency_ms)),
  }));

  return (
    <PageBody className="gap-[18px]">
      <div className="grid grid-cols-2 gap-3.5 xl:grid-cols-4">
        {summary.isLoading ? (
          Array.from({ length: 4 }).map((_, i) => <Skeleton key={i} height={104} radius={10} />)
        ) : (
          <>
            <StatCard label="Requests (24h)" value={requests.toLocaleString()} />
            <StatCard label="Spend (24h)" value={money(num(s?.cost_usd))} />
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

      <div className="grid gap-3.5 xl:grid-cols-[1.6fr_1fr]">
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              Last 24h
            </CardDescription>
            <CardTitle className="text-base">Spend</CardTitle>
            <CardDescription>Hourly gateway spend, USD</CardDescription>
          </CardHeader>
          <CardContent>
            {series.isLoading ? (
              <Skeleton height={220} />
            ) : spendPoints.length === 0 ? (
              <p className="py-16 text-center text-sm text-muted-foreground">
                No requests in this window.
              </p>
            ) : (
              <LineChart
                series={[{ name: "spend", values: spendPoints }]}
                labels={spendLabels}
                height={220}
                formatValue={(v) => money(v)}
              />
            )}
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              Last 24h
            </CardDescription>
            <CardTitle className="text-base">Traffic share</CardTitle>
            <CardDescription>Requests by model</CardDescription>
          </CardHeader>
          <CardContent>
            {byModel.isLoading ? (
              <Skeleton height={180} />
            ) : traffic.length === 0 ? (
              <p className="py-16 text-center text-sm text-muted-foreground">No traffic yet.</p>
            ) : (
              <Donut
                segments={traffic}
                size={150}
                centerLabel={fmtK(totalReq)}
                centerSub="requests"
              />
            )}
          </CardContent>
        </Card>
      </div>

      <div className="grid gap-3.5 xl:grid-cols-2">
        <Card>
          <CardHeader>
            <CardDescription className="text-[0.6875rem] uppercase tracking-[0.07em]">
              Last 24h
            </CardDescription>
            <CardTitle className="text-base">Requests by model</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-2 px-0.5 py-1">
            {bars.length === 0 && (
              <p className="py-10 text-center text-sm text-muted-foreground">No traffic yet.</p>
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
              Live
            </CardDescription>
            <CardTitle className="text-base">Recent requests</CardTitle>
          </CardHeader>
          <CardContent>
            {recent.isLoading ? (
              <Skeleton height={160} />
            ) : recentRows.length === 0 ? (
              <p className="py-10 text-center text-sm text-muted-foreground">
                Nothing logged yet.
              </p>
            ) : (
              <Table
                rowKey="id"
                columns={[
                  { key: "t", header: "Time", mono: true, width: "92px" },
                  { key: "model", header: "Model", mono: true },
                  {
                    key: "status",
                    header: "Status",
                    render: (v) => <StatusBadge status={v as number} />,
                  },
                  {
                    key: "lat",
                    header: "ms",
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
      ? ["var(--status-success)", "rgba(22,163,74,.14)"]
      : status === 429
        ? ["var(--status-warning)", "rgba(245,158,11,.14)"]
        : ["var(--status-danger)", "rgba(229,57,53,.14)"];
  return (
    <span
      className="inline-flex items-center rounded-[6px] px-[7px] py-0.5 font-mono text-[11px] font-semibold"
      style={{ color: tone[0], background: tone[1] }}
    >
      {status}
    </span>
  );
}
