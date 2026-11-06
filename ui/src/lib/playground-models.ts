// what the playground knows about a route before anyone sends to it (#3652):
// whether its targets carried traffic and answered, and which model this
// browser last picked. both only steer the column's starting model and mark
// the picker; nothing here refuses a route
import type { EffectiveModelDto, UptimeRow } from "@/lib/api";
import { targetHealth } from "@/lib/route-targets";

/**
 * `working`: a target of the route answered its recent traffic within the SLA.
 * `failing`: every target of it is below the SLA, so a message would most
 * likely fail. A route with no health to judge it by has no verdict.
 */
export type RouteVerdict = "working" | "failing";

/**
 * A verdict per public model name, from the health rollup. Several projects
 * may route one name, and a name is failing only when all of its routes are.
 */
export function routeVerdicts(
  routes: EffectiveModelDto[] | undefined,
  uptime: UptimeRow[] | undefined,
): Map<string, RouteVerdict> {
  const out = new Map<string, RouteVerdict>();
  if (!routes || !Array.isArray(uptime)) return out;
  const failingAll = new Map<string, boolean>();
  for (const route of routes) {
    const targets = route.route_targets ?? [];
    if (targets.length === 0) continue;
    const health = targets.map((tg) =>
      targetHealth(uptime, tg.provider, tg.model || route.model, undefined, route.org_id),
    );
    if (health.some((h) => h && h.state === "within" && h.uptime > 0)) {
      out.set(route.model, "working");
      continue;
    }
    const failing = health.every((h) => h !== null && h.state === "breaching");
    failingAll.set(route.model, (failingAll.get(route.model) ?? true) && failing);
  }
  for (const [model, failing] of failingAll) {
    if (failing && !out.has(model)) out.set(model, "failing");
  }
  return out;
}

/**
 * The model to open on among `candidates`, in their order: one whose route
 * answers its traffic first, then one with nothing known against it. A list
 * whose every route is failing opens on nothing, so the column keeps the
 * built-in that always answers.
 */
export function pickModel(
  candidates: string[],
  verdicts: Map<string, RouteVerdict>,
): string | null {
  return (
    candidates.find((id) => verdicts.get(id) === "working") ??
    candidates.find((id) => verdicts.get(id) !== "failing") ??
    null
  );
}

const LAST_MODEL_KEY = "rolter.playground.lastModel";

/** the model this browser last picked in a chat column, if storage allows */
export function lastPickedModel(): string | null {
  try {
    return window.localStorage.getItem(LAST_MODEL_KEY);
  } catch {
    return null;
  }
}

export function rememberPickedModel(model: string): void {
  try {
    window.localStorage.setItem(LAST_MODEL_KEY, model);
  } catch {
    // a browser that refuses storage opens on the catalog's pick next time
  }
}
