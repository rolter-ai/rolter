import type { ClusterNodeRow } from "@/lib/api";

/**
 * Whether the gateways have picked up what was just saved.
 *
 * A write to a setting the data plane reads bumps the control plane's config
 * version, and every gateway reports the version it runs on each snapshot poll.
 * `GET /api/v1/cluster/nodes` compares the two per node (`converged`), so the
 * dashboard can say which gateways run the change instead of promising that
 * they will. Read it after the save: the version it compares against is the
 * one the save produced, so a node that has not polled since shows as lagging.
 *
 * Only a gateway that is polling counts. A node outside the liveness window is
 * not serving anything, and a control-plane node is not a gateway.
 */
export type Pickup =
  /** the nodes have not answered yet */
  | { state: "checking" }
  /** the read failed, so nothing is known */
  | { state: "unavailable" }
  /** the read worked and no live gateway has reported to this control plane */
  | { state: "none" }
  /** every live gateway runs the current config */
  | { state: "converged"; total: number }
  /** some do not yet; they apply it on their next poll */
  | { state: "lagging"; converged: number; total: number };

export function gatewayPickup(
  nodes: readonly ClusterNodeRow[] | undefined,
  failed: boolean,
): Pickup {
  if (failed) return { state: "unavailable" };
  if (nodes === undefined) return { state: "checking" };
  // a proxy that answers something other than an inventory has told us nothing
  if (!Array.isArray(nodes)) return { state: "unavailable" };
  const live = nodes.filter((node) => node.role === "gateway" && node.live);
  if (live.length === 0) return { state: "none" };
  const converged = live.filter((node) => node.converged).length;
  return converged === live.length
    ? { state: "converged", total: live.length }
    : { state: "lagging", converged, total: live.length };
}
