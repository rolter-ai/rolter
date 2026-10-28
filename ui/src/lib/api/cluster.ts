import { getJson, sendJson } from "./request";

export interface ClusterNodeRow {
  id: string;
  role: string;
  build_version: string;
  config_version: number;
  desired_state: string;
  state_changed_at: string;
  first_seen_at: string;
  last_seen_at: string;
  // polled inside the liveness window
  live: boolean;
  // reported at least the control plane's current config version
  converged: boolean;
}

/**
 * `GET /api/v1/cluster/summary`: the gateway fleet as counts, readable by every
 * signed-in caller (#2013). `inventory` is `false` when the control plane has no
 * database, which leaves nothing to count.
 */
export interface ClusterSummary {
  inventory: boolean;
  gateways: {
    /** polled the control plane within the last minute */
    live: number;
    /** silent for more than a minute but less than 15: crashed or partitioned */
    stale: number;
    /** live, but not yet on the control plane's `config_version`; a subset of `live` */
    lagging: number;
  };
  config_version: number;
  checked_at: string;
}

export function fetchClusterSummary(): Promise<ClusterSummary> {
  return getJson<ClusterSummary>("/api/v1/cluster/summary");
}

export function fetchClusterNodes(): Promise<ClusterNodeRow[]> {
  return getJson<ClusterNodeRow[]>("/api/v1/cluster/nodes");
}

export function setClusterNodeDrain(id: string, draining: boolean): Promise<ClusterNodeRow> {
  return sendJson<ClusterNodeRow>("PUT", `/api/v1/cluster/nodes/${encodeURIComponent(id)}/drain`, {
    draining,
  });
}

export function forgetClusterNode(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/cluster/nodes/${encodeURIComponent(id)}`);
}

// ---------------------------------------------------------------------------
// adaptive routing policy: the global kill switch and blend weights every route
// on the `adaptive` strategy is governed by
