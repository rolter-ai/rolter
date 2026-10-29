// how the health rollup's rows read as providers (#1257, #3344): the Health
// screen's cards and the dashboard home's provider tile share one definition
// of a provider's headline figures, so the two cannot disagree about whether a
// provider is breaching its SLA
import type { UptimeRow } from "@/lib/api";
import { bucketStart } from "@/lib/route-targets";

/** when a rollup row last saw an event, in ms; the zone-less ClickHouse form is read as UTC */
export function eventAt(row: UptimeRow): number | undefined {
  const at = bucketStart(row.last_event);
  return Number.isNaN(at) ? undefined : at;
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
export type Grain = "provider" | "target";

export interface ProviderGroup {
  /**
   * the organization that owns the provider, `""` for a config-file provider
   * or a control plane that predates the field. Two organizations may each
   * have a provider of one name, and each is a group of its own (#3441)
   */
  org_id: string;
  provider: string;
  /** the provider-grain row, when probes or a status page feed this provider */
  overall: UptimeRow | undefined;
  targets: UptimeRow[];
}

export interface Headline {
  /** undefined when no event was observed: a figure nobody measured is not shown */
  uptime: number | undefined;
  /** the newest event behind the card, in ms */
  lastEvent: number | undefined;
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
export function headline(group: ProviderGroup): Headline {
  const row = group.overall;
  if (row) {
    return {
      uptime: row.events === 0 ? undefined : row.uptime,
      lastEvent: eventAt(row),
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
  let lastEvent: number | undefined;
  const sources = new Set<string>();
  for (const t of group.targets) {
    events += t.events;
    ok += t.ok;
    failures += t.errors + t.timeouts;
    // the budget is per target, so the card reports the worst one rather than
    // an average that would hide it
    burn = Math.max(burn, t.error_budget_burn);
    const at = eventAt(t);
    if (at !== undefined) lastEvent = Math.max(lastEvent ?? at, at);
    for (const s of t.sources ?? []) sources.add(s);
  }
  return {
    // no events is no data, not a perfect score
    uptime: events === 0 ? undefined : ok / events,
    lastEvent,
    events,
    failures,
    burn,
    breached: group.targets.some((t) => t.sla_breached === 1),
    derived: true,
    sources: [...sources].sort(),
  };
}

/** a provider's identity across organizations, for a map key or a React key */
export function providerKey(org: string | undefined, provider: string): string {
  return `${org ?? ""}/${provider}`;
}

export function groupByProvider(rows: UptimeRow[]): ProviderGroup[] {
  const byProvider = new Map<string, ProviderGroup>();
  for (const row of rows) {
    const org = row.org_id ?? "";
    let group = byProvider.get(providerKey(org, row.provider));
    if (!group) {
      group = { org_id: org, provider: row.provider, overall: undefined, targets: [] };
      byProvider.set(providerKey(org, row.provider), group);
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
  // worst first, the same intent as the server's `order by uptime asc`; a
  // provider with nothing measured goes last
  groups.sort((a, b) => (headline(a).uptime ?? 2) - (headline(b).uptime ?? 2));
  return groups;
}

/**
 * The providers the health rollup reports, reduced to what one tile can say
 * (#3344): how many meet the SLA of the total that reported, and the worst one.
 *
 * A provider with no observed event is left out of the count, since no event is
 * no data and not a perfect score; one breaching its SLA over the window is not
 * up. `worst` is the breaching provider with the lowest uptime, or, when none
 * breaches, the provider with the lowest uptime at all.
 */
export interface ProviderRollup {
  total: number;
  up: number;
  breaching: string[];
  worst: { provider: string; uptime: number; breaching: boolean } | undefined;
}

export function rollUpProviders(rows: UptimeRow[]): ProviderRollup {
  const measured = groupByProvider(rows)
    .map((group) => ({ provider: group.provider, head: headline(group) }))
    .filter((entry) => entry.head.uptime !== undefined);
  const breaching = measured.filter((entry) => entry.head.breached);
  // `groupByProvider` orders worst uptime first
  const worst = breaching[0] ?? measured[0];
  return {
    total: measured.length,
    up: measured.length - breaching.length,
    breaching: breaching.map((entry) => entry.provider),
    worst: worst
      ? {
          provider: worst.provider,
          uptime: worst.head.uptime as number,
          breaching: worst.head.breached,
        }
      : undefined,
  };
}
