// Presentation rules for balancing strategies (#897).
//
// `STRATEGIES` in `api.ts` is the contract: every value the control plane will
// accept. That is not the same as the menu. Two things have to be true at once:
//
//   - a picker must offer everything an operator can meaningfully choose, or
//     the dashboard cannot express a route the API can;
//   - a picker must never silently rewrite a value it was not going to offer.
//
// The second is the one that bites. A native `<select>` whose `value` matches
// no `<option>` does not render that value — it falls back to showing the first
// option — so a group balanced by `adaptive` displayed as `round_robin`, and
// the operator had no way to know the screen was lying to them.

import { STRATEGIES } from "@/lib/api";

export type Strategy = (typeof STRATEGIES)[number];

/**
 * Strategies governed somewhere other than this picker.
 *
 * `adaptive` is driven by the deployment-wide `[adaptive_routing]` policy and
 * has its own screen; offering it in a per-route dropdown would misrepresent
 * how it is controlled. It is still rendered when it is already the value —
 * see {@link strategyOptions} — because hiding a choice is not a reason to
 * destroy it.
 */
const NOT_OFFERED: readonly string[] = ["adaptive"];

/**
 * Strategies that need a telemetry source configured on the member providers.
 *
 * They are offered rather than disabled: an operator who *has* wired up KV
 * events or an LMCache controller must be able to select them, and the
 * dashboard cannot tell from here whether they did. The caveat is carried as a
 * hint instead, since the failure mode is a quiet degrade to least-load rather
 * than an error.
 */
export const NEEDS_TELEMETRY: readonly string[] = ["precise_cache_aware", "lmcache_aware"];

/** The i18n key describing `strategy`, or `null` when it needs no caveat. */
export function strategyHintKey(strategy: string): string | null {
  if (NEEDS_TELEMETRY.includes(strategy)) return "pages.routing.strategyHints.needsTelemetry";
  if (NOT_OFFERED.includes(strategy)) return "pages.routing.strategyHints.deploymentWide";
  return null;
}

/**
 * The options a strategy picker should render, given the value it currently
 * holds.
 *
 * `current` is always present in the result even when it is not offered, so the
 * control displays the truth and a save cannot drop it. An unknown value — a
 * strategy added to the backend and not yet to `STRATEGIES` — is preserved on
 * the same principle rather than snapped to a default.
 */
export function strategyOptions(current?: string): string[] {
  const offered = STRATEGIES.filter((s) => !NOT_OFFERED.includes(s));
  if (current && !offered.includes(current as Strategy)) return [current, ...offered];
  return [...offered];
}

/** `[color, tint]` for the strategy pill. */
export const STRATEGY_TONE: Record<string, [string, string]> = {
  cache_aware: ["var(--status-info-text)", "rgba(59,130,246,.14)"],
  precise_cache_aware: ["var(--status-info-text)", "rgba(59,130,246,.14)"],
  lmcache_aware: ["var(--status-info-text)", "rgba(59,130,246,.14)"],
  lora_aware: ["var(--status-info-text)", "rgba(59,130,246,.14)"],
  predicted_latency: ["var(--status-info-text)", "rgba(59,130,246,.14)"],
  weighted: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  cheapest: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  round_robin: ["var(--text-secondary)", "var(--surface-subtle)"],
  random: ["var(--text-secondary)", "var(--surface-subtle)"],
  pipeline: ["var(--text-secondary)", "var(--surface-subtle)"],
  consistent_hash: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
  fastest: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
  adaptive: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
  power_of_two: ["var(--red-folk-text)", "var(--red-tint)"],
};

/** Tone for `strategy`, falling back to the neutral one for unknown values. */
export function strategyTone(strategy: string): [string, string] {
  return STRATEGY_TONE[strategy] ?? STRATEGY_TONE.round_robin;
}
