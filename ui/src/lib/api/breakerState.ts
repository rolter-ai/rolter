import { useQuery } from "@tanstack/react-query";

import { getJson } from "./request";

/** a circuit breaker's state on one gateway; `half_open` is the probing state */
export type BreakerStateName = "closed" | "open" | "half_open";

/** one gateway's breaker for a target; only a superadmin's response carries these */
export interface BreakerNodeDto {
  node_id: string;
  state: BreakerStateName;
  consecutive_failures: number;
  retry_after_secs?: number;
  reported_at: string;
}

/**
 * One route target's breaker across the live gateways. Only targets with a
 * breaker entry are listed: a target that is absent is closed on every live
 * gateway. A `closed` row is a target that is failing but has not tripped yet.
 */
export interface BreakerTargetDto {
  /**
   * the organization that owns the route; empty for a route from the
   * gateway's own config file, and absent from a control plane that predates
   * the field. Two organizations may each have a route of one name, so a route
   * is `org_id` and `model` together (#3458)
   */
  org_id?: string;
  model: string;
  /** position in the route's target list */
  target_index: number;
  /** the provider's name, which the health rollups key on */
  provider: string;
  upstream_model: string;
  /** the worst state across the gateways */
  state: BreakerStateName;
  nodes_open: number;
  nodes_half_open: number;
  nodes: BreakerNodeDto[];
}

export interface BreakerStateDto {
  generated_at: string;
  fresh_window_secs: number;
  /** 0 means nothing can be said about the fleet: no inventory or no gateway polling */
  gateways_live: number;
  targets: BreakerTargetDto[];
}

/** how often the screens re-read the live breakers; gateways report about every 10 s */
export const BREAKER_STATE_POLL_MS = 15_000;

export function fetchBreakerState(): Promise<BreakerStateDto> {
  return getJson<BreakerStateDto>("/api/v1/breaker-state").then((r) => {
    // a control plane that answers with anything else has nothing to say
    if (!r || !Array.isArray(r.targets)) throw new TypeError("breaker_state_shape");
    return r;
  });
}

/**
 * The live breakers, or no data. The read is optional context on screens that
 * are about something else: a control plane without a store, a caller who
 * cannot read it or a failed request all leave `data` undefined, and the
 * screens then draw what they drew before there was breaker state.
 */
export function useBreakerState() {
  return useQuery({
    queryKey: ["breaker-state"],
    queryFn: fetchBreakerState,
    retry: false,
    refetchInterval: BREAKER_STATE_POLL_MS,
  });
}

/**
 * The breaker of one target of a route, or undefined when none is listed (the
 * target is closed on every live gateway). `org` is the organization that owns
 * the route, `""` for a config-file route: another organization's route of the
 * same name has a breaker of its own (#3460). A row from a control plane that
 * predates `org_id`, or a caller that does not know the route's organization,
 * matches on the name alone, as before.
 */
export function breakerFor(
  state: Pick<BreakerStateDto, "targets"> | undefined,
  model: string,
  targetIndex: number,
  org?: string,
): BreakerTargetDto | undefined {
  return state?.targets.find(
    (tg) =>
      tg.model === model &&
      tg.target_index === targetIndex &&
      (org === undefined || tg.org_id === undefined || tg.org_id === org),
  );
}

/** true for a breaker that is stopping or probing traffic on at least one gateway */
export function isTripped(
  breaker: Pick<BreakerTargetDto, "state"> | undefined,
): breaker is BreakerTargetDto {
  return !!breaker && breaker.state !== "closed";
}

const RANK: Record<BreakerStateName, number> = { closed: 0, half_open: 1, open: 2 };

/** the worst of several states */
export function worstBreaker(states: BreakerStateName[]): BreakerStateName {
  return states.reduce<BreakerStateName>((a, b) => (RANK[b] > RANK[a] ? b : a), "closed");
}

/**
 * The words' catalog key and plural count for a tripped breaker: `open` counts
 * the gateways it is open on, `probing` the gateways letting a trial request
 * through. Undefined for a closed breaker, which shows nothing.
 */
export function breakerLabel(
  breaker: BreakerTargetDto | undefined,
): { key: "breaker.open" | "breaker.probing"; count: number } | undefined {
  if (!isTripped(breaker)) return undefined;
  return breaker.state === "open"
    ? { key: "breaker.open", count: breaker.nodes_open }
    : { key: "breaker.probing", count: breaker.nodes_half_open };
}
