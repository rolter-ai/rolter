import { type TargetDto } from "./config";
import { getJson, sendJson } from "./request";

/**
 * Every balancing strategy the control plane accepts, in the order of the
 * `STRATEGIES` allowlist in `crates/rolter-control/src/crud/routes.rs` (#897).
 *
 * This is the API contract, not the menu: which of these a picker offers, and
 * with what caveat, is decided in `lib/strategies.ts`. Adding a strategy to the
 * backend allowlist without adding it here makes a route on it uneditable from
 * the dashboard, which is the bug #897 recorded.
 */
export const STRATEGIES = [
  "round_robin",
  "random",
  "power_of_two",
  "consistent_hash",
  "cache_aware",
  "weighted",
  "pipeline",
  "cheapest",
  "fastest",
  "precise_cache_aware",
  "lmcache_aware",
  "adaptive",
  "lora_aware",
  "predicted_latency",
] as const;

/**
 * The fields of a route's `AdvancedModelConfig` the dashboard types; the rest
 * of the blob travels untouched. `context_length` is the model's context
 * window in tokens, served on `GET /v1/models` and not enforced (#3265).
 */
export interface RouteAdvanced extends Record<string, unknown> {
  context_length?: number;
}

export interface RouteRow {
  id: string;
  project_id: string;
  model: string;
  strategy: string;
  enabled: boolean;
  params: Record<string, unknown>;
  param_policy: Record<string, unknown>;
  /**
   * catalog metadata and per-model execution policy, written by
   * `setRouteAdvanced`. an `AdvancedModelConfig` object; every field of it is
   * `#[serde(default)]` on the backend, so a route nobody has edited carries
   * `{}` rather than null
   */
  advanced: RouteAdvanced;
  created_at: string;
  /**
   * the route's targets; present only when the list was read with
   * `include: ["targets"]`, so an absent field means "not asked for", never
   * "no targets" (that is an empty array)
   */
  targets?: RouteTargetRow[];
  /**
   * the route's weighted variants (#391); present only when the list was read
   * with `include: ["variants"]`
   */
  variants?: RouteVariantRow[];
  /**
   * the route's complexity policy; present only when the list was read with
   * `include: ["complexity"]`. `{ tiers: [] }` is a route with no policy
   */
  complexity?: ComplexityPolicy;
}

export interface RouteTargetRow {
  id: string;
  route_id: string;
  provider_id: string;
  upstream_model?: string | null;
  weight: number;
  /**
   * the failure classes this target is a fallback for (#1584); empty for an
   * ordinary member of the pool. Absent from a control plane that predates it
   */
  takes_over?: string[];
  created_at: string;
}

/** what `GET /api/v1/projects/{id}/routes` can embed beside each route */
export type RouteInclude = "targets" | "variants" | "complexity";

/**
 * A project's routes. `include` embeds each route's targets and/or complexity
 * policy, so a screen that shows them reads the project in one request rather
 * than one per route (#3031, #3046).
 */
export function fetchRoutes(projectId: string, include?: RouteInclude[]): Promise<RouteRow[]> {
  const query = include?.length ? `?include=${include.join(",")}` : "";
  return getJson<RouteRow[]>(`/api/v1/projects/${projectId}/routes${query}`);
}

export function createRoute(
  projectId: string,
  input: { model: string; strategy: string },
): Promise<RouteRow> {
  return sendJson<RouteRow>("POST", `/api/v1/projects/${projectId}/routes`, input);
}

/** one entry of a route's target list, as `PUT /api/v1/routes/{id}` takes it */
export interface RouteTargetInput {
  provider_id: string;
  /** null or blank forwards the route's own name; a stored value is sent back unchanged */
  upstream_model?: string | null;
  weight: number;
  /**
   * the failure classes the target takes over; omitted or empty makes it an
   * ordinary target. Sent on every entry, since an entry without it clears a
   * stored set
   */
  takes_over?: string[];
}

/**
 * The body of `PUT /api/v1/routes/{id}`. Every field is optional and an omitted
 * one keeps its stored value. `targets` is the route's whole list, applied in
 * one transaction: an entry is matched to a stored target by `provider_id` and
 * `upstream_model` (a match keeps its id and takes the new weight), a stored
 * target the list leaves out is deleted, and an unmatched entry is added.
 */
export interface UpdateRouteInput {
  enabled?: boolean;
  strategy?: string;
  targets?: RouteTargetInput[];
}

export function updateRoute(id: string, input: UpdateRouteInput): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${id}`, input);
}

export function setRouteEnabled(id: string, enabled: boolean): Promise<RouteRow> {
  return updateRoute(id, { enabled });
}

export function deleteRoute(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/routes/${id}`);
}

export function updateRouteParams(
  id: string,
  params: Record<string, unknown>,
  paramPolicy: Record<string, unknown>,
): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${id}/params`, {
    params,
    param_policy: paramPolicy,
  });
}

export function fetchRouteTargets(routeId: string): Promise<RouteTargetRow[]> {
  return getJson<RouteTargetRow[]>(`/api/v1/routes/${routeId}/targets`);
}

/**
 * One weighted variant of a route (#391), in fallback order (`position`), with
 * its targets in routing order. A route with variants samples one by weight per
 * request and falls back through the rest; its own targets are not used.
 */
export interface RouteVariantRow {
  id: string;
  route_id: string;
  name: string;
  weight: number;
  position: number;
  params: Record<string, unknown>;
  created_at: string;
  targets: RouteVariantTargetRow[];
}

export interface RouteVariantTargetRow {
  id: string;
  variant_id: string;
  provider_id: string;
  upstream_model?: string | null;
  weight: number;
  position: number;
  /** the failure classes this target takes over (#3373); empty for an ordinary target */
  takes_over?: string[];
}

/** One variant as `PUT /api/v1/routes/{id}/variants` takes it. */
export interface RouteVariantInput {
  name: string;
  weight: number;
  params?: Record<string, unknown>;
  targets: RouteTargetInput[];
}

export function fetchRouteVariants(routeId: string): Promise<RouteVariantRow[]> {
  return getJson<RouteVariantRow[]>(`/api/v1/routes/${routeId}/variants`);
}

/** Replace the route's whole variant set; an empty list puts it back on its targets. */
export function setRouteVariants(
  routeId: string,
  variants: RouteVariantInput[],
): Promise<RouteVariantRow[]> {
  return sendJson<RouteVariantRow[]>("PUT", `/api/v1/routes/${routeId}/variants`, { variants });
}

export function createRouteTarget(
  routeId: string,
  input: { provider_id: string; upstream_model?: string; weight?: number },
): Promise<RouteTargetRow> {
  return sendJson<RouteTargetRow>("POST", `/api/v1/routes/${routeId}/targets`, input);
}

export function deleteRouteTarget(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/route-targets/${id}`);
}

// effective model list — bootstrap-config routes (read-only) merged with
// DB-defined routes (full CRUD), as served to the gateway
export interface EffectiveModelDto {
  model: string;
  strategy: string;
  /** how many targets the route has; `route_targets.length` */
  targets: number;
  /** the targets themselves: provider name, upstream model (null forwards the public name), weight; absent from a control plane that predates it */
  route_targets?: TargetDto[];
  /** the organization that owns the route; absent for one from `rolter.toml` */
  org_id?: string;
  /** the project that owns the route; absent for one from `rolter.toml` */
  project_id?: string;
  source: "config" | "db";
}

export function fetchModels(): Promise<EffectiveModelDto[]> {
  return getJson<EffectiveModelDto[]>("/api/v1/models");
}

/**
 * Delete a database model: its routes in one organization (#1857). Names are
 * per organization, so `orgId` says whose; the control plane refuses a delete
 * without it once two organizations share the name.
 */
export function deleteModel(model: string, orgId?: string): Promise<void> {
  const query = orgId ? `?org_id=${encodeURIComponent(orgId)}` : "";
  return sendJson<void>("DELETE", `/api/v1/models/${encodeURIComponent(model)}${query}`);
}

export interface ComplexityTier {
  name: string;
  /// inclusive byte ceiling; null marks the final catch-all tier
  max_input_bytes: number | null;
  route: string;
}

export interface ComplexityPolicy {
  tiers: ComplexityTier[];
}

export function fetchRouteComplexity(routeId: string): Promise<ComplexityPolicy> {
  return getJson<ComplexityPolicy>(`/api/v1/routes/${routeId}/complexity`);
}

export function disableRouteComplexity(routeId: string): Promise<RouteRow> {
  return sendJson<RouteRow>("DELETE", `/api/v1/routes/${routeId}/complexity`);
}

export function setRouteComplexity(routeId: string, policy: ComplexityPolicy): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${routeId}/complexity`, policy);
}

// advanced per-route model configuration (base_url, pricing, limits, headers…)
export function setRouteAdvanced(routeId: string, advanced: RouteAdvanced): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${routeId}/advanced`, {
    advanced,
  });
}

// ---------------------------------------------------------------------------
// plugin configuration registry (#567; gateway dispatch is tracked by #509)
