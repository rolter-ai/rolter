import { useQuery } from "@tanstack/react-query";

import { PageBody } from "@/components/screen";
import {
  fetchHealthTimeline,
  fetchMttr,
  fetchUptime,
  type TimelineRow,
} from "@/lib/api";
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
    <PageBody className="gap-[18px]">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          Per-target circuit breakers, uptime, and error-budget burn — last 7 days, SLA target{" "}
          {pct(SLA)}
        </span>
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
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(340px,1fr))]">
        {uptime.data?.map((row) => {
          const key = `${row.provider}::${row.target_id}`;
          const breached = row.sla_breached === 1;
          const m = mttrByTarget.get(key);
          const dot = breached ? "var(--status-danger)" : "var(--status-success)";
          return (
            <div
              key={key}
              className="flex flex-col gap-3.5 rounded-[10px] border bg-card p-4"
              style={{
                borderColor: breached
                  ? "color-mix(in srgb, var(--status-danger) 45%, transparent)"
                  : "var(--border-default)",
              }}
            >
              <div className="flex items-center gap-2.5">
                <span className="h-2 w-2 flex-none rounded-full" style={{ background: dot }} />
                <span className="font-mono text-sm font-semibold">{row.target_id}</span>
                <span className="font-mono text-xs text-[color:var(--text-subtle)]">
                  {row.provider}
                </span>
                <span
                  className="ml-auto rounded-[6px] px-2 py-[3px] font-mono text-[0.6875rem] uppercase tracking-[0.05em]"
                  style={{
                    color: dot,
                    background: breached ? "rgba(229,57,53,.14)" : "rgba(22,163,74,.14)",
                  }}
                >
                  {breached ? "tripped" : "closed"}
                </span>
              </div>
              <div className="flex items-end gap-3">
                <div className="flex flex-col gap-px">
                  <span
                    className={cn(
                      "font-mono text-2xl font-medium leading-none",
                      breached && "text-destructive",
                    )}
                  >
                    {pct(row.uptime)}
                  </span>
                  <span className="text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
                    uptime · {row.events} events
                  </span>
                </div>
                <div className="min-w-0 flex-1">
                  <Timeline buckets={timelineByTarget.get(key) ?? []} />
                </div>
              </div>
              <div className="grid grid-cols-3 gap-2.5 border-t border-[color:var(--border-subtle)] pt-3">
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    Failures
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {row.errors + row.timeouts}
                  </div>
                </div>
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    MTTR
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {mttrLabel(m?.mttr_seconds)}
                  </div>
                </div>
                <div>
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    Incidents
                  </div>
                  <div className="font-mono text-sm text-[color:var(--text-secondary)]">
                    {m?.incidents ?? "—"}
                  </div>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <span className="text-xs text-muted-foreground">error budget burn</span>
                <span
                  className="ml-auto font-mono text-xs"
                  style={{
                    color:
                      row.error_budget_burn > 1
                        ? "var(--status-danger)"
                        : "var(--text-secondary)",
                  }}
                >
                  {(row.error_budget_burn * 100).toFixed(0)}%
                </span>
              </div>
            </div>
          );
        })}
      </div>
    </PageBody>
  );
}
