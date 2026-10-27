// Presentation rules for balancing strategies (#897).
//
// `STRATEGIES` in `api/routes.ts` is the contract: every value the control plane will
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
import type { DocsPage } from "@/lib/docs";

export type Strategy = (typeof STRATEGIES)[number];

/**
 * Strategies a deployment-wide policy governs on top of the per-route choice.
 *
 * `adaptive` is picked per route like any other, but the `[adaptive_routing]`
 * policy sets its blend and the kill switch, so its hint says so and links
 * Adaptive Routing settings.
 */
const DEPLOYMENT_WIDE: readonly string[] = ["adaptive"];

/**
 * How the pickers group the strategies, in the order the groups are listed
 * (#2136). Every value of `STRATEGIES` sits in exactly one; the strategy test
 * fails on one that sits in none.
 */
export const STRATEGY_GROUPS = {
  simple: ["round_robin", "random", "power_of_two"],
  cacheAware: [
    "cache_aware",
    "consistent_hash",
    "precise_cache_aware",
    "lmcache_aware",
    "lora_aware",
  ],
  latencyCost: ["fastest", "predicted_latency", "cheapest", "adaptive"],
  weighted: ["weighted", "pipeline"],
} as const satisfies Record<string, readonly Strategy[]>;

export type StrategyGroup = keyof typeof STRATEGY_GROUPS;

/** The group `strategy` is listed under, or `null` for a value this build does not know. */
export function strategyGroup(strategy: string): StrategyGroup | null {
  for (const [group, members] of Object.entries(STRATEGY_GROUPS)) {
    if ((members as readonly string[]).includes(strategy)) return group as StrategyGroup;
  }
  return null;
}

/**
 * The i18n key of the one-line description of `strategy`, drawn from the
 * load-balancing page's "best for" column, or `null` for a value this build
 * does not know.
 */
export function strategyDescriptionKey(strategy: string): string | null {
  // camel-cased: a key ending `_two` (`power_of_two`) reads as a plural form
  const key = strategy.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
  return strategyGroup(strategy) ? `strategies.descriptions.${key}` : null;
}

/**
 * Strategies that need a telemetry source configured on the member providers.
 *
 * They are offered rather than disabled: an operator who *has* wired up KV
 * events or an LMCache controller must be able to select them, and the
 * dashboard cannot tell from here whether they did. The caveat is carried as a
 * hint instead, since the failure mode is a quiet degrade to least-load rather
 * than an error.
 *
 * The source is set per provider, in the provider sheet or in `rolter.toml`
 * (#2236, #2996), and the hint says so.
 */
export const NEEDS_TELEMETRY: readonly string[] = ["precise_cache_aware", "lmcache_aware"];

/**
 * Strategies that read a target's `weight` (`build_with_stats` in
 * `crates/rolter-balancer/src/lib.rs`).
 *
 * Every other strategy is built from the target count alone, so a weight on
 * one of their targets is stored and never consulted. A screen that turns
 * weights into traffic shares for those would be stating a split the gateway
 * does not make.
 */
const READS_WEIGHTS: readonly string[] = ["weighted", "pipeline", "adaptive"];

/** Whether the gateway spreads traffic by weight under `strategy`. */
export function usesWeights(strategy: string): boolean {
  return READS_WEIGHTS.includes(strategy);
}

/** The i18n key describing `strategy`, or `null` when it needs no caveat. */
export function strategyHintKey(strategy: string): string | null {
  if (NEEDS_TELEMETRY.includes(strategy)) return "pages.routing.strategyHints.needsTelemetry";
  if (DEPLOYMENT_WIDE.includes(strategy)) return "pages.routing.strategyHints.deploymentWide";
  return null;
}

/**
 * The documentation page that explains `strategy`'s caveat at length, or
 * `null` when the hint says all there is to say.
 *
 * The hint has to stand on its own: `DocsLink` renders nothing when the
 * deployment configured no documentation host, which is the air-gapped
 * default.
 */
export function strategyDocsPage(strategy: string): DocsPage | null {
  if (NEEDS_TELEMETRY.includes(strategy)) return "cacheAwareRouting";
  return null;
}

/** Where the deployment-wide policy of `strategy` is set, when it has one. */
export function strategyPolicyPath(strategy: string): string | null {
  return DEPLOYMENT_WIDE.includes(strategy) ? "/adaptive-settings" : null;
}

/**
 * The options a strategy picker should render, given the value it currently
 * holds, ordered by group.
 *
 * `current` is always present in the result even when this build does not know
 * it, so the control displays the truth and a save cannot drop it. An unknown
 * value — a strategy added to the backend and not yet to `STRATEGIES` — is
 * preserved on the same principle rather than snapped to a default.
 */
export function strategyOptions(current?: string): string[] {
  // grouped first; a contract value no group lists yet is appended, not lost
  const grouped: string[] = Object.values(STRATEGY_GROUPS).flatMap((m) => [...m]);
  const offered = [...grouped, ...STRATEGIES.filter((s) => !grouped.includes(s))];
  if (current && !offered.includes(current)) return [current, ...offered];
  return offered;
}
