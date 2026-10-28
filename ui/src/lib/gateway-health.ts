import { useQuery } from "@tanstack/react-query";

import { fetchClusterSummary } from "@/lib/api/cluster";
import { fetchGatewayReadiness, type GatewayReadiness } from "@/lib/gateway";

// the gateway pill in every screen header (#1973) and the fleet tile on the
// dashboard home (#3263) read one query, so the two cannot disagree.
//
// the signal is the control plane's `GET /api/v1/cluster/summary` (#2013): how
// many gateways are live, stale and behind on config, readable by every role.
// it sees the whole fleet, which a probe through a Kubernetes Service cannot, and
// it sees a gateway that still answers `/readyz` on an old `config_version`.
//
// it falls back to the gateway's own `/readyz`, reached through the control
// plane's `/gw` proxy, when the summary has nothing to count: `inventory: false`
// (no database), no gateway in the inventory (none sends a node id), or a failed
// request (an older control plane answers 404). what that proves is only that
// the control plane reached *a* gateway at `ROLTER_GATEWAY_URL` and that one is
// taking traffic.

/** What the pill shows. */
export type GatewayHealth = "checking" | "healthy" | "degraded" | "down" | "unknown";

/** The query key the pill polls under. */
export const GATEWAY_HEALTH_KEY = ["gateway-health"] as const;

/**
 * How often the pill asks. TanStack Query pauses the interval while the tab is
 * hidden (`refetchIntervalInBackground` is off by default) and asks again when
 * the tab comes back.
 */
export const GATEWAY_HEALTH_POLL_MS = 30_000;

/**
 * How long a last answer is still shown after the checks behind it started
 * failing: through one failed poll, not through two. One and a half intervals
 * rather than two, because the gap is measured from when the last answer
 * *arrived*, and a slow answer followed by a fast failure would otherwise
 * stretch the grace to a third poll.
 */
export const GATEWAY_HEALTH_STALE_MS = GATEWAY_HEALTH_POLL_MS * 1.5;

/** The gateways the control plane counted: what `GET /api/v1/cluster/summary` calls `gateways`. */
export interface FleetCounts {
  live: number;
  stale: number;
  lagging: number;
}

/** What one check learned: the fleet's counts, or the one gateway a probe reached. */
export type GatewayCheck = GatewayReadiness | FleetCounts;

const HEALTH: Record<GatewayReadiness, GatewayHealth> = {
  ready: "healthy",
  draining: "degraded",
  unreachable: "down",
  // a gateway with no config refuses every key, which is down for whoever calls
  // it, however well the process itself is running (#3348, #3350)
  awaitingSnapshot: "down",
  snapshotFormatUnsupported: "down",
};

/** A probe answer that says more than its health does, so the pill words it on its own. */
export type GatewayRefusal = Extract<
  GatewayReadiness,
  "awaitingSnapshot" | "snapshotFormatUnsupported"
>;

function refusalOf(data: GatewayCheck): GatewayRefusal | null {
  return data === "awaitingSnapshot" || data === "snapshotFormatUnsupported" ? data : null;
}

/**
 * What a fleet of gateways adds up to. Down only when none is live: a stale or
 * lagging gateway beside live ones is a fleet that is degraded, still taking
 * traffic, which is not the same alarm.
 */
export function fleetHealth(fleet: FleetCounts): GatewayHealth {
  if (fleet.live === 0) return "down";
  return fleet.lagging > 0 || fleet.stale > 0 ? "degraded" : "healthy";
}

export interface GatewayHealthView {
  health: GatewayHealth;
  /**
   * the latest check produced the answer shown. false while an older answer is
   * held through a failed check, and in every state with no answer at all
   */
  live: boolean;
  /** when the answer shown arrived, or `null` when there has been none */
  answeredAt: number | null;
  /** the fleet's counts when the answer came from the cluster summary, else `null` (a probe) */
  fleet: FleetCounts | null;
  /**
   * why a probed gateway refuses every key when it has no config to serve, or
   * `null`. Its health reads `down` either way; this is what the words say
   */
  refusal: GatewayRefusal | null;
}

/** The slice of a query's state the pill is derived from. */
export interface GatewayHealthQuery {
  data: GatewayCheck | undefined;
  dataUpdatedAt: number;
  errorUpdatedAt: number;
  isError: boolean;
}

/**
 * Decide what the pill says from the query behind it.
 *
 * A single failed check does not blank a known answer: it is held, without the
 * pulse, until {@link GATEWAY_HEALTH_STALE_MS} has passed since it arrived.
 * Past that the pill admits it does not know. Both timestamps come from the
 * query, so the derivation is pure and needs no clock of its own.
 */
export function gatewayHealthFrom(query: GatewayHealthQuery): GatewayHealthView {
  const { data, dataUpdatedAt, errorUpdatedAt, isError } = query;
  if (data === undefined) {
    return {
      health: isError ? "unknown" : "checking",
      live: false,
      answeredAt: null,
      fleet: null,
      refusal: null,
    };
  }
  const failedSince = isError && errorUpdatedAt > dataUpdatedAt;
  if (failedSince && errorUpdatedAt - dataUpdatedAt >= GATEWAY_HEALTH_STALE_MS) {
    return {
      health: "unknown",
      live: false,
      answeredAt: dataUpdatedAt,
      fleet: null,
      refusal: null,
    };
  }
  const fleet = typeof data === "string" ? null : data;
  return {
    health: fleet ? fleetHealth(fleet) : HEALTH[data as GatewayReadiness],
    live: !failedSince,
    answeredAt: dataUpdatedAt,
    fleet,
    refusal: refusalOf(data),
  };
}

/**
 * One check: the cluster summary when it counted any gateway, the `/readyz`
 * probe when it did not. A summary that failed is not an error of its own, since
 * the probe is the answer an older control plane has always given.
 */
export async function fetchGatewayCheck(signal?: AbortSignal): Promise<GatewayCheck> {
  try {
    const { inventory, gateways } = await fetchClusterSummary();
    if (inventory && gateways.live + gateways.stale > 0) {
      return { live: gateways.live, stale: gateways.stale, lagging: gateways.lagging };
    }
  } catch {
    // fall through to the probe
  }
  return fetchGatewayReadiness(signal);
}

/** Poll the fleet's health and say what the header pill and the dashboard's fleet tile show. */
export function useGatewayHealth(): GatewayHealthView {
  const query = useQuery({
    queryKey: GATEWAY_HEALTH_KEY,
    queryFn: ({ signal }) => fetchGatewayCheck(signal),
    refetchInterval: GATEWAY_HEALTH_POLL_MS,
    // the poll is the retry: a backoff would only delay the answer the grace
    // window above is already waiting for
    retry: false,
  });
  return gatewayHealthFrom(query);
}
