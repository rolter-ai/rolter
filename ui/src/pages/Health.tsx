import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, HeartPulse } from "lucide-react";
import { useTranslation } from "react-i18next";

import { BreakerBadge } from "@/components/BreakerBadge";
import { OwnerOrgPill, sharedNames } from "@/components/OwnerOrgPill";
import { Overline } from "@/components/ui/overline";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { EmptyState, EmptyStateLink } from "@/components/ui/empty-state";
import {
  fetchHealthTimeline,
  fetchMttr,
  fetchUptime,
  isTripped,
  useBreakerState,
  type BreakerStateDto,
  type BreakerTargetDto,
  type MttrRow,
  type TimelineRow,
  type UptimeRow,
} from "@/lib/api";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import {
  HEALTH_SLA as SLA,
  bucketStart,
  recentBurn,
  recentSince,
  slaState,
  worstState,
  type SlaState,
} from "@/lib/route-targets";
import { cn } from "@/lib/utils";
import {
  eventAt,
  groupByProvider,
  headline,
  providerKey,
  type ProviderGroup,
} from "@/lib/provider-health";
import { useNow } from "@/lib/use-now";
import { useScreenReady } from "@/lib/ux-react";

// the rollups are hourly buckets over a 7-day window, so a minute-level poll
// buys nothing; the interval pauses in a hidden tab (react-query's default)
const REFETCH_MS = 30_000;
// a provider that has reported nothing for this long is quiet enough to say so:
// several probe sweeps, and long enough that a lull in passive traffic is not
const STALE_AFTER_MS = 15 * 60 * 1000;

/** the status fill for a state's dots and border */
const SLA_FILL: Record<SlaState, string> = {
  breaching: "var(--status-danger)",
  atRisk: "var(--status-warning)",
  within: "var(--status-success)",
};

const SLA_TONE = { breaching: "danger", atRisk: "warning", within: "success" } as const;

function useSlaLabel(): (state: SlaState) => string {
  const { t } = useTranslation();
  return (state) =>
    state === "breaching"
      ? t("pages.health.slaBreaching")
      : state === "atRisk"
        ? t("pages.health.slaAtRisk")
        : t("pages.health.slaWithin");
}

function useSourceLabel(): (source: string) => string {
  const { t } = useTranslation();
  return (source) =>
    source === "probe"
      ? t("pages.health.sources.probe")
      : source === "status_page"
        ? t("pages.health.sources.status_page")
        : source === "passive"
          ? t("pages.health.sources.passive")
          : // a source this build does not know is shown as the control plane sent it
            source;
}

function pct(fmt: Formatters, v: number | undefined): string {
  return v === undefined ? "—" : fmt.percent(v, 2);
}

function mttrLabel(fmt: Formatters, seconds: number | undefined): string {
  return seconds === undefined ? "—" : fmt.duration(seconds);
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

/** a failing bucket never drops below this height, so one failure stays visible */
const FAIL_MIN_HEIGHT = 55;
/** the failure share at which a bucket's bar reaches full height */
const FAIL_FULL_SHARE = 0.1;

/** a bucket's bar height in percent: a clean bucket is short, a failing one grows with its share */
function barHeight(b: TimelineRow): number {
  const bad = b.errors + b.timeouts;
  if (bad === 0) return 40;
  const share = b.events > 0 ? bad / b.events : 1;
  return FAIL_MIN_HEIGHT + (100 - FAIL_MIN_HEIGHT) * Math.min(1, share / FAIL_FULL_SHARE);
}

/**
 * One thin bar per time bucket: green when clean, `--status-danger` and taller
 * as the failure share grows. The bars are decoration for sighted users; the
 * range and every bucket's counts are also text, for assistive tech. `detail`
 * adds the time axis and legend, which a row's small strip has no room for.
 */
function Timeline({
  buckets,
  className,
  detail = false,
}: {
  buckets: TimelineRow[];
  className?: string;
  detail?: boolean;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (buckets.length === 0) {
    return <span className="text-xs text-muted-foreground">{t("pages.health.noTimeline")}</span>;
  }
  const at = (b: TimelineRow) => fmt.dateTime(new Date(bucketStart(b.bucket)));
  const first = at(buckets[0]);
  const last = at(buckets[buckets.length - 1]);
  const events = buckets.reduce((n, b) => n + b.events, 0);
  const failures = buckets.reduce((n, b) => n + b.errors + b.timeouts, 0);
  return (
    <div className="flex flex-col gap-1">
      <div
        role="group"
        aria-label={t("pages.health.timelineSummary", {
          from: first,
          to: last,
          buckets: buckets.length,
          events: fmt.number(events),
          failures: fmt.number(failures),
        })}
        data-testid="health-timeline"
        className={cn("flex items-end gap-px", className ?? "h-8")}
      >
        {buckets.map((b) => {
          const down = b.errors + b.timeouts > 0;
          return (
            <div
              key={b.bucket}
              aria-hidden="true"
              title={t("pages.health.bucketTitle", {
                bucket: at(b),
                ok: b.ok,
                errors: b.errors,
                timeouts: b.timeouts,
              })}
              data-down={down || undefined}
              className="w-1.5 flex-1 rounded-sm"
              style={{
                height: `${barHeight(b)}%`,
                backgroundColor: down
                  ? "var(--status-danger)"
                  : "color-mix(in srgb, var(--status-success) 70%, transparent)",
              }}
            />
          );
        })}
        <ul className="sr-only">
          {buckets.map((b) => (
            <li key={b.bucket}>
              {t("pages.health.bucketTitle", {
                bucket: at(b),
                ok: b.ok,
                errors: b.errors,
                timeouts: b.timeouts,
              })}
            </li>
          ))}
        </ul>
      </div>
      {detail && (
        <>
          <div
            aria-hidden="true"
            className="flex flex-wrap justify-between gap-x-2 font-mono text-2xs text-[color:var(--text-subtle)]"
          >
            <span>{first}</span>
            {buckets.length > 1 && <span className="ml-auto text-right">{last}</span>}
          </div>
          <div
            aria-hidden="true"
            className="flex flex-wrap gap-x-3 text-2xs text-[color:var(--text-subtle)]"
          >
            <span className="inline-flex items-center gap-1">
              <span
                className="h-1.5 w-1.5 rounded-sm"
                style={{ background: "var(--status-success)" }}
              />
              {t("pages.health.legendOk")}
            </span>
            <span className="inline-flex items-center gap-1">
              <span
                className="h-1.5 w-1.5 rounded-sm"
                style={{ background: "var(--status-danger)" }}
              />
              {t("pages.health.legendFailed")}
            </span>
          </div>
        </>
      )}
    </div>
  );
}

/**
 * When a provider last reported, relative to the screen's clock. The uptime
 * figure is a 7-day window, so it says nothing about whether the provider is
 * still reporting; this does. Past `STALE_AFTER_MS` it takes the warning tone
 * and different words, so the state is not carried by colour alone.
 */
function LastEvent({ at, now }: { at: number | undefined; now: number }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (at === undefined) {
    return <span className="text-xs text-muted-foreground">{t("pages.health.noEvents")}</span>;
  }
  const stale = now - at > STALE_AFTER_MS;
  const when = fmt.relative(at, now);
  return (
    <span
      data-testid="health-last-event"
      data-stale={stale || undefined}
      title={fmt.dateTime(at)}
      className={cn(
        "inline-flex items-center gap-1 text-xs",
        stale ? "text-[color:var(--status-warning-text)]" : "text-muted-foreground",
      )}
    >
      {stale && <AlertTriangle aria-hidden className="h-3 w-3 flex-none" />}
      {t(stale ? "pages.health.lastEventStale" : "pages.health.lastEvent", { when })}
    </span>
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

interface ProviderBreakers {
  /**
   * the organization of the routes whose targets these are, `""` for a
   * config-file route: two organizations' routes may name providers of one
   * name, and each organization's are listed apart (#3460)
   */
  org_id: string;
  provider: string;
  /** the tripped targets, open ones first */
  targets: BreakerTargetDto[];
  /** the worst of them, as a breaker the badge can draw */
  worst: BreakerTargetDto;
}

/**
 * the tripped breakers grouped by the provider they sit in front of and the
 * organization of their routes, open ones first, keyed by {@link providerKey}
 */
function breakersByProvider(state: BreakerStateDto | undefined): Map<string, ProviderBreakers> {
  const byProvider = new Map<string, ProviderBreakers>();
  for (const tg of state?.targets ?? []) {
    if (!isTripped(tg)) continue;
    const org = tg.org_id ?? "";
    const k = providerKey(org, tg.provider);
    const entry = byProvider.get(k);
    if (!entry) byProvider.set(k, { org_id: org, provider: tg.provider, targets: [tg], worst: tg });
    else entry.targets.push(tg);
  }
  for (const entry of byProvider.values()) {
    entry.targets.sort((a, b) => (a.state === b.state ? 0 : a.state === "open" ? -1 : 1));
    // open sorts first, so the first target is the worst one
    entry.worst = entry.targets[0];
  }
  return byProvider;
}

/**
 * The worst tripped breaker in front of the provider a health card is about.
 * A route reaches its own organization's provider first and a config-file one
 * after it (#1857), so a card of an organization's provider takes that
 * organization's breakers, and a config-file provider's card the breakers of
 * every route with no provider of that name in its own organization.
 */
function cardBreaker(
  group: ProviderGroup,
  byProvider: Map<string, ProviderBreakers>,
  cards: Set<string>,
): BreakerTargetDto | undefined {
  const own = byProvider.get(providerKey(group.org_id, group.provider));
  if (group.org_id !== "") return own?.worst;
  const reached = [...byProvider.values()].filter(
    (entry) =>
      entry.provider === group.provider &&
      (entry.org_id === "" || !cards.has(providerKey(entry.org_id, entry.provider))),
  );
  return reached.find((entry) => entry.worst.state === "open")?.worst ?? reached[0]?.worst;
}

/**
 * What the gateways report about their circuit breakers right now. It is each
 * gateway's in-memory state, reported about every 10 seconds, and says nothing
 * of the 7-day SLA verdict beside it, which is recorded traffic. A provider
 * whose breaker is open or probing is listed with the models it affects; with
 * none, the panel says so, and with no gateway reporting it says that instead.
 */
function BreakerPanel({
  state,
  byProvider,
}: {
  state: BreakerStateDto;
  byProvider: Map<string, ProviderBreakers>;
}) {
  const { t } = useTranslation();
  const providers = [...byProvider.values()];
  const shared = sharedNames(providers.map((p) => ({ name: p.provider, orgId: p.org_id })));
  return (
    <section
      aria-labelledby="health-breakers-title"
      data-testid="health-breakers"
      className="flex flex-col gap-2.5 rounded-lg border bg-card p-4"
    >
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
        <h2 id="health-breakers-title" className="text-sm font-semibold">
          {t("pages.health.breakerTitle")}
        </h2>
      </div>
      <p className="text-xs text-muted-foreground">
        {state.gateways_live > 0
          ? t("pages.health.breakerNote", { count: state.gateways_live })
          : t("pages.health.breakerNoGateways")}
      </p>
      {state.gateways_live > 0 && providers.length === 0 && (
        <p className="text-xs text-[color:var(--text-secondary)]">
          {t("pages.health.breakerNone")}
        </p>
      )}
      {providers.length > 0 && (
        <ul className="divide-y divide-[color:var(--border-subtle)]">
          {providers.map((entry) => (
            <li
              key={providerKey(entry.org_id, entry.provider)}
              data-testid={`health-breaker-${entry.provider}`}
              data-org={entry.org_id}
              className="flex flex-col gap-1.5 py-2 first:pt-0 last:pb-0"
            >
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-mono text-sm font-semibold">{entry.provider}</span>
                {shared.has(entry.provider) && <OwnerOrgPill orgId={entry.org_id} />}
                <BreakerBadge breaker={entry.worst} />
              </div>
              <ul className="flex flex-col gap-1">
                {entry.targets.map((tg) => (
                  <li key={`${tg.model}-${tg.target_index}`} className="flex flex-col gap-0.5">
                    <div className="flex flex-wrap items-center gap-x-2 font-mono text-xs">
                      <span className="text-[color:var(--text-secondary)]">
                        {t("pages.health.breakerTarget", {
                          model: tg.model,
                          index: tg.target_index + 1,
                          upstream: tg.upstream_model,
                        })}
                      </span>
                      <BreakerBadge breaker={tg} />
                    </div>
                    {tg.nodes.length > 0 && <BreakerNodes nodes={tg.nodes} />}
                  </li>
                ))}
              </ul>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** the per-gateway breakers, which only a superadmin's answer carries */
function BreakerNodes({ nodes }: { nodes: BreakerTargetDto["nodes"] }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  return (
    <ul className="flex flex-col gap-0.5 pl-3 text-xs text-[color:var(--text-subtle)]">
      {nodes
        .filter((n) => n.state !== "closed")
        .map((n) => (
          <li key={n.node_id} className="font-mono">
            {n.node_id} · {t(`breaker.states.${n.state}`)} ·{" "}
            {t("breaker.failures", { count: n.consecutive_failures })}
            {n.retry_after_secs !== undefined &&
              ` · ${t("breaker.retryIn", { duration: fmt.duration(n.retry_after_secs) })}`}
          </li>
        ))}
    </ul>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <Overline>{label}</Overline>
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
  now,
}: {
  row: UptimeRow;
  mttr: MttrRow | undefined;
  buckets: TimelineRow[];
  state: SlaState;
  now: number;
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
      {/* a full row of its own below sm, so the four fixed-width figures wrap
          under the name instead of squeezing it to "gp…" (#2004) */}
      <span className="min-w-0 flex-[1_1_100%] truncate font-mono text-xs sm:flex-1">
        {row.target_id}
      </span>
      <Timeline buckets={buckets} className="h-4 w-20 flex-none" />
      <span className="w-16 flex-none text-right font-mono text-xs">
        <span className="sr-only">{t("pages.health.uptimeShort")} </span>
        {pct(fmt, row.events === 0 ? undefined : row.uptime)}
      </span>
      <span className="w-24 flex-none text-right font-mono text-xs text-[color:var(--text-subtle)]">
        {t("pages.health.failuresShort", { count: row.errors + row.timeouts })}
      </span>
      {/* a bare "42s" says nothing, so the figure names itself */}
      <span className="w-28 flex-none text-right font-mono text-xs text-[color:var(--text-subtle)]">
        {t("pages.health.mttr")} {mttrLabel(fmt, mttr?.mttr_seconds)}
      </span>
      <span className="w-40 flex-none text-right">
        <LastEvent at={eventAt(row)} now={now} />
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
    refetchInterval: REFETCH_MS,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `uptime` is the query the user is actually waiting on for this screen
  useScreenReady(!uptime.isLoading);
  const mttr = useQuery({
    queryKey: ["health-mttr"],
    queryFn: fetchMttr,
    refetchInterval: REFETCH_MS,
  });
  const timeline = useQuery({
    queryKey: ["health-timeline"],
    queryFn: () => fetchHealthTimeline("hour"),
    refetchInterval: REFETCH_MS,
  });
  // the gateways' live breakers: optional context, so a failed read draws nothing
  const breakers = useBreakerState();
  const breakersByName = breakersByProvider(breakers.data);
  // one clock for every relative time on the screen, so "3 minutes ago" keeps
  // ageing between refetches and the cards do not drift apart
  const now = useNow(30_000);
  // the page is as fresh as its oldest read; shown only once all three answered
  const updatedAt = Math.min(uptime.dataUpdatedAt, mttr.dataUpdatedAt, timeline.dataUpdatedAt);

  const isLoading = uptime.isLoading || mttr.isLoading || timeline.isLoading;
  const error = uptime.error || mttr.error || timeline.error;

  // a provider is its organization and its name: two organizations may each
  // have an `openai`, and the rollups answer a series for each (#3441)
  const key = (org: string | undefined, provider: string, target: string) =>
    `${providerKey(org, provider)}::${target}`;
  const mttrByTarget = new Map(
    (mttr.data ?? []).map((m) => [key(m.org_id, m.provider, m.target_id), m]),
  );
  const timelineByTarget = new Map<string, TimelineRow[]>();
  for (const row of timeline.data ?? []) {
    const k = key(row.org_id, row.provider, row.target_id);
    const list = timelineByTarget.get(k) ?? [];
    list.push(row);
    timelineByTarget.set(k, list);
  }
  const sourceLabel = useSourceLabel();
  const groups = groupByProvider(uptime.data ?? []);
  const cards = new Set(groups.map((g) => providerKey(g.org_id, g.provider)));
  const shared = sharedNames(groups.map((g) => ({ name: g.provider, orgId: g.org_id })));
  const since = recentSince(timeline.data ?? []);

  return (
    <PageBody className="gap-[18px]">
      <div className="flex items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {t("pages.health.subtitle", { sla: pct(fmt, SLA) })}
        </span>
        {updatedAt > 0 && (
          <span
            data-testid="health-updated"
            className="ml-auto text-xs text-[color:var(--text-subtle)]"
          >
            {t("pages.health.updatedAt", { time: fmt.time(updatedAt) })}
          </span>
        )}
      </div>
      {breakers.data && <BreakerPanel state={breakers.data} byProvider={breakersByName} />}
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
          thread
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
          const at = (target: string) => key(group.org_id, group.provider, target);
          const providerMttr =
            mttrByTarget.get(at(group.provider)) ??
            (group.overall
              ? undefined
              : // no provider-grain incidents to report: fall back to the worst
                // target's, which is the one an operator is chasing
                [...group.targets]
                  .map((tgt) => mttrByTarget.get(at(tgt.target_id)))
                  .filter((m): m is MttrRow => !!m)
                  .sort((a, b) => b.mttr_seconds - a.mttr_seconds)[0]);
          const headBuckets = group.overall
            ? (timelineByTarget.get(at(group.provider)) ?? [])
            : mergeBuckets(
                group.targets.flatMap((tgt) => timelineByTarget.get(at(tgt.target_id)) ?? []),
              );
          const targetState = (row: UptimeRow) =>
            slaState(
              row.sla_breached === 1,
              recentBurn(timelineByTarget.get(at(row.target_id)) ?? [], since),
            );
          // a probed provider is judged on its own row, like its headline; a
          // rolled-up card takes its worst target, as `headline` does for a breach
          const state = group.overall
            ? slaState(head.breached, recentBurn(headBuckets, since))
            : worstState(group.targets.map(targetState));
          return (
            <div
              key={providerKey(group.org_id, group.provider)}
              data-testid={`health-card-${group.provider}`}
              data-org={group.org_id}
              className="flex flex-col gap-3.5 rounded-lg border bg-card p-4"
              style={{
                borderColor:
                  state === "within"
                    ? "var(--border-default)"
                    : `color-mix(in srgb, ${SLA_FILL[state]} 45%, transparent)`,
              }}
            >
              <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1.5">
                <span className="font-mono text-sm font-semibold">{group.provider}</span>
                {shared.has(group.provider) && <OwnerOrgPill orgId={group.org_id} />}
                <span className="font-mono text-xs text-[color:var(--text-subtle)]">
                  {head.derived
                    ? t("pages.health.grainDerived")
                    : t("pages.health.grainProvider", {
                        sources: head.sources.map(sourceLabel).join(", ") || "—",
                      })}
                </span>
                <BreakerBadge breaker={cardBreaker(group, breakersByName, cards)} />
                <SlaPill state={state} />
              </div>
              <div className="flex items-end gap-3">
                <div className="flex flex-col gap-px">
                  <span className={cn("font-mono text-2xl font-medium leading-none")}>
                    {pct(fmt, head.uptime)}
                  </span>
                  <span className="text-2xs uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
                    {t("pages.health.uptimeMeta", { events: fmt.number(head.events) })}
                  </span>
                </div>
                <div className="min-w-0 flex-1">
                  <Timeline buckets={headBuckets} detail />
                </div>
              </div>
              <LastEvent at={head.lastEvent} now={now} />
              <div className="grid grid-cols-1 gap-2.5 border-t sm:grid-cols-3 border-[color:var(--border-subtle)] pt-3">
                <Stat label={t("pages.health.failures")} value={fmt.number(head.failures)} />
                <Stat
                  label={t("pages.health.mttr")}
                  value={mttrLabel(fmt, providerMttr?.mttr_seconds)}
                />
                <Stat
                  label={t("pages.health.incidents")}
                  // the rollups answered and name no incident: that is zero, not unknown
                  value={fmt.number(providerMttr ? providerMttr.incidents : 0)}
                />
              </div>
              {group.targets.length > 0 && (
                <div className="border-t border-[color:var(--border-subtle)] pt-2">
                  <Overline>{t("pages.health.targets", { count: group.targets.length })}</Overline>
                  <div className="divide-y divide-[color:var(--border-subtle)]">
                    {group.targets.map((row) => (
                      <TargetRow
                        key={row.target_id}
                        row={row}
                        mttr={mttrByTarget.get(at(row.target_id))}
                        buckets={timelineByTarget.get(at(row.target_id)) ?? []}
                        state={targetState(row)}
                        now={now}
                      />
                    ))}
                  </div>
                </div>
              )}
              <div className="flex items-center gap-2">
                <span className="text-xs text-muted-foreground">
                  {t("pages.health.errorBudget")}
                </span>
                <span className="ml-auto font-mono text-xs text-[color:var(--text-secondary)]">
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
