import type { StatTone, StatTrend } from "@/components/ui/stat-card";

/**
 * A change smaller than this share of the earlier value reads as flat. Exact
 * equality would draw an arrow for a 0.1% wobble that means nothing (#2010).
 */
export const FLAT_BAND = 0.01;

/** a figure measured against the same figure over the window before */
export interface Comparison {
  trend: StatTrend;
  /** the change as a share of the earlier value: 0.12 is a 12% rise */
  ratio: number;
}

/**
 * How `current` moved against `previous`, or `null` when there is nothing to
 * compare with: no earlier value, or an earlier value of zero, from which no
 * share of a change can be stated. A first day with no previous window draws
 * no arrow rather than a 100% rise (#2010).
 */
export function compare(
  current: number | undefined,
  previous: number | undefined,
): Comparison | null {
  if (current === undefined || previous === undefined) return null;
  if (!Number.isFinite(current) || !Number.isFinite(previous) || previous <= 0) return null;
  const ratio = (current - previous) / previous;
  const trend: StatTrend = Math.abs(ratio) < FLAT_BAND ? "flat" : ratio > 0 ? "up" : "down";
  return { trend, ratio };
}

/**
 * What a move means for a figure whose rise is `rise`: traffic going up is
 * good news, errors and latency going up are bad, and spend going up is
 * neither on its own. Flat is always neutral.
 */
export function toneOfChange(trend: StatTrend, rise: Exclude<StatTone, "warning">): StatTone {
  if (trend === "flat" || rise === "neutral") return "neutral";
  if (trend === "up") return rise;
  return rise === "good" ? "bad" : "good";
}
