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

// bearer token from a real login (see lib/auth.tsx); attached to every request
// so the session-scoped /me/* endpoints authenticate. absent in open-mode /
// email-only sessions, where the control plane needs no auth anyway.
const TOKEN_STORAGE_KEY = "rolter.session.token";

function authHeaders(): Record<string, string> {
  try {
    const token = localStorage.getItem(TOKEN_STORAGE_KEY);
    return token ? { Authorization: `Bearer ${token}` } : {};
  } catch {
    return {};
  }
}

async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: authHeaders() });
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
    headers: {
      ...authHeaders(),
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
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

/// thrown by the analytics fetchers when the control plane has no
/// clickhouse_url configured (503 from crates/rolter-control/src/analytics.rs);
/// callers check for this to render a calm "not configured" empty state
/// instead of a real error banner (reserved for 502s / network failures)
export class AnalyticsUnavailableError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AnalyticsUnavailableError";
  }
}

async function getAnalytics<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: authHeaders() });
  if (!res.ok) {
    const err = await apiError(res);
    if (res.status === 503) {
      throw new AnalyticsUnavailableError(err.message);
    }
    throw err;
  }
  return (await res.json()) as T;
}

export interface AnalyticsWindow {
  since?: string;
  until?: string;
  bucket?: string;
}

function windowParams(window: AnalyticsWindow): string {
  const params = new URLSearchParams();
  if (window.since) params.set("since", window.since);
  if (window.until) params.set("until", window.until);
  if (window.bucket) params.set("bucket", window.bucket);
  const qs = params.toString();
  return qs ? `?${qs}` : "";
}

// ClickHouse JSON rows: numeric columns may come back as strings depending on
// type/format, so callers should coerce with Number(...) when rendering
export interface AnalyticsSummary {
  requests: number | string;
  tokens: number | string;
  prompt_tokens: number | string;
  completion_tokens: number | string;
  cost_usd: number | string;
  errors: number | string;
  avg_latency_ms: number | string;
}

export interface AnalyticsTimeseriesPoint {
  bucket: string;
  requests: number | string;
  tokens: number | string;
  cost_usd: number | string;
}

export interface AnalyticsByModelRow {
  model: string;
  requests: number | string;
  tokens: number | string;
  cost_usd: number | string;
  errors: number | string;
  p50_latency_ms: number | string;
  p95_latency_ms: number | string;
}

export function fetchAnalyticsSummary(
  window: AnalyticsWindow = {},
): Promise<AnalyticsSummary | undefined> {
  return getAnalytics<DataEnvelope<AnalyticsSummary>>(
    `/api/v1/analytics/summary${windowParams(window)}`,
  ).then((r) => r.data[0]);
}

export function fetchAnalyticsTimeseries(
  window: AnalyticsWindow = {},
): Promise<AnalyticsTimeseriesPoint[]> {
  return getAnalytics<DataEnvelope<AnalyticsTimeseriesPoint>>(
    `/api/v1/analytics/timeseries${windowParams(window)}`,
  ).then((r) => r.data);
}

export function fetchAnalyticsByModel(
  window: AnalyticsWindow = {},
): Promise<AnalyticsByModelRow[]> {
  return getAnalytics<DataEnvelope<AnalyticsByModelRow>>(
    `/api/v1/analytics/by-model${windowParams(window)}`,
  ).then((r) => r.data);
}

// one row of the `request_logs` table: a single gateway invocation. numeric
// columns may arrive as strings from ClickHouse JSON, so coerce when rendering.
export interface InvocationRow {
  ts: string;
  request_id: string;
  trace_id: string;
  org_id: string;
  team_id: string;
  project_id: string;
  virtual_key_id: string;
  model: string;
  provider: string;
  target: string;
  variant: string;
  status: number | string;
  stream: number | string;
  cache_hit: number | string;
  cache_read_tokens: number | string;
  cache_write_tokens: number | string;
  prompt_tokens: number | string;
  completion_tokens: number | string;
  total_tokens: number | string;
  cost_usd: number | string;
  latency_ms: number | string;
  ttft_ms: number | string;
  error: string;
  request_payload?: string;
  response_payload?: string;
}

export interface InvocationsQuery extends AnalyticsWindow {
  model?: string;
  key?: string;
  status?: "all" | "error" | "success";
  limit?: number;
  offset?: number;
}

export function fetchInvocations(
  query: InvocationsQuery = {},
): Promise<InvocationRow[]> {
  const params = new URLSearchParams();
  if (query.since) params.set("since", query.since);
  if (query.until) params.set("until", query.until);
  if (query.model) params.set("model", query.model);
  if (query.key) params.set("key", query.key);
  if (query.status) params.set("status", query.status);
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.offset != null) params.set("offset", String(query.offset));
  const qs = params.toString();
  return getAnalytics<DataEnvelope<InvocationRow>>(
    `/api/v1/analytics/invocations${qs ? `?${qs}` : ""}`,
  ).then((r) => r.data);
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
  "gemini",
  "gemini_native",
  "mistral",
  "groq",
  "xai",
  "meta_llama_api",
  "cohere",
  "perplexity",
  "together",
  "fireworks",
  "databricks",
  "aleph_alpha",
  "nebius",
  "ovhcloud",
  "scaleway",
  "deepseek",
  "qwen",
  "zhipu",
  "kimi",
  "ernie",
  "doubao",
  "hunyuan",
  "yi",
  "minimax",
  "baichuan",
  "gigachat",
  "yandex_gpt",
  "cloud_ru",
  "mts_ai",
  "naver",
  "upstage",
  "rinna",
  "rakuten",
  "sarvam",
  "krutrim",
  "falcon",
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

export function createOrg(input: {
  name: string;
  slug: string;
}): Promise<OrgRow> {
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
  return sendJson<ProjectRow>(
    "POST",
    `/api/v1/teams/${teamId}/projects`,
    input,
  );
}

export function deleteProject(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/projects/${id}`);
}

export interface ProviderRow {
  id: string;
  org_id: string;
  name: string;
  /** stable, URL-safe identity used for `provider-slug/model` addressing */
  slug: string;
  kind: string;
  api_base: string;
  api_key_env?: string | null;
  egress_proxy?: string | null;
  created_at: string;
}

export interface CreateProviderInput {
  name: string;
  /** omit to derive a slug from the name; immutable after create */
  slug?: string;
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
  return sendJson<ProviderRow>(
    "POST",
    `/api/v1/orgs/${orgId}/providers`,
    input,
  );
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

// --- provider groups (ADR-0022): unify a fleet of providers behind one
// `group-slug/model` address, balanced by a chosen strategy. the CRUD API
// returns default/DB groups (editable); config-owned readonly groups live only
// in the gateway snapshot and are refused by mutations with a 4xx.

export interface ProviderGroupMember {
  group_id: string;
  provider_id: string;
  provider_name: string;
  /** null = passthrough of the requested model */
  upstream_model?: string | null;
  weight: number;
  position: number;
}

export interface ProviderGroupRow {
  id: string;
  org_id: string;
  name: string;
  /** stable, URL-safe identity used for `group-slug/model` addressing */
  slug: string;
  strategy: string;
  created_at: string;
  members: ProviderGroupMember[];
}

export interface GroupMemberInput {
  provider_id: string;
  /** omit/blank for passthrough of the requested model */
  upstream_model?: string;
  weight?: number;
}

export interface CreateProviderGroupInput {
  name: string;
  /** omit to derive a slug from the name; immutable after create */
  slug?: string;
  strategy: string;
  members: GroupMemberInput[];
}

export interface UpdateProviderGroupInput {
  name?: string;
  slug?: string;
  /** required to change the otherwise-immutable slug */
  allow_slug_change?: boolean;
  strategy?: string;
  /** present = replace the whole membership; omit = leave unchanged */
  members?: GroupMemberInput[];
}

export function fetchProviderGroups(orgId: string): Promise<ProviderGroupRow[]> {
  return getJson<ProviderGroupRow[]>(`/api/v1/orgs/${orgId}/provider-groups`);
}

export function createProviderGroup(
  orgId: string,
  input: CreateProviderGroupInput,
): Promise<ProviderGroupRow> {
  return sendJson<ProviderGroupRow>(
    "POST",
    `/api/v1/orgs/${orgId}/provider-groups`,
    input,
  );
}

export function updateProviderGroup(
  id: string,
  input: UpdateProviderGroupInput,
): Promise<ProviderGroupRow> {
  return sendJson<ProviderGroupRow>("PUT", `/api/v1/provider-groups/${id}`, input);
}

export function deleteProviderGroup(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/provider-groups/${id}`);
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
  return sendJson<RouteRow>(
    "POST",
    `/api/v1/projects/${projectId}/routes`,
    input,
  );
}

export function setRouteEnabled(
  id: string,
  enabled: boolean,
): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${id}`, { enabled });
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
  return sendJson<void>(
    "DELETE",
    `/api/v1/models/${encodeURIComponent(model)}`,
  );
}

// --- virtual keys (crates/rolter-control/src/crud.rs) ---

export interface VirtualKeyRow {
  id: string;
  project_id: string;
  key_hash: string;
  key_prefix: string;
  name?: string | null;
  models: string[];
  disabled: boolean;
  expires_at?: string | null;
  /// per-key response-cache override; null inherits the route decision
  cache_enabled?: boolean | null;
  created_at: string;
}

// returned only from createVirtualKey — carries the plaintext secret, shown
// once and never persisted beyond the create mutation's immediate result
export interface CreatedVirtualKey extends VirtualKeyRow {
  key: string;
}

export interface CreateVirtualKeyInput {
  name?: string;
  models?: string[];
  cache?: boolean | null;
}

export function fetchVirtualKeys(projectId: string): Promise<VirtualKeyRow[]> {
  return getJson<VirtualKeyRow[]>(`/api/v1/projects/${projectId}/virtual-keys`);
}

export function createVirtualKey(
  projectId: string,
  input: CreateVirtualKeyInput,
): Promise<CreatedVirtualKey> {
  return sendJson<CreatedVirtualKey>(
    "POST",
    `/api/v1/projects/${projectId}/virtual-keys`,
    input,
  );
}

export function setVirtualKeyDisabled(
  id: string,
  disabled: boolean,
): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>("PUT", `/api/v1/virtual-keys/${id}`, {
    disabled,
  });
}

export function setVirtualKeyCache(
  id: string,
  cache: boolean | null,
): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>("PUT", `/api/v1/virtual-keys/${id}/cache`, {
    cache,
  });
}

export function deleteVirtualKey(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/virtual-keys/${id}`);
}

// --- budgets, rate limits, model pricing (crates/rolter-control/src/crud.rs) ---

export const SCOPE_TYPES = ["org", "team", "project", "virtual_key"] as const;

export interface BudgetRow {
  id: string;
  scope_type: string;
  scope_id: string;
  /// decimal, returned as text
  limit_usd: string;
  period: string;
  created_at: string;
}

export interface CreateBudgetInput {
  scope_type: string;
  scope_id: string;
  limit_usd: string;
  period?: string;
}

export function fetchBudgets(
  scopeType: string,
  scopeId: string,
): Promise<BudgetRow[]> {
  return getJson<BudgetRow[]>(
    `/api/v1/budgets?scope_type=${encodeURIComponent(scopeType)}&scope_id=${encodeURIComponent(scopeId)}`,
  );
}

export function createBudget(input: CreateBudgetInput): Promise<BudgetRow> {
  return sendJson<BudgetRow>("POST", "/api/v1/budgets", input);
}

export function deleteBudget(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/budgets/${id}`);
}

export interface RateLimitRow {
  id: string;
  scope_type: string;
  scope_id: string;
  rpm?: number | null;
  tpm?: number | null;
  created_at: string;
}

export interface CreateRateLimitInput {
  scope_type: string;
  scope_id: string;
  rpm?: number;
  tpm?: number;
}

export function fetchRateLimits(
  scopeType: string,
  scopeId: string,
): Promise<RateLimitRow[]> {
  return getJson<RateLimitRow[]>(
    `/api/v1/rate-limits?scope_type=${encodeURIComponent(scopeType)}&scope_id=${encodeURIComponent(scopeId)}`,
  );
}

export function createRateLimit(
  input: CreateRateLimitInput,
): Promise<RateLimitRow> {
  return sendJson<RateLimitRow>("POST", "/api/v1/rate-limits", input);
}

export function deleteRateLimit(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/rate-limits/${id}`);
}

export interface ModelPriceRow {
  id: string;
  model: string;
  /// decimal, returned as text
  input_per_mtok: string;
  output_per_mtok: string;
  cached_input_per_mtok?: string | null;
  currency: string;
  created_at: string;
}

export interface UpsertModelPriceInput {
  model: string;
  input_per_mtok: string;
  output_per_mtok: string;
  cached_input_per_mtok?: string;
  currency?: string;
}

export function fetchModelPrices(): Promise<ModelPriceRow[]> {
  return getJson<ModelPriceRow[]>("/api/v1/model-prices");
}

export function upsertModelPrice(
  input: UpsertModelPriceInput,
): Promise<ModelPriceRow> {
  return sendJson<ModelPriceRow>("PUT", "/api/v1/model-prices", input);
}

export function deleteModelPrice(model: string): Promise<void> {
  return sendJson<void>(
    "DELETE",
    `/api/v1/model-prices/${encodeURIComponent(model)}`,
  );
}

// --- local-account auth (crates/rolter-control/src/auth.rs, ROL-32) ---

export interface LoginResponse {
  /** opaque bearer token; store it and send as Authorization: Bearer */
  token: string;
  expires_at: string;
  user: { id: string; email: string; is_superadmin: boolean };
}

// authenticate a local account; returns a session token. rejects (throws) on
// bad credentials or when local accounts aren't configured.
export function login(email: string, password: string): Promise<LoginResponse> {
  return sendJson<LoginResponse>("POST", "/api/v1/auth/login", {
    email,
    password,
  });
}

export function logout(): Promise<void> {
  return sendJson<void>("POST", "/api/v1/auth/logout");
}

// --- users + memberships (crates/rolter-control/src/crud.rs, ROL-223) ---

export const ROLES = ["admin", "member", "viewer"] as const;
export type Role = (typeof ROLES)[number];

// scope types a membership can be granted at (virtual_key is not a role scope)
export const MEMBERSHIP_SCOPE_TYPES = ["org", "team", "project"] as const;
export type MembershipScopeType = (typeof MEMBERSHIP_SCOPE_TYPES)[number];

export interface UserRow {
  id: string;
  email: string;
  is_superadmin: boolean;
  /** set when the account is deactivated (login blocked); null when active */
  deactivated_at?: string | null;
  created_at: string;
}

export interface MembershipRow {
  id: string;
  user_id: string;
  org_id?: string | null;
  team_id?: string | null;
  project_id?: string | null;
  role: string;
  created_at: string;
}

// returned by inviteUser: the new account plus its initial org membership
export interface CreatedUser {
  user: UserRow;
  membership: MembershipRow;
}

export interface InviteUserInput {
  email: string;
  /** optional initial password; omit for an sso-only shell account */
  password?: string;
  /** role granted at the org; defaults to member */
  role?: string;
}

export interface UpdateUserInput {
  email?: string;
  password?: string;
  is_superadmin?: boolean;
  deactivated?: boolean;
}

export interface CreateMembershipInput {
  user_id: string;
  scope_type: MembershipScopeType;
  scope_id: string;
  role: string;
}

// every account with a membership anywhere in the org's tree
export function fetchUsers(orgId: string): Promise<UserRow[]> {
  return getJson<UserRow[]>(`/api/v1/orgs/${orgId}/users`);
}

// create/invite an account and grant it a role in the org atomically
export function inviteUser(
  orgId: string,
  input: InviteUserInput,
): Promise<CreatedUser> {
  return sendJson<CreatedUser>("POST", `/api/v1/orgs/${orgId}/users`, input);
}

export function updateUser(
  id: string,
  input: UpdateUserInput,
): Promise<UserRow> {
  return sendJson<UserRow>("PUT", `/api/v1/users/${id}`, input);
}

export function deleteUser(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/users/${id}`);
}

// every role grant scoped within the org (org/team/project)
export function fetchMemberships(orgId: string): Promise<MembershipRow[]> {
  return getJson<MembershipRow[]>(`/api/v1/orgs/${orgId}/memberships`);
}

export function createMembership(
  orgId: string,
  input: CreateMembershipInput,
): Promise<MembershipRow> {
  return sendJson<MembershipRow>(
    "POST",
    `/api/v1/orgs/${orgId}/memberships`,
    input,
  );
}

export function deleteMembership(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/memberships/${id}`);
}

// --- self-service (crates/rolter-control/src/me.rs, ROL-224) ---
//
// end-user surface: manage your own virtual keys and see your own usage. these
// require a real login session (not the admin token path).

// a key the current user owns, enriched with its project/org names; never
// carries the key hash
export interface OwnedKeyRow {
  id: string;
  project_id: string;
  project_name: string;
  org_name: string;
  key_prefix: string;
  name?: string | null;
  models: string[];
  disabled: boolean;
  expires_at?: string | null;
  created_at: string;
}

// returned from mint/rotate — carries the plaintext secret, shown once
export interface MintedKey extends VirtualKeyRow {
  key: string;
}

export interface MintKeyInput {
  name?: string;
  models?: string[];
  cache?: boolean | null;
}

export interface MyUsageRow {
  virtual_key_id: string;
  requests: number | string;
  tokens: number | string;
  cost_usd: number | string;
  errors: number | string;
}

export function fetchMyKeys(): Promise<OwnedKeyRow[]> {
  return getJson<OwnedKeyRow[]>("/api/v1/me/virtual-keys");
}

export function mintMyKey(
  projectId: string,
  input: MintKeyInput,
): Promise<MintedKey> {
  return sendJson<MintedKey>(
    "POST",
    `/api/v1/me/projects/${projectId}/virtual-keys`,
    input,
  );
}

export function rotateMyKey(id: string): Promise<MintedKey> {
  return sendJson<MintedKey>("POST", `/api/v1/me/virtual-keys/${id}/rotate`);
}

export function deleteMyKey(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/me/virtual-keys/${id}`);
}

// per-key usage/spend over the window; throws AnalyticsUnavailableError (503)
// when the deployment has no ClickHouse configured
export function fetchMyUsage(
  window: AnalyticsWindow = {},
): Promise<MyUsageRow[]> {
  return getAnalytics<DataEnvelope<MyUsageRow>>(
    `/api/v1/me/usage${windowParams(window)}`,
  ).then((r) => r.data);
}

export interface AuditLogEntry {
  id: string;
  org_id?: string | null;
  actor_user_id?: string | null;
  action: string;
  target_type?: string | null;
  target_id?: string | null;
  detail?: unknown;
  at: string;
}

export interface AuditLogPage {
  items: AuditLogEntry[];
  next_cursor: string | null;
  previous_cursor: string | null;
  has_next: boolean;
  has_previous: boolean;
  total?: number;
}

export interface AuditLogQuery {
  limit?: number;
  cursor?: string;
  direction?: "next" | "previous";
  actor?: string;
  action?: string;
  target_type?: string;
  from?: string;
  to?: string;
  include_total?: boolean;
}

export function fetchAuditLogPage(
  orgId: string,
  query: AuditLogQuery = {},
): Promise<AuditLogPage> {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== "") {
      params.set(key, String(value));
    }
  }
  const qs = params.toString();
  return getJson<AuditLogPage>(
    `/api/v1/orgs/${orgId}/audit-log${qs ? `?${qs}` : ""}`,
  );
}

// ---------------------------------------------------------------------------
// security settings (superadmin-only global gateway policy)

export interface SecuritySettingsDto {
  virtual_key_required: boolean;
  allow_direct_provider_keys: boolean;
  allowed_origins: string[];
  allowed_headers: string[];
  required_headers: Record<string, string>;
  auth_bypass_routes: string[];
  dashboard_auth_enabled: boolean;
  dashboard_credential_ref: string | null;
  dashboard_secret_configured: boolean;
  updated_at: string;
}

export interface UpdateSecuritySettingsInput {
  virtual_key_required: boolean;
  allow_direct_provider_keys: boolean;
  allowed_origins: string[];
  allowed_headers: string[];
  required_headers: Record<string, string>;
  auth_bypass_routes: string[];
  dashboard_auth_enabled: boolean;
  dashboard_credential_ref?: string | null;
  /// write-only; sealed server-side, never echoed back
  managed_dashboard_secret?: string;
}

export function fetchSecuritySettings(): Promise<SecuritySettingsDto> {
  return getJson<SecuritySettingsDto>("/api/v1/security-settings");
}

export function updateSecuritySettings(
  input: UpdateSecuritySettingsInput,
): Promise<SecuritySettingsDto> {
  return sendJson<SecuritySettingsDto>("PUT", "/api/v1/security-settings", input);
}

// ---------------------------------------------------------------------------
// alerting: channels, rules, notification history

export const ALERT_SIGNALS = [
  "error_rate",
  "p95_latency_ms",
  "spend_velocity",
  "request_volume",
  "provider_health_flaps",
] as const;

export interface AlertChannelRow {
  id: string;
  name: string;
  kind: string;
  endpoint: string;
  enabled: boolean;
  secret_configured: boolean;
  created_at: string;
  updated_at: string;
}

export interface AlertChannelInput {
  name: string;
  endpoint: string;
  enabled: boolean;
  managed_secret?: string;
}

export interface AlertRuleRow {
  id: string;
  name: string;
  signal: string;
  threshold: number;
  window_secs: number;
  channel_id: string | null;
  enabled: boolean;
  state: string;
  last_value: number | null;
  last_evaluated_at: string | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

export interface AlertRuleInput {
  name: string;
  signal: string;
  threshold: number;
  window_secs: number;
  channel_id?: string | null;
  enabled: boolean;
}

export interface AlertNotificationRow {
  id: string;
  rule_id: string;
  channel_id: string | null;
  state: string;
  delivery_status: string;
  detail: string | null;
  sent_at: string;
}

export function fetchAlertChannels(): Promise<AlertChannelRow[]> {
  return getJson<AlertChannelRow[]>("/api/v1/alert-channels");
}

export function createAlertChannel(
  input: AlertChannelInput,
): Promise<AlertChannelRow> {
  return sendJson<AlertChannelRow>("POST", "/api/v1/alert-channels", input);
}

export function updateAlertChannel(
  id: string,
  input: AlertChannelInput,
): Promise<AlertChannelRow> {
  return sendJson<AlertChannelRow>("PUT", `/api/v1/alert-channels/${id}`, input);
}

export function deleteAlertChannel(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/alert-channels/${id}`);
}

export function fetchAlertRules(): Promise<AlertRuleRow[]> {
  return getJson<AlertRuleRow[]>("/api/v1/alert-rules");
}

export function createAlertRule(input: AlertRuleInput): Promise<AlertRuleRow> {
  return sendJson<AlertRuleRow>("POST", "/api/v1/alert-rules", input);
}

export function updateAlertRule(
  id: string,
  input: AlertRuleInput,
): Promise<AlertRuleRow> {
  return sendJson<AlertRuleRow>("PUT", `/api/v1/alert-rules/${id}`, input);
}

export function deleteAlertRule(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/alert-rules/${id}`);
}

export function evaluateAlertRule(
  id: string,
): Promise<{ rule: AlertRuleRow; notified: boolean }> {
  return sendJson<{ rule: AlertRuleRow; notified: boolean }>(
    "POST",
    `/api/v1/alert-rules/${id}/evaluate`,
  );
}

export function fetchAlertHistory(
  limit = 100,
  ruleId?: string,
): Promise<AlertNotificationRow[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (ruleId) params.set("rule_id", ruleId);
  return getJson<AlertNotificationRow[]>(
    `/api/v1/alert-notifications?${params}`,
  );
}

// ---------------------------------------------------------------------------
// observability connectors (OTLP log shipping)

export interface ConnectorRow {
  id: string;
  name: string;
  kind: string;
  endpoint: string;
  enabled: boolean;
  sampling_rate: number;
  auth_secret_ref: string | null;
  auth_secret_configured: boolean;
  health_status: string;
  health_checked_at: string | null;
  health_error: string | null;
  created_at: string;
  updated_at: string;
}

export interface ConnectorInput {
  name: string;
  kind: "otlp_http";
  endpoint: string;
  enabled: boolean;
  sampling_rate: number;
  auth_secret_ref?: string | null;
  /// write-only bearer token, sealed before persistence
  managed_auth_secret?: string;
}

export function fetchConnectors(): Promise<ConnectorRow[]> {
  return getJson<ConnectorRow[]>("/api/v1/connectors");
}

export function createConnector(input: ConnectorInput): Promise<ConnectorRow> {
  return sendJson<ConnectorRow>("POST", "/api/v1/connectors", input);
}

export function updateConnector(
  id: string,
  input: ConnectorInput,
): Promise<ConnectorRow> {
  return sendJson<ConnectorRow>("PUT", `/api/v1/connectors/${id}`, input);
}

export function deleteConnector(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/connectors/${id}`);
}

export function testConnector(id: string): Promise<{
  delivered: boolean;
  health_status: string;
  health_checked_at: string;
}> {
  return sendJson<{
    delivered: boolean;
    health_status: string;
    health_checked_at: string;
  }>("POST", `/api/v1/connectors/${id}/test`);
}

// ---------------------------------------------------------------------------
// mcp tool-call logs (clickhouse-backed; 503 → AnalyticsUnavailableError)

export const MCP_TRANSPORTS = ["stdio", "streamable_http", "sse"] as const;
export const MCP_STATUSES = [
  "success",
  "error",
  "timeout",
  "auth_denied",
  "transport_error",
] as const;

export interface McpLogRow {
  ts: string;
  event_id: string;
  server: string;
  tool: string;
  transport: string;
  status: string;
  latency_ms: number;
  org_id: string;
  team_id: string;
  project_id: string;
  virtual_key_id: string;
  user_id: string;
  request_id: string;
  trace_id: string;
  error: string | null;
}

export interface McpLogDetail extends McpLogRow {
  arguments: string | null;
  result: string | null;
}

export interface McpLogsQuery extends AnalyticsWindow {
  server?: string;
  tool?: string;
  transport?: string;
  status?: string;
  key?: string;
  user?: string;
  limit?: number;
  cursor?: string;
}

export interface McpSummaryRow {
  calls: string | number;
  failures: string | number;
  avg_latency_ms: number;
  p95_latency_ms: number;
}

export function fetchMcpLogs(
  query: McpLogsQuery = {},
): Promise<{ data: McpLogRow[]; next_cursor: string | null }> {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== "") {
      params.set(key, String(value));
    }
  }
  const qs = params.toString();
  return getAnalytics<{ data: McpLogRow[]; next_cursor: string | null }>(
    `/api/v1/mcp/logs${qs ? `?${qs}` : ""}`,
  );
}

export function fetchMcpSummary(
  window: AnalyticsWindow = {},
): Promise<McpSummaryRow | undefined> {
  return getAnalytics<{ data: McpSummaryRow[] }>(
    `/api/v1/mcp/logs/summary${windowParams(window)}`,
  ).then((r) => r.data[0]);
}

export function fetchMcpLogDetail(eventId: string): Promise<McpLogDetail> {
  return getAnalytics<McpLogDetail>(
    `/api/v1/mcp/logs/${encodeURIComponent(eventId)}`,
  );
}

// ---------------------------------------------------------------------------
// complexity routing policy (stored in route params, validated server-side)

export interface ComplexityTier {
  name: string;
  /// inclusive byte ceiling; null marks the final catch-all tier
  max_input_bytes: number | null;
  route: string;
}

export interface ComplexityPolicy {
  tiers: ComplexityTier[];
}

export function fetchRouteComplexity(
  routeId: string,
): Promise<ComplexityPolicy> {
  return getJson<ComplexityPolicy>(`/api/v1/routes/${routeId}/complexity`);
}

export function setRouteComplexity(
  routeId: string,
  policy: ComplexityPolicy,
): Promise<RouteRow> {
  return sendJson<RouteRow>(
    "PUT",
    `/api/v1/routes/${routeId}/complexity`,
    policy,
  );
}

// advanced per-route model configuration (base_url, pricing, limits, headers…)
export function setRouteAdvanced(
  routeId: string,
  advanced: Record<string, unknown>,
): Promise<RouteRow> {
  return sendJson<RouteRow>("PUT", `/api/v1/routes/${routeId}/advanced`, {
    advanced,
  });
}
