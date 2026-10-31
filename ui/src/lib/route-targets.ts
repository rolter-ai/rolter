// how a route's targets are read for display (#1979): the effective config
// names each target's provider and upstream model, and the health rollups key
// their rows on the same two strings
import { breakerFor, type BreakerStateDto, type BreakerTargetDto } from "@/lib/api";
import type { RouteDto, TimelineRow, UptimeRow } from "@/lib/api";

/**
 * The SLA the uptime rollup is judged against. The Health screen asks for the
 * same one, so the two screens share a cached answer and agree on "breached".
 */
export const HEALTH_SLA = 0.99;

/**
 * How far back "recently" reaches: the newest hourly bucket the timeline holds
 * and the one before it, so a state never hangs on the few minutes an hour has
 * had so far.
 */
export const RECENT_MS = 60 * 60 * 1000;

/**
 * Where a provider or target stands against the SLA. Every input comes from
 * `provider_health_events`, not from the gateway's circuit breakers, so the
 * states are named after the SLA, never after a breaker (#2113). The Health and
 * Models screens share this one definition.
 *
 * - `breaching`: over the window, more requests failed than the SLA allows
 * - `atRisk`: the window still meets the SLA, but the recent buckets failed
 *   faster than it allows, so the error budget is burning at more than 1x
 * - `within`: the window meets the SLA and the recent buckets do too
 */
export type SlaState = "breaching" | "atRisk" | "within";

export function slaState(breached: boolean, recentBurn: number | undefined): SlaState {
  if (breached) return "breaching";
  return recentBurn !== undefined && recentBurn > 1 ? "atRisk" : "within";
}

const SLA_RANK: Record<SlaState, number> = { within: 0, atRisk: 1, breaching: 2 };

/** the worst of several states, for a card rolled up from its targets */
export function worstState(states: SlaState[]): SlaState {
  return states.reduce<SlaState>((a, b) => (SLA_RANK[b] > SLA_RANK[a] ? b : a), "within");
}

/**
 * A bucket's start in milliseconds. The control plane asks ClickHouse for ISO
 * output, so a bucket carries its zone (`2026-08-06T10:00:00Z`) and is read as
 * is. A zone-less `2026-08-06 10:00:00`, from a control plane that predates
 * that (#2255), is read as UTC: every bucket shares the server's zone, so the
 * gaps between them stay right, which is all `recentBurn` compares.
 */
export function bucketStart(bucket: string): number {
  const iso = bucket.replace(" ", "T");
  return Date.parse(/(Z|[+-]\d{2}:?\d{2})$/i.test(iso) ? iso : `${iso}Z`);
}

/**
 * Where "recently" starts: counted back from the newest bucket the timeline
 * holds, not from the browser's clock, which cannot know the zone the buckets
 * were written in. While probes or traffic flow, that bucket is the current hour.
 */
export function recentSince(timeline: TimelineRow[]): number {
  const newest = timeline.reduce((max, row) => {
    const start = bucketStart(row.bucket);
    return start > max ? start : max;
  }, -Infinity);
  return newest - RECENT_MS;
}

/**
 * Failures over the buckets that start at or after `since`, as a multiple of
 * what the SLA allows — the same formula as the rollup's `error_budget_burn`,
 * over a shorter span. Undefined when nothing was observed in that span.
 */
export function recentBurn(buckets: TimelineRow[], since: number): number | undefined {
  let events = 0;
  let failures = 0;
  for (const b of buckets) {
    // `!(>=)` rather than `<`, so an unparsable bucket is skipped too
    if (!(bucketStart(b.bucket) >= since)) continue;
    events += b.events;
    failures += b.errors + b.timeouts;
  }
  return events === 0 ? undefined : failures / events / (1 - HEALTH_SLA);
}

/**
 * Uptime of one target against the SLA, over the health rollup's window.
 *
 * Read from `GET /api/v1/health/uptime`, which needs ClickHouse and analytics
 * access; where either is missing there is no health to show, and a target
 * carries `null` rather than a guess.
 */
export interface TargetHealth {
  uptime: number;
  breached: boolean;
  state: SlaState;
}

/** One route target as a person reads it: names, never ids. */
export interface RouteTargetView {
  /** the provider's name, which is also what the health rollups key on */
  provider: string;
  /** the model id sent upstream: the target's own, or the route's public name */
  upstream: string;
  weight: number;
  /**
   * the failure classes the target is a fallback for (#1584); empty for an
   * ordinary target. A fallback takes no ordinary traffic, so it has no share
   */
  takesOver?: string[];
  health?: TargetHealth | null;
  /**
   * The target's live breaker where the gateways report one that is not
   * closed; absent for a target that is closed everywhere and wherever the
   * breaker state was not read.
   */
  breaker?: BreakerTargetDto;
}

// a control plane that predates the `grain` column still says which is which
// through the id: a provider-grain row carries the provider as its target id
function grainOf(row: UptimeRow): "provider" | "target" {
  return row.grain ?? (row.target_id === row.provider ? "provider" : "target");
}

/**
 * The rollup rows of the provider a row of organization `org` names
 * `provider` by (#3460). Two organizations may each have a provider of one
 * name, and the rollups answer a series for each (#3441), so a row reaches
 * its own organization's provider first and a config-file one (`org_id` `""`)
 * after it, never another organization's: the rule the gateway resolves a
 * route's targets by. Rows from a control plane that predates `org_id`, and a
 * caller that does not know the organization (`org` undefined), match on the
 * name alone, as before.
 */
export function providerRows<T extends { provider: string; org_id?: string }>(
  rows: T[],
  provider: string,
  org?: string,
): T[] {
  const named = rows.filter((r) => r.provider === provider);
  if (org === undefined || named.every((r) => r.org_id === undefined)) return named;
  const own = named.filter((r) => r.org_id === org);
  return own.length > 0 ? own : named.filter((r) => !r.org_id);
}

/**
 * The health of one target, or `null` when nothing observed it.
 *
 * Passive rows describe the exact target — the provider plus the model id the
 * gateway sent — so they win. A target with no traffic of its own yet falls
 * back to the provider's probe row, which watches the provider as a whole.
 * `org` is the organization of the row naming the provider; see
 * {@link providerRows}.
 */
export function targetHealth(
  rows: UptimeRow[] | undefined,
  provider: string,
  upstream: string,
  timeline?: TimelineRow[],
  org?: string,
): TargetHealth | null {
  if (!Array.isArray(rows)) return null;
  const mine = providerRows(rows, provider, org);
  const own = mine.find((r) => grainOf(r) === "target" && r.target_id === upstream);
  const row = own ?? mine.find((r) => grainOf(r) === "provider");
  if (!row) return null;
  const breached = row.sla_breached === 1;
  // without the timeline there is no recent window, so only the rollup speaks
  const burn = Array.isArray(timeline)
    ? recentBurn(
        timeline.filter(
          (b) =>
            b.provider === row.provider &&
            b.target_id === row.target_id &&
            (b.org_id ?? "") === (row.org_id ?? ""),
        ),
        recentSince(timeline),
      )
    : undefined;
  return { uptime: row.uptime, breached, state: slaState(breached, burn) };
}

/**
 * A route from the effective config as the rows a screen draws.
 *
 * `health` is left off when the uptime rollup was not read, so a screen can
 * tell "no health data for this target" from "health was never asked for".
 * `org` is the organization that owns the route, `""` for a config-file
 * route: its breakers and its providers' health are its own, never those of
 * another organization's route or provider of the same name (#3460).
 */
export function targetViews(
  route: Pick<RouteDto, "model" | "targets">,
  uptime?: UptimeRow[],
  timeline?: TimelineRow[],
  breakers?: Pick<BreakerStateDto, "targets">,
  org?: string,
): RouteTargetView[] {
  return (route.targets ?? []).map((tg, i) => {
    const upstream = tg.model || route.model;
    // matched on the route's own target list, whose order the gateway reports
    const breaker = breakerFor(breakers, route.model, i, org);
    return {
      provider: tg.provider,
      upstream,
      weight: tg.weight,
      takesOver: tg.takes_over ?? [],
      ...(breaker && breaker.state !== "closed" ? { breaker } : {}),
      ...(uptime ? { health: targetHealth(uptime, tg.provider, upstream, timeline, org) } : {}),
    };
  });
}
