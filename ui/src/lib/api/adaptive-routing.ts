import { getJson, sendJson } from "./request";

// the gateway clamps the exploration ratio to this; the server rejects anything
// above it rather than silently reinterpreting the value
export const MAX_EXPLORATION_RATIO = 0.5;

export const MAX_ADAPTIVE_WEIGHT = 100;

export const MAX_ADAPTIVE_MIN_SAMPLES = 1_000_000;

export interface AdaptiveRoutingPolicyDto {
  enabled: boolean;
  latency_weight: number;
  cost_weight: number;
  load_weight: number;
  exploration_ratio: number;
  min_samples: number;
  updated_at: string;
  // public model names of enabled routes on the `adaptive` strategy, so the
  // blast radius of the kill switch is visible before it is flipped
  affected_routes: string[];
}

export interface UpdateAdaptiveRoutingPolicyInput {
  enabled: boolean;
  latency_weight: number;
  cost_weight: number;
  load_weight: number;
  exploration_ratio: number;
  min_samples: number;
}

export function fetchAdaptiveRoutingPolicy(): Promise<AdaptiveRoutingPolicyDto> {
  return getJson<AdaptiveRoutingPolicyDto>("/api/v1/adaptive-routing-policy");
}

export function updateAdaptiveRoutingPolicy(
  input: UpdateAdaptiveRoutingPolicyInput,
): Promise<AdaptiveRoutingPolicyDto> {
  return sendJson<AdaptiveRoutingPolicyDto>("PUT", "/api/v1/adaptive-routing-policy", input);
}

export interface AdaptiveDecisionCountsDto {
  blend: number;
  exploration: number;
  fallback: number;
}

export interface AdaptiveTargetTelemetryDto {
  target: number;
  provider?: string;
  upstream_model?: string;
  score?: number;
  latency_score?: number;
  cost_score?: number;
  load_score?: number;
  latency_ms?: number;
  cost_per_mtok?: number;
  in_flight?: number;
  samples?: number;
  last_sample_age_ms?: number | null;
}

export interface AdaptiveNodeTelemetryDto {
  node_id: string;
  engaged: boolean;
  observed: number;
  decisions: AdaptiveDecisionCountsDto;
  policy: {
    enabled?: boolean;
    latency_weight?: number;
    cost_weight?: number;
    load_weight?: number;
    exploration_ratio?: number;
    min_samples?: number;
  };
  targets: AdaptiveTargetTelemetryDto[];
  reported_at: string;
}

export interface AdaptiveRouteTelemetryDto {
  /**
   * the organization that owns the route; empty for a config-file route and
   * absent from a control plane that predates the field. Two organizations may
   * each have a route of one name, so a route is `org_id` and `model` (#3458)
   */
  org_id?: string;
  model: string;
  engaged: boolean;
  nodes: AdaptiveNodeTelemetryDto[];
}

export interface AdaptiveRoutingTelemetryDto {
  generated_at: string;
  fresh_window_secs: number;
  routes: AdaptiveRouteTelemetryDto[];
}

export function fetchAdaptiveRoutingTelemetry(): Promise<AdaptiveRoutingTelemetryDto> {
  return getJson<AdaptiveRoutingTelemetryDto>("/api/v1/adaptive-routing-telemetry");
}

// ---------------------------------------------------------------------------
// alerting: channels, rules, notification history
