import { useQuery } from "@tanstack/react-query";
import { HeartPulse } from "lucide-react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { EmptyState, EmptyStateLink } from "@/components/ui/empty-state";
import {
  fetchHealthTimeline,
  fetchMttr,
  fetchUptime,
  type MttrRow,
  type TimelineRow,
  type UptimeRow,
} from "@/lib/api";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { HEALTH_SLA as SLA } from "@/lib/route-targets";
import { cn } from "@/lib/utils";
import { useScreenReady } from "@/lib/ux-react";

/**
 * How far back "recently" reaches: the newest hourly bucket the timeline holds
 * and the one before it, so a state never hangs on the few minutes an hour has
 * had so far.
 */
const RECENT_MS = 60 * 60 * 1000;

/**
 * Where a provider or target stands against the SLA. Every input comes from
 * `provider_health_events`, not from the gateway's circuit breakers, which this
 * screen never reads — so the states are named after the SLA, never after a
 * breaker (#2113).
 *
 * - `breaching`: over the window, more requests failed than the SLA allows
 * - `atRisk`: the window still meets the SLA, but the recent buckets failed
 *   faster than it allows, so the error budget is burning at more than 1x
 * - `within`: the window meets the SLA and the recent buckets do too
 */
type SlaState = "breaching" | "atRisk" | "within";

/** the status fill for a state's dots and border */
const SLA_FILL: Record<SlaState, string> = {
  breaching: "var(--status-danger)",
  atRisk: "var(--status-warning)",
  within: "var(--status-success)",
};

const SLA_TONE = { breaching: "danger", atRisk: "warning", within: "success" } as const;

function slaState(breached: boolean, recentBurn: number | undefined): SlaState {
  if (breached) return "breaching";
  return recentBurn !== undefined && recentBurn > 1 ? "atRisk" : "within";
}

const SLA_RANK: Record<SlaState, number> = { within: 0, atRisk: 1, breaching: 2 };

/** the worst of several states, for a card rolled up from its targets */
function worstState(states: SlaState[]): SlaState {
  return states.reduce<SlaState>((a, b) => (SLA_RANK[b] > SLA_RANK[a] ? b : a), "within");
}

function useSlaLabel(): (state: SlaState) => string {
  const { t } = useTranslation();
  return (state) =>
    state === "breaching"
      ? t("pages.health.slaBreaching")
      : state === "atRisk"
        ? t("pages.health.slaAtRisk")
        : t("pages.health.slaWithin");
}

/**
 * A bucket's start in milliseconds. ClickHouse writes `2026-08-06 10:00:00`
 * with no zone, in its own; every bucket shares that zone, so reading them all
 * as UTC keeps the gaps between them right, which is all `recentBurn` compares.
 */
function bucketStart(bucket: string): number {
  const iso = bucket.replace(" ", "T");
  return Date.parse(/(Z|[+-]\d{2}:?\d{2})$/i.test(iso) ? iso : `${iso}Z`);
}

/**
 * Failures over the buckets that start at or after `since`, as a multiple of
 * what the SLA allows — the same formula as the rollup's `error_budget_burn`,
 * over a shorter span. Undefined when nothing was observed in that span.
 */
function recentBurn(buckets: TimelineRow[], since: number): number | undefined {
  let events = 0;
  let failures = 0;
  for (const b of buckets) {
    // `!(>=)` rather than `<`, so an unparsable bucket is skipped too
    if (!(bucketStart(b.bucket) >= since)) continue;
    events += b.events;
    failures += b.errors + b.timeouts;
  }
  return events === 0 ? undefined : failures / events / (1 - SLA);
}

function pct(fmt: Formatters, v: number): string {
  return fmt.percent(v, 2);
}

function mttrLabel(fmt: Formatters, seconds: number | undefined): string {
  return seconds === undefined ? "—" : fmt.duration(seconds);
}

/**
 * The two grains the gateway observes a provider at, as returned by the
 * rollups. `provider` rows come from probes and status-page polls and describe
 * the provider as a whole; `target` rows come from real traffic and describe
 * one route through it. Before #1257 the screen laid both out as peer cards,
 * so one dead provider rendered as several unrelated cards with contradictory
 * counts. They are never summed together — a probe every interval and a
 * per-request observation count different things — so a provider's target rows
 * are nested inside its own card instead.
 */
type Grain = "provider" | "target";

interface ProviderGroup {
  provider: string;
  /** the provider-grain row, when probes or a status page feed this provider */
  overall: UptimeRow | undefined;
  targets: UptimeRow[];
}

interface Headline {
  uptime: number;
  events: number;
  failures: number;
  burn: number;
  breached: boolean;
  /** true when there is no provider-grain row and the numbers are a roll-up */
  derived: boolean;
  sources: string[];
}

/**
 * The card's top-line numbers. A provider-grain row is used verbatim when one
 * exists. Without one, the target rows are disjoint views of the same provider
 * — every request went through exactly one of them — so summing them is the
 * honest roll-up; `derived` says so and the card labels it.
 */
function headline(group: ProviderGroup): Headline {
  const row = group.overall;
  if (row) {
    return {
      uptime: row.uptime,
      events: row.events,
      failures: row.errors + row.timeouts,
      burn: row.error_budget_burn,
      breached: row.sla_breached === 1,
      derived: false,
      sources: row.sources ?? [],
    };
  }
  let events = 0;
  let ok = 0;
  let failures = 0;
  let burn = 0;
  const sources = new Set<string>();
  for (const t of group.targets) {
    events += t.events;
    ok += t.ok;
    failures += t.errors + t.timeouts;
    // the budget is per target, so the card reports the worst one rather than
    // an average that would hide it
    burn = Math.max(burn, t.error_budget_burn);
    for (const s of t.sources ?? []) sources.add(s);
  }
  return {
    uptime: events === 0 ? 1 : ok / events,
    events,
    failures,
    burn,
    breached: group.targets.some((t) => t.sla_breached === 1),
    derived: true,
    sources: [...sources].sort(),
  };
}

function groupByProvider(rows: UptimeRow[]): ProviderGroup[] {
  const byProvider = new Map<string, ProviderGroup>();
  for (const row of rows) {
    let group = byProvider.get(row.provider);
    if (!group) {
      group = { provider: row.provider, overall: undefined, targets: [] };
      byProvider.set(row.provider, group);
    }
    // fall back on the id comparison the server derives `grain` from, so a
    // control plane that predates the column still groups correctly
    const grain: Grain = (row.grain ??
      (row.target_id === row.provider ? "provider" : "target")) as Grain;
    if (grain === "provider") group.overall = row;
    else group.targets.push(row);
  }
  const groups = [...byProvider.values()];
  for (const group of groups) group.targets.sort((a, b) => a.uptime - b.uptime);
  // worst first, the same intent as the server's `order by uptime asc`
  groups.sort((a, b) => headline(a).uptime - headline(b).uptime);
  return groups;
}

/** Merge timeline rows that share a bucket, for a card's roll-up strip. */
function mergeBuckets(rows: TimelineRow[]): TimelineRow[] {
  const byBucket = new Map<string, TimelineRow>();
  for (const row of rows) {
    const seen = byBucket.get(row.bucket);
    if (!seen) {
      byBucket.set(row.bucket, { ...row });
      continue;
    }
    seen.events += row.events;
    seen.ok += row.ok;
    seen.errors += row.errors;
    seen.timeouts += row.timeouts;
  }
  return [...byBucket.values()].sort((a, b) => a.bucket.localeCompare(b.bucket));
}

// one thin bar per time bucket: red if any failure landed in it, else green
function Timeline({ buckets, className }: { buckets: TimelineRow[]; className?: string }) {
  const { t } = useTranslation();
  if (buckets.length === 0) {
    return <span className="text-xs text-muted-foreground">{t("pages.health.noEvents")}</span>;
  }
  return (
    <div className={cn("flex items-end gap-px", className ?? "h-8")}>
      {buckets.map((b) => {
        const bad = b.errors + b.timeouts;
        const down = bad > 0;
        return (
          <div
            key={b.bucket}
            title={t("pages.health.bucketTitle", {
              bucket: b.bucket,
              ok: b.ok,
              errors: b.errors,
              timeouts: b.timeouts,
            })}
            className={cn(
              "w-1.5 flex-1 rounded-sm",
              down ? "bg-destructive" : "bg-[color:var(--status-success)]/70",
            )}
            style={{ height: down ? "100%" : "40%" }}
          />
        );
      })}
    </div>
  );
}

function SlaPill({ state }: { state: SlaState }) {
  const label = useSlaLabel();
  return (
    <Badge tone={SLA_TONE[state]} className="ml-auto" data-testid="health-sla-state">
      {label(state)}
    </Badge>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="font-mono text-sm text-[color:var(--text-secondary)]">{value}</div>
    </div>
  );
}

/** One route through the provider, nested inside the provider's card. */
function TargetRow({
  row,
  mttr,
  buckets,
  state,
}: {
  row: UptimeRow;
  mttr: MttrRow | undefined;
  buckets: TimelineRow[];
  state: SlaState;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const label = useSlaLabel();
  return (
    <div
      className="flex flex-wrap items-center gap-x-3 gap-y-1.5 py-2"
      data-testid={`health-target-${row.target_id}`}
    >
      <span
        className="h-1.5 w-1.5 flex-none rounded-full"
        style={{ background: SLA_FILL[state] }}
      />
      {/* the dot is the row's only state marker, so its meaning is also text */}
      <span className="sr-only">{label(state)}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-xs">{row.target_id}</span>
      <Timeline buckets={buckets} className="h-4 w-20 flex-none" />
      <span
        className={cn(
          "w-16 flex-none text-right font-mono text-xs",
          state === "breaching" && "text-[color:var(--status-danger-text)]",
        )}
      >
        {pct(fmt, row.uptime)}
      </span>
      <span className="w-24 flex-none text-right font-mono text-xs text-[color:var(--text-subtle)]">
        {t("pages.health.failuresShort", { count: row.errors + row.timeouts })}
      </span>
      <span className="w-20 flex-none text-right font-mono text-xs text-[color:var(--text-subtle)]">
        {mttrLabel(fmt, mttr?.mttr_seconds)}
      </span>
    </div>
  );
}

export default function Health() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const uptime = useQuery({
    queryKey: ["health-uptime", SLA],
    queryFn: () => fetchUptime(SLA),
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `uptime` is the query the user is actually waiting on for this screen
  useScreenReady(!uptime.isLoading);
  const mttr = useQuery({ queryKey: ["health-mttr"], queryFn: fetchMttr });
  const timeline = useQuery({
    queryKey: ["health-timeline"],
    queryFn: () => fetchHealthTimeline("hour"),
  });

  const isLoading = uptime.isLoading || mttr.isLoading || timeline.isLoading;
  const error = uptime.error || mttr.error || timeline.error;

  const key = (provider: string, target: string) => `${provider}::${target}`;
  const mttrByTarget = new Map((mttr.data ?? []).map((m) => [key(m.provider, m.target_id), m]));
  const timelineByTarget = new Map<string, TimelineRow[]>();
  for (const row of timeline.data ?? []) {
    const k = key(row.provider, row.target_id);
    const list = timelineByTarget.get(k) ?? [];
    list.push(row);
    timelineByTarget.set(k, list);
  }
  const groups = groupByProvider(uptime.data ?? []);
  // "recently" counts back from the newest bucket the timeline holds, not from
  // the browser's clock, which cannot know the zone the buckets were written
  // in. while probes or traffic flow, that bucket is the current hour
  const newest = (timeline.data ?? []).reduce((max, row) => {
    const start = bucketStart(row.bucket);
    return start > max ? start : max;
  }, -Infinity);
  const since = newest - RECENT_MS;

  return (
    <PageBody className="gap-[18px]">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {t("pages.health.subtitle", { sla: pct(fmt, SLA) })}
        </span>
      </div>
      {isLoading && <CardGridSkeleton cards={4} height={218} min={340} />}
      {error && (
        <LoadError
          error={error}
          resource={t("errors.resources.healthRollups")}
          onRetry={() => {
            uptime.refetch();
            mttr.refetch();
            timeline.refetch();
          }}
          target="health-rollups"
        />
      )}
      {!isLoading && !error && groups.length === 0 && (
        // health rollups are a by-product of traffic, so there is nothing to
        // create here — the honest CTA is the one that produces an event
        <EmptyState
          uxTarget="health-rollups"
          icon={<HeartPulse />}
          title={t("pages.health.emptyTitle")}
          description={t("pages.health.emptyBody")}
          actions={
            <EmptyStateLink to="/playground">{t("pages.health.emptyAction")}</EmptyStateLink>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(340px,100%),1fr))]">
        {groups.map((group) => {
          const head = headline(group);
          const providerMttr =
            mttrByTarget.get(key(group.provider, group.provider)) ??
            (group.overall
              ? undefined
              : // no provider-grain incidents to report: fall back to the worst
                // target's, which is the one an operator is chasing
                [...group.targets]
                  .map((tgt) => mttrByTarget.get(key(group.provider, tgt.target_id)))
                  .filter((m): m is MttrRow => !!m)
                  .sort((a, b) => b.mttr_seconds - a.mttr_seconds)[0]);
          const headBuckets = group.overall
            ? (timelineByTarget.get(key(group.provider, group.provider)) ?? [])
            : mergeBuckets(
                group.targets.flatMap(
                  (tgt) => timelineByTarget.get(key(group.provider, tgt.target_id)) ?? [],
                ),
              );
          const targetState = (row: UptimeRow) =>
            slaState(
              row.sla_breached === 1,
              recentBurn(timelineByTarget.get(key(group.provider, row.target_id)) ?? [], since),
            );
          // a probed provider is judged on its own row, like its headline; a
          // rolled-up card takes its worst target, as `headline` does for a breach
          const state = group.overall
            ? slaState(head.breached, recentBurn(headBuckets, since))
            : worstState(group.targets.map(targetState));
          return (
            <div
              key={group.provider}
              data-testid={`health-card-${group.provider}`}
              className="flex flex-col gap-3.5 rounded-[10px] border bg-card p-4"
              style={{
                borderColor:
                  state === "within"
                    ? "var(--border-default)"
                    : `color-mix(in srgb, ${SLA_FILL[state]} 45%, transparent)`,
              }}
            >
              <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5">
                <span
                  className="h-2 w-2 flex-none rounded-full"
                  style={{ background: SLA_FILL[state] }}
                />
                <span className="font-mono text-sm font-semibold">{group.provider}</span>
                <span className="font-mono text-xs text-[color:var(--text-subtle)]">
                  {head.derived
                    ? t("pages.health.grainDerived")
                    : t("pages.health.grainProvider", {
                        sources: head.sources.join(", ") || "—",
                      })}
                </span>
                <SlaPill state={state} />
              </div>
              <div className="flex items-end gap-3">
                <div className="flex flex-col gap-px">
                  <span
                    className={cn(
                      "font-mono text-2xl font-medium leading-none",
                      // the figure is the window's uptime, which an at-risk
                      // provider still holds, so only a breach colours it
                      state === "breaching" && "text-[color:var(--status-danger-text)]",
                    )}
                  >
                    {pct(fmt, head.uptime)}
                  </span>
                  <span className="text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
                    {t("pages.health.uptimeMeta", { events: fmt.number(head.events) })}
                  </span>
                </div>
                <div className="min-w-0 flex-1">
                  <Timeline buckets={headBuckets} />
                </div>
              </div>
              <div className="grid grid-cols-1 gap-2.5 border-t sm:grid-cols-3 border-[color:var(--border-subtle)] pt-3">
                <Stat label={t("pages.health.failures")} value={fmt.number(head.failures)} />
                <Stat
                  label={t("pages.health.mttr")}
                  value={mttrLabel(fmt, providerMttr?.mttr_seconds)}
                />
                <Stat
                  label={t("pages.health.incidents")}
                  value={providerMttr ? fmt.number(providerMttr.incidents) : "—"}
                />
              </div>
              {group.targets.length > 0 && (
                <div className="border-t border-[color:var(--border-subtle)] pt-2">
                  <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
                    {t("pages.health.targets", { count: group.targets.length })}
                  </div>
                  <div className="divide-y divide-[color:var(--border-subtle)]">
                    {group.targets.map((row) => (
                      <TargetRow
                        key={row.target_id}
                        row={row}
                        mttr={mttrByTarget.get(key(group.provider, row.target_id))}
                        buckets={timelineByTarget.get(key(group.provider, row.target_id)) ?? []}
                        state={targetState(row)}
                      />
                    ))}
                  </div>
                </div>
              )}
              <div className="flex items-center gap-2">
                <span className="text-xs text-muted-foreground">
                  {t("pages.health.errorBudget")}
                </span>
                <span
                  className="ml-auto font-mono text-xs"
                  style={{
                    color: head.burn > 1 ? "var(--status-danger-text)" : "var(--text-secondary)",
                  }}
                >
                  {fmt.percent(head.burn, 0)}
                </span>
              </div>
            </div>
          );
        })}
      </div>
    </PageBody>
  );
}
