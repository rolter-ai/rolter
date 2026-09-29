// how a route's targets are read for display (#1979): the effective config
// names each target's provider and upstream model, and the health rollups key
// their rows on the same two strings
import type { RouteDto, UptimeRow } from "@/lib/api";

/**
 * The SLA the uptime rollup is judged against. The Health screen asks for the
 * same one, so the two screens share a cached answer and agree on "breached".
 */
export const HEALTH_SLA = 0.99;

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
}

/** One route target as a person reads it: names, never ids. */
export interface RouteTargetView {
  /** the provider's name, which is also what the health rollups key on */
  provider: string;
  /** the model id sent upstream: the target's own, or the route's public name */
  upstream: string;
  weight: number;
  health?: TargetHealth | null;
}

// a control plane that predates the `grain` column still says which is which
// through the id: a provider-grain row carries the provider as its target id
function grainOf(row: UptimeRow): "provider" | "target" {
  return row.grain ?? (row.target_id === row.provider ? "provider" : "target");
}

/**
 * The health of one target, or `null` when nothing observed it.
 *
 * Passive rows describe the exact target — the provider plus the model id the
 * gateway sent — so they win. A target with no traffic of its own yet falls
 * back to the provider's probe row, which watches the provider as a whole.
 */
export function targetHealth(
  rows: UptimeRow[] | undefined,
  provider: string,
  upstream: string,
): TargetHealth | null {
  if (!Array.isArray(rows)) return null;
  const own = rows.find(
    (r) => r.provider === provider && grainOf(r) === "target" && r.target_id === upstream,
  );
  const row = own ?? rows.find((r) => r.provider === provider && grainOf(r) === "provider");
  return row ? { uptime: row.uptime, breached: row.sla_breached === 1 } : null;
}

/**
 * A route from the effective config as the rows a screen draws.
 *
 * `health` is left off when the uptime rollup was not read, so a screen can
 * tell "no health data for this target" from "health was never asked for".
 */
export function targetViews(route: RouteDto, uptime?: UptimeRow[]): RouteTargetView[] {
  return (route.targets ?? []).map((tg) => {
    const upstream = tg.model || route.model;
    return {
      provider: tg.provider,
      upstream,
      weight: tg.weight,
      ...(uptime ? { health: targetHealth(uptime, tg.provider, upstream) } : {}),
    };
  });
}
