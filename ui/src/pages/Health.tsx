import { useQuery } from "@tanstack/react-query";

import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  fetchHealthTimeline,
  fetchMttr,
  fetchUptime,
  type TimelineRow,
} from "@/lib/api";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

const SLA = 0.99;

function pct(v: number): string {
  return `${(v * 100).toFixed(2)}%`;
}

function mttrLabel(seconds: number | undefined): string {
  if (seconds === undefined) return "—";
  if (seconds < 60) return `${Math.round(seconds)}s`;
  if (seconds < 3600) return `${(seconds / 60).toFixed(1)}m`;
  return `${(seconds / 3600).toFixed(1)}h`;
}

// one thin bar per time bucket: red if any failure landed in it, else green
function Timeline({ buckets }: { buckets: TimelineRow[] }) {
  if (buckets.length === 0) {
    return <p className="text-xs text-muted-foreground">No events in window.</p>;
  }
  return (
    <div className="flex h-8 items-end gap-px">
      {buckets.map((b) => {
        const bad = b.errors + b.timeouts;
        const down = bad > 0;
        return (
          <div
            key={b.bucket}
            title={`${b.bucket}: ${b.ok} ok, ${b.errors} error, ${b.timeouts} timeout`}
            className={cn(
              "w-1.5 flex-1 rounded-sm",
              down ? "bg-destructive" : "bg-emerald-500/70",
            )}
            style={{ height: down ? "100%" : "40%" }}
          />
        );
      })}
    </div>
  );
}

export default function Health() {
  const uptime = useQuery({
    queryKey: ["health-uptime", SLA],
    queryFn: () => fetchUptime(SLA),
  });
  const mttr = useQuery({ queryKey: ["health-mttr"], queryFn: fetchMttr });
  const timeline = useQuery({
    queryKey: ["health-timeline"],
    queryFn: () => fetchHealthTimeline("hour"),
  });

  const isLoading = uptime.isLoading || mttr.isLoading || timeline.isLoading;
  const error = uptime.error || mttr.error || timeline.error;

  const mttrByTarget = new Map(
    (mttr.data ?? []).map((m) => [`${m.provider}::${m.target_id}`, m]),
  );
  const timelineByTarget = new Map<string, TimelineRow[]>();
  for (const row of timeline.data ?? []) {
    const key = `${row.provider}::${row.target_id}`;
    const list = timelineByTarget.get(key) ?? [];
    list.push(row);
    timelineByTarget.set(key, list);
  }

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold">Provider health</h1>
        <p className="text-sm text-muted-foreground">
          Uptime, MTTR and failure timeline per target over the last 7 days (SLA
          target {pct(SLA)}).
        </p>
      </div>
      {isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {error && (
        <p className="text-sm text-destructive">
          Failed to load health rollups — is ClickHouse configured?
        </p>
      )}
      {!isLoading && !error && (uptime.data?.length ?? 0) === 0 && (
        <p className="text-sm text-muted-foreground">
          No health events recorded yet.
        </p>
      )}
      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {uptime.data?.map((row) => {
          const key = `${row.provider}::${row.target_id}`;
          const breached = row.sla_breached === 1;
          const m = mttrByTarget.get(key);
          return (
            <Card key={key}>
              <CardHeader>
                <div className="flex items-start justify-between gap-2">
                  <div>
                    <CardTitle className="text-base">{row.target_id}</CardTitle>
                    <CardDescription>{row.provider}</CardDescription>
                  </div>
                  <Badge tone={breached ? "danger" : "success"} dot>
                    {breached ? "SLA breached" : "Healthy"}
                  </Badge>
                </div>
              </CardHeader>
              <CardContent className="space-y-3 text-sm">
                <div className="flex items-baseline justify-between">
                  <span
                    className={cn(
                      "text-2xl font-semibold",
                      breached ? "text-destructive" : "",
                    )}
                  >
                    {pct(row.uptime)}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    uptime · {row.events} events
                  </span>
                </div>
                <Timeline buckets={timelineByTarget.get(key) ?? []} />
                <div className="grid grid-cols-3 gap-2 text-xs text-muted-foreground">
                  <div>
                    <div className="font-medium text-foreground">
                      {row.errors + row.timeouts}
                    </div>
                    failures
                  </div>
                  <div>
                    <div className="font-medium text-foreground">
                      {mttrLabel(m?.mttr_seconds)}
                    </div>
                    MTTR{m ? ` · ${m.incidents}×` : ""}
                  </div>
                  <div>
                    <div className="font-medium text-foreground">
                      {(row.error_budget_burn * 100).toFixed(0)}%
                    </div>
                    budget burn
                  </div>
                </div>
              </CardContent>
            </Card>
          );
        })}
      </div>
    </div>
  );
}
