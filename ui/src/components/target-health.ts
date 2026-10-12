import type { TargetHealth } from "@/lib/route-targets";

/**
 * How a target's health is drawn, wherever a route's targets are listed: the
 * dot's fill, the text's colour and the catalog key of the words that carry the
 * state, so it is never colour alone (WCAG 1.4.1). `null` is a target nothing
 * observed.
 */
export function healthPresentation(health: TargetHealth | null | undefined): {
  color: string;
  textClass: string;
  labelKey: string;
} {
  if (!health) {
    return {
      color: "var(--zinc-500)",
      textClass: "text-[color:var(--text-subtle)]",
      labelKey: "routeTargets.noHealth",
    };
  }
  if (health.state === "breaching") {
    return {
      color: "var(--status-danger)",
      textClass: "text-[color:var(--status-danger-text)]",
      labelKey: "routeTargets.belowSla",
    };
  }
  if (health.state === "atRisk") {
    return {
      color: "var(--status-warning)",
      textClass: "text-[color:var(--status-warning-text)]",
      labelKey: "routeTargets.slaAtRisk",
    };
  }
  return {
    color: "var(--status-success)",
    textClass: "text-[color:var(--text-secondary)]",
    labelKey: "routeTargets.uptime",
  };
}
