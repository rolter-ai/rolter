import { useQuery } from "@tanstack/react-query";

import { fetchGatewayReadiness, type GatewayReadiness } from "@/lib/gateway";

// the gateway pill in every screen header (#1973). it used to be a hard-coded
// green "gateway healthy" with no request behind it, so it stayed green through
// the one incident an operator would look at it for.
//
// the signal is the gateway's own `/readyz`, reached through the control
// plane's `/gw` proxy — the only gateway state the dashboard can read for every
// role: `/api/v1/cluster/nodes` is superadmin-only, and `/api/v1/health/*` is
// provider health out of ClickHouse, not the gateway's. what it proves is that
// the control plane reached *a* gateway at `ROLTER_GATEWAY_URL` and that one is
// taking traffic. it cannot see the rest of a fleet or whether a gateway runs
// the current config; that needs an endpoint every caller can read (#2013).

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

const HEALTH: Record<GatewayReadiness, GatewayHealth> = {
  ready: "healthy",
  draining: "degraded",
  unreachable: "down",
};

export interface GatewayHealthView {
  health: GatewayHealth;
  /**
   * the latest check produced the answer shown. false while an older answer is
   * held through a failed check, and in every state with no answer at all
   */
  live: boolean;
  /** when the answer shown arrived, or `null` when there has been none */
  answeredAt: number | null;
}

/** The slice of a query's state the pill is derived from. */
export interface GatewayHealthQuery {
  data: GatewayReadiness | undefined;
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
    return { health: isError ? "unknown" : "checking", live: false, answeredAt: null };
  }
  const failedSince = isError && errorUpdatedAt > dataUpdatedAt;
  if (failedSince && errorUpdatedAt - dataUpdatedAt >= GATEWAY_HEALTH_STALE_MS) {
    return { health: "unknown", live: false, answeredAt: dataUpdatedAt };
  }
  return { health: HEALTH[data], live: !failedSince, answeredAt: dataUpdatedAt };
}

/** Poll the gateway's readiness and say what the header pill should show. */
export function useGatewayHealth(): GatewayHealthView {
  const query = useQuery({
    queryKey: GATEWAY_HEALTH_KEY,
    queryFn: ({ signal }) => fetchGatewayReadiness(signal),
    refetchInterval: GATEWAY_HEALTH_POLL_MS,
    // the poll is the retry: a backoff would only delay the answer the grace
    // window above is already waiting for
    retry: false,
  });
  return gatewayHealthFrom(query);
}
