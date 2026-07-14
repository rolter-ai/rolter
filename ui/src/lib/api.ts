// typed fetch helpers for the rolter control api (proxied at /api in dev)

export interface TargetDto {
  provider: string;
  model?: string | null;
  weight: number;
}

export interface RouteDto {
  model: string;
  strategy: string;
  targets: TargetDto[];
}

export interface VirtualKeyDto {
  key: string;
  name?: string | null;
  models: string[];
}

export interface ProviderDto {
  name: string;
  kind: string;
  api_base: string;
}

export interface GatewayConfigDto {
  providers: ProviderDto[];
  routes: RouteDto[];
  virtual_keys: VirtualKeyDto[];
}

async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(`request failed: ${res.status}`);
  }
  return (await res.json()) as T;
}

/// extract the control api's `{"error": {"message": ...}}` body when present,
/// falling back to the raw status text
async function apiError(res: Response): Promise<Error> {
  try {
    const body = (await res.json()) as { error?: { message?: string } };
    if (body?.error?.message) {
      return new Error(body.error.message);
    }
  } catch {
    // not json, fall through
  }
  return new Error(`request failed: ${res.status}`);
}

async function sendJson<T>(
  method: "POST" | "PUT" | "DELETE",
  url: string,
  body?: unknown,
): Promise<T> {
  const res = await fetch(url, {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    throw await apiError(res);
  }
  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

export function fetchConfig(): Promise<GatewayConfigDto> {
  return getJson<GatewayConfigDto>("/api/v1/config");
}

export function fetchRoles(): Promise<string[]> {
  return getJson<string[]>("/api/v1/roles");
}

// provider stability rollups over provider_health_events (ROL-198)

export interface UptimeRow {
  provider: string;
  target_id: string;
  events: number;
  ok: number;
  errors: number;
  timeouts: number;
  uptime: number;
  failure_rate: number;
  error_budget_burn: number;
  sla_breached: number;
  last_event: string;
}

export interface MttrRow {
  provider: string;
  target_id: string;
  mttr_seconds: number;
  incidents: number;
}

export interface TimelineRow {
  bucket: string;
  provider: string;
  target_id: string;
  events: number;
  ok: number;
  errors: number;
  timeouts: number;
}

interface DataEnvelope<T> {
  data: T[];
}

export function fetchUptime(sla = 0.99): Promise<UptimeRow[]> {
  return getJson<DataEnvelope<UptimeRow>>(
    `/api/v1/health/uptime?sla=${sla}`,
  ).then((r) => r.data);
}

export function fetchMttr(): Promise<MttrRow[]> {
  return getJson<DataEnvelope<MttrRow>>("/api/v1/health/mttr").then(
    (r) => r.data,
  );
}

export function fetchHealthTimeline(bucket = "hour"): Promise<TimelineRow[]> {
  return getJson<DataEnvelope<TimelineRow>>(
    `/api/v1/health/timeline?bucket=${bucket}`,
  ).then((r) => r.data);
}

// --- control-plane CRUD (only reachable when rolter-control is started
// with --database-url; see crates/rolter-control/src/crud.rs) ---

export const PROVIDER_KINDS = [
  "openai",
  "anthropic",
  "openai_compatible",
  "ollama",
  "ollama_cloud",
  "llama_cpp",
  "openrouter",
  "tei",
  "azure_openai",
  "bedrock",
  "vertex",
] as const;

export const STRATEGIES = [
  "round_robin",
  "random",
  "power_of_two",
  "consistent_hash",
  "cache_aware",
  "weighted",
  "pipeline",
] as const;

export interface OrgRow {
  id: string;
  name: string;
  slug: string;
  created_at: string;
}

export interface TeamRow {
  id: string;
  org_id: string;
  name: string;
  created_at: string;
}

export interface ProjectRow {
  id: string;
  team_id: string;
  name: string;
  created_at: string;
}

export function fetchOrgs(): Promise<OrgRow[]> {
  return getJson<OrgRow[]>("/api/v1/orgs");
}

export function fetchTeams(orgId: string): Promise<TeamRow[]> {
  return getJson<TeamRow[]>(`/api/v1/orgs/${orgId}/teams`);
}

export function fetchProjects(teamId: string): Promise<ProjectRow[]> {
  return getJson<ProjectRow[]>(`/api/v1/teams/${teamId}/projects`);
}

export function createOrg(input: { name: string; slug: string }): Promise<OrgRow> {
  return sendJson<OrgRow>("POST", "/api/v1/orgs", input);
}

export function deleteOrg(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/orgs/${id}`);
}

export function createTeam(
  orgId: string,
  input: { name: string },
): Promise<TeamRow> {
  return sendJson<TeamRow>("POST", `/api/v1/orgs/${orgId}/teams`, input);
}

export function deleteTeam(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/teams/${id}`);
}

export function createProject(
  teamId: string,
  input: { name: string },
): Promise<ProjectRow> {
  return sendJson<ProjectRow>("POST", `/api/v1/teams/${teamId}/projects`, input);
}

export function deleteProject(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/projects/${id}`);
}

export interface ProviderRow {
  id: string;
  org_id: string;
  name: string;
  kind: string;
  api_base: string;
  api_key_env?: string | null;
  egress_proxy?: string | null;
  created_at: string;
}

export interface CreateProviderInput {
  name: string;
  kind: string;
  api_base: string;
  api_key?: string;
  api_key_env?: string;
  egress_proxy?: string;
}

export interface UpdateProviderInput {
  kind?: string;
  api_base?: string;
  api_key?: string;
  api_key_env?: string;
  egress_proxy?: string;
}

export function fetchProviders(orgId: string): Promise<ProviderRow[]> {
  return getJson<ProviderRow[]>(`/api/v1/orgs/${orgId}/providers`);
}

export function createProvider(
  orgId: string,
  input: CreateProviderInput,
): Promise<ProviderRow> {
  return sendJson<ProviderRow>("POST", `/api/v1/orgs/${orgId}/providers`, input);
}

export function updateProvider(
  id: string,
  input: UpdateProviderInput,
): Promise<ProviderRow> {
  return sendJson<ProviderRow>("PUT", `/api/v1/providers/${id}`, input);
}

export function deleteProvider(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/providers/${id}`);
}

export interface RouteRow {
  id: string;
  project_id: string;
  model: string;
  strategy: string;
  enabled: boolean;
  params: Record<string, unknown>;
  param_policy: Record<string, unknown>;
  created_at: string;
}

export interface RouteTargetRow {
  id: string;
  route_id: string;
  provider_id: string;
  upstream_model?: string | null;
  weight: number;
  created_at: string;
}

export function fetchRoutes(projectId: string): Promise<RouteRow[]> {
  return getJson<RouteRow[]>(`/api/v1/projects/${projectId}/routes`);
}

export function createRoute(
  projectId: string,
  input: { model: string; strategy: string },
): Promise<RouteRow> {
  return sendJson<RouteRow>("POST", `/api/v1/projects/${projectId}/routes`, input);
}

export function setRouteEnabled(id: string, enabled: boolean): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${id}`, { enabled });
}

export function deleteRoute(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/routes/${id}`);
}

export function fetchRouteTargets(routeId: string): Promise<RouteTargetRow[]> {
  return getJson<RouteTargetRow[]>(`/api/v1/routes/${routeId}/targets`);
}

export function createRouteTarget(
  routeId: string,
  input: { provider_id: string; upstream_model?: string; weight?: number },
): Promise<RouteTargetRow> {
  return sendJson<RouteTargetRow>(
    "POST",
    `/api/v1/routes/${routeId}/targets`,
    input,
  );
}

export function deleteRouteTarget(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/route-targets/${id}`);
}

// effective model list — bootstrap-config routes (read-only) merged with
// DB-defined routes (full CRUD), as served to the gateway
export interface EffectiveModelDto {
  model: string;
  strategy: string;
  targets: number;
  source: "config" | "db";
}

export function fetchModels(): Promise<EffectiveModelDto[]> {
  return getJson<EffectiveModelDto[]>("/api/v1/models");
}

export function deleteModel(model: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/models/${encodeURIComponent(model)}`);
}
