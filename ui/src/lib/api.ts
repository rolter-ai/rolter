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
  /**
   * every other section of `GatewayConfig` (`crates/rolter-core/src/config.rs`)
   * — server, tls, cache, guardrails, feature_flags, … — which the Effective
   * config screen renders generically rather than typing one by one (#1204)
   */
  [section: string]: unknown;
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

export class ApiError extends Error {
  readonly status: number;
  /**
   * Stable machine-readable reason from the control plane's
   * `{"error": {"code": ...}}` body, when it sent one. Screens branch on this
   * rather than on the message, which is free to be reworded and translated.
   */
  readonly code?: string;
  /**
   * Seconds the caller was told to wait, from the response's `Retry-After`
   * header. Present on a `429` — a lock is a clock, and a screen that can read
   * it does not have to make the user poll to find out (#1079).
   */
  readonly retryAfterSeconds?: number;

  constructor(
    message: string,
    status: number,
    code?: string,
    retryAfterSeconds?: number,
  ) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.retryAfterSeconds = retryAfterSeconds;
  }
}

/**
 * The control plane is running with no admin token, so it has no local accounts
 * and the per-user `/me/*` endpoints cannot be served to anyone (#942).
 *
 * A plain 401 means "sign in"; this one cannot be fixed that way, so the two
 * must not render alike.
 */
export function isOpenModeNoSession(error: unknown): boolean {
  return error instanceof ApiError && error.code === "open_mode_no_session";
}

// the shell's "this token is dead" handler, registered by AuthProvider (#1196).
// a module-level callback rather than an import so api.ts keeps knowing nothing
// about react, and so a request that 401s in a screen nobody is watching still
// signs the session out once, centrally, instead of leaving every screen to
// reach LoadError with the same dead token still attached
type SessionExpiredHandler = () => void;
let sessionExpiredHandler: SessionExpiredHandler | null = null;

/**
 * Register the handler called when a request made *with a session token* is
 * answered 401. Returns an unsubscribe, so a provider can drop it on unmount.
 */
export function setSessionExpiredHandler(
  handler: SessionExpiredHandler | null,
): () => void {
  sessionExpiredHandler = handler;
  return () => {
    if (sessionExpiredHandler === handler) sessionExpiredHandler = null;
  };
}

// endpoints whose 401 is about the credentials in the request, not about the
// session token that happens to be in localStorage: a wrong password and an
// invite token that expired are both answered 401, and neither means the
// current session died
const SESSION_EXEMPT_PATHS = [
  "/api/v1/auth/login",
  "/api/v1/invitations/accept/",
];

/// signal a dead session, but only for the failures that actually are one
function noteUnauthorized(url: string, authed: boolean, err: ApiError) {
  if (!authed || err.status !== 401) return;
  // open mode has no accounts at all, so there is no session to have expired
  // and no sign-in that would help (#942)
  if (isOpenModeNoSession(err)) return;
  if (SESSION_EXEMPT_PATHS.some((path) => url.includes(path))) return;
  sessionExpiredHandler?.();
}

/**
 * The control plane answered, and the answer was "this endpoint is not
 * mounted here": every CRUD and settings route only exists when it runs with a
 * database (`--database-url`). A config-file-only deployment therefore 404s
 * Users, Keys, Providers and every settings screen, which is a deployment
 * shape, not a bug in the request (#1204).
 *
 * Newer control planes send `code: no_such_endpoint`; older ones only the
 * message, which is matched on its prefix so the screen still explains itself.
 */
export function isEndpointNotMounted(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 404 &&
    (error.code === "no_such_endpoint" || error.message.startsWith("no such endpoint"))
  );
}

async function getJson<T>(url: string): Promise<T> {
  const headers = authHeaders();
  const res = await fetch(url, { headers });
  if (!res.ok) {
    const err = await apiError(res);
    noteUnauthorized(url, "Authorization" in headers, err);
    throw err;
  }
  return (await res.json()) as T;
}

/// the same request as `getJson` for an endpoint that answers with a document
/// rather than a record — the collector config is `application/yaml`, and
/// parsing it would only be a step towards printing it again
async function getText(url: string): Promise<string> {
  const headers = authHeaders();
  const res = await fetch(url, { headers });
  if (!res.ok) {
    const err = await apiError(res);
    noteUnauthorized(url, "Authorization" in headers, err);
    throw err;
  }
  return res.text();
}

/// extract the control api's `{"error": {"message": ..., "code": ...}}` body
/// when present, falling back to the raw status text
async function apiError(res: Response): Promise<ApiError> {
  // only the delta-seconds form is sent by rolter. the http-date form is legal
  // and unhandled, and an absent header reads as 0 through `Number` — both are
  // dropped rather than rendered, since "retry in NaN seconds" and "retry in 0
  // seconds" are each worse than saying nothing
  const raw = res.headers.get("retry-after");
  const seconds = raw === null ? Number.NaN : Number(raw);
  const retryAfter = Number.isFinite(seconds) && seconds > 0 ? seconds : undefined;
  try {
    const body = (await res.json()) as {
      error?: { message?: string; code?: string };
    };
    if (body?.error?.message) {
      return new ApiError(
        body.error.message,
        res.status,
        body.error.code,
        retryAfter,
      );
    }
  } catch {
    // not json, fall through
  }
  return new ApiError(
    `request failed: ${res.status}`,
    res.status,
    undefined,
    retryAfter,
  );
}

async function sendJson<T>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body?: unknown,
): Promise<T> {
  const auth = authHeaders();
  const res = await fetch(url, {
    method,
    headers: {
      ...auth,
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!res.ok) {
    const err = await apiError(res);
    noteUnauthorized(url, "Authorization" in auth, err);
    throw err;
  }
  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

/// thrown by the analytics fetchers when this deployment cannot serve
/// analytics at all — either the control plane has no clickhouse_url
/// configured (503 from crates/rolter-control/src/analytics.rs) or the
/// endpoint does not exist on this binary (404, an older control plane).
/// Both are deployment shape, not failure, so callers render a calm empty
/// state instead of an error banner (reserved for 5xx / network failures).
export class AnalyticsUnavailableError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AnalyticsUnavailableError";
  }
}

async function getAnalytics<T>(
  url: string,
  // a 404 on a collection endpoint means the route is absent from this
  // binary; on a by-id endpoint it means that one record is missing, which
  // is a genuine not-found the caller should surface as such
  { notFoundIsUnavailable = true }: { notFoundIsUnavailable?: boolean } = {},
): Promise<T> {
  const res = await fetch(url, { headers: authHeaders() });
  if (!res.ok) {
    const err = await apiError(res);
    if (res.status === 503 || (res.status === 404 && notFoundIsUnavailable)) {
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
  /**
   * Requests in this window that ran against a model with no price row, so
   * `cost_usd` excludes them entirely (#969). A non-zero value means the spend
   * figure is a floor, not a total.
   */
  unpriced_requests: number | string;
  /** how many distinct models those requests hit */
  unpriced_models: number | string;
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
  /** requests recorded with no price for this model; see AnalyticsSummary */
  unpriced_requests: number | string;
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

/** the governance dimension `by-attribution` groups on */
export type AttributionDimension = "business_unit" | "customer";

/**
 * One bucket of the cost-attribution rollup. `id` is the business unit's or
 * customer's uuid, and is the empty string for the unattributed bucket — the
 * spend recorded against keys nobody pointed at a unit or a customer.
 */
export interface AttributionSpendRow {
  id: string;
  requests: number | string;
  tokens: number | string;
  prompt_tokens: number | string;
  completion_tokens: number | string;
  cost_usd: number | string;
  errors: number | string;
}

/**
 * Spend and usage grouped by business unit or customer over the window.
 *
 * The unattributed bucket is off by default on the server, because a chargeback
 * report is skewed by an empty row. The dashboard asks for it anyway: a unit
 * list that adds up to less than the deployment spent is a report with a hole
 * in it, and the hole is the thing worth seeing.
 */
export function fetchAttributionSpend(
  window: AnalyticsWindow = {},
  dimension: AttributionDimension = "business_unit",
  includeUnattributed = true,
): Promise<AttributionSpendRow[]> {
  const params = new URLSearchParams();
  if (window.since) params.set("since", window.since);
  if (window.until) params.set("until", window.until);
  params.set("dimension", dimension);
  if (includeUnattributed) params.set("include_unattributed", "true");
  return getAnalytics<DataEnvelope<AttributionSpendRow>>(
    `/api/v1/analytics/by-attribution?${params.toString()}`,
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
  /// governance dimensions the key is attributed to. both are clickhouse
  /// `String default ''`, so an unattributed request reads "" and never null
  business_unit_id: string;
  customer_id: string;
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
  /** business unit ids to narrow to; omitted means every unit (#1247) */
  business_unit?: string[];
  /** customer ids to narrow to; omitted means every customer */
  customer?: string[];
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
  // the control plane splits these on commas, so a set travels as one param
  if (query.business_unit?.length)
    params.set("business_unit", query.business_unit.join(","));
  if (query.customer?.length) params.set("customer", query.customer.join(","));
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

/**
 * Config entries the control plane is not serving to gateways, and why (#926).
 *
 * A malformed provider is dropped from the snapshot rather than withholding the
 * whole fleet's config, so without this nothing would say it stopped being
 * served — every gateway keeps running its last good config and the change
 * simply never takes effect.
 */
export function fetchConfigProblems(): Promise<string[]> {
  return getJson<{ problems: string[] }>("/api/v1/config/problems").then(
    (r) => r.problems,
  );
}

export function fetchRoles(): Promise<string[]> {
  return getJson<string[]>("/api/v1/roles");
}

// provider stability rollups over provider_health_events (ROL-198)

/**
 * Which grain a health rollup row describes: `provider` for probe and
 * status-page observations, which watch the provider as a whole, `target` for
 * passive observations of one route through it. The two are never summed —
 * they count different things — so the screen nests targets under a provider
 * instead of laying them out as peers (#1257).
 */
export type HealthGrain = "provider" | "target";

export interface UptimeRow {
  provider: string;
  target_id: string;
  grain: HealthGrain;
  /** the distinct event sources behind this row, e.g. `["passive", "probe"]` */
  sources: string[];
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
  grain: HealthGrain;
  mttr_seconds: number;
  incidents: number;
}

export interface TimelineRow {
  bucket: string;
  provider: string;
  target_id: string;
  grain: HealthGrain;
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

/**
 * Whether `api_base` must already end in the API version prefix, per kind.
 *
 * Served by the control plane rather than restated here: the rule covers ~38
 * of 48 kinds, and a copy in TypeScript would drift the moment a kind is added
 * (#947).
 */
export interface ProviderKindInfo {
  kind: string;
  base_includes_v1: boolean;
}

export function fetchProviderKinds(): Promise<ProviderKindInfo[]> {
  return getJson<ProviderKindInfo[]>("/api/v1/provider-kinds");
}

/**
 * The upstream URL a gateway request resolves to, mirroring
 * `ProviderKind::resolve_upstream_url` in rolter-core.
 *
 * A Rust test (`every_kind_resolves_by_the_documented_rule`) pins the backend
 * to exactly this rule, so the preview shown while typing is the URL that will
 * actually be called.
 */
export function resolveUpstreamUrl(
  apiBase: string,
  baseIncludesV1: boolean,
  path = "/v1/chat/completions",
): string {
  const base = apiBase.trim().replace(/\/+$/, "");
  if (!base) return "";
  return baseIncludesV1 ? base + path.replace(/^\/v1/, "") : base + path;
}

/**
 * The doubled-prefix defect, or null when the base is well formed.
 *
 * Only a defect for kinds that append `/v1` themselves; for the rest a trailing
 * `/v1` is required. Mirrors `ProviderKind::api_base_problem`.
 */
export function apiBaseDoublesV1(apiBase: string, baseIncludesV1: boolean): boolean {
  const base = apiBase.trim().replace(/\/+$/, "");
  return !!base && !baseIncludesV1 && base.endsWith("/v1");
}

/**
 * Every provider kind the control plane accepts, in the order of the
 * `PROVIDER_KINDS` allowlist in `crates/rolter-control/src/crud.rs`.
 *
 * A fallback and a type, not the menu: `ProviderSheet` builds its picker from
 * `fetchProviderKinds()` so a kind the deployment gained is selectable without
 * a dashboard release. This list is what it offers when the control plane
 * cannot answer.
 */
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
  "gemini_interactions",
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

/**
 * Every balancing strategy the control plane accepts, in the order of the
 * `STRATEGIES` allowlist in `crates/rolter-control/src/crud.rs` (#897).
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

// ---------------------------------------------------------------------------
// cost attribution: business units roll teams up, customers attribute spend to
// the org's own customers. both are retired rather than deleted once they have
// history, so `retired_at` is part of the row, not a separate lookup

export interface BusinessUnitRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  retired_at: string | null;
  created_at: string;
}

export interface CustomerRow {
  id: string;
  org_id: string;
  business_unit_id: string | null;
  name: string;
  slug: string;
  retired_at: string | null;
  created_at: string;
}

// ---------------------------------------------------------------------------
// org-defined roles (#534, crates/rolter-control/src/access_control.rs): a base
// role plus the explicit `(resource, action)` pairs it also allows. A grant only
// ever widens what the base role already permitted, and none can reach a
// superadmin capability — deployment-wide policy has no org to define a role in,
// so granting it from inside a tenant would be a way out of that tenant.

export interface CustomRoleRow {
  id: string;
  org_id: string;
  slug: string;
  name: string;
  description: string | null;
  /** the built-in role this one extends: `admin` | `member` | `viewer` */
  base_role: Role;
  created_at: string;
  updated_at: string;
}

/** one stored `(resource, action)` pair a custom role grants */
export interface CustomRoleGrantRow {
  id: string;
  role_id: string;
  resource: string;
  action: RbacAction;
}

/**
 * What every custom-role read and write returns.
 *
 * The role is `#[serde(flatten)]`ed on the rust side, so its own fields sit
 * beside `grants` rather than under a `role` key.
 */
export interface CustomRoleDetail extends CustomRoleRow {
  grants: CustomRoleGrantRow[];
}

/** a grant as it is written; the server assigns the id */
export interface CustomRoleGrantInput {
  resource: string;
  action: RbacAction;
}

export function fetchCustomRoles(orgId: string): Promise<CustomRoleRow[]> {
  return getJson<CustomRoleRow[]>(`/api/v1/orgs/${orgId}/custom-roles`);
}

export function createCustomRole(
  orgId: string,
  input: {
    name: string;
    slug?: string;
    description?: string | null;
    /** omitted defaults to `viewer` server-side */
    base_role?: Role;
    grants?: CustomRoleGrantInput[];
  },
): Promise<CustomRoleDetail> {
  return sendJson<CustomRoleDetail>(
    "POST",
    `/api/v1/orgs/${orgId}/custom-roles`,
    input,
  );
}

// `grants` absent leaves the stored set alone; present replaces it wholesale, so
// an editor that sends a grid has to send every pair it means to keep — including
// the ones this build's capability table no longer defines
export function updateCustomRole(
  id: string,
  input: {
    name?: string;
    description?: string | null;
    base_role?: Role;
    grants?: CustomRoleGrantInput[];
  },
): Promise<CustomRoleDetail> {
  return sendJson<CustomRoleDetail>("PUT", `/api/v1/custom-roles/${id}`, input);
}

// 409 for as long as any access profile still composes the role: detaching is an
// audited profile update, so nobody's access changes without a record that names
// the profile it changed
export function deleteCustomRole(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/custom-roles/${id}`);
}

// ---------------------------------------------------------------------------
// access profiles (#534 / #830): reusable permission bundles handed to users or
// whole teams. the merged model/route policy they carry is enforced on the
// gateway, not just reported — see ADR-0023

export interface AccessProfileRow {
  id: string;
  org_id: string;
  slug: string;
  name: string;
  description: string | null;
  created_at: string;
  updated_at: string;
}

export interface AccessProfilePolicy {
  profile_id: string;
  allowed_models: string[];
  denied_models: string[];
  allowed_routes: string[];
  denied_routes: string[];
  updated_at: string;
}

export interface AccessProfileAssignmentRow {
  id: string;
  profile_id: string;
  user_id: string | null;
  team_id: string | null;
  created_at: string;
}

/**
 * One `(custom role, scope)` pair inside a profile.
 *
 * Scope follows the same most-specific-non-null convention as a membership: a
 * project pins the role to that project, a team to every project under it, and
 * an org to the whole tenant.
 */
export interface AccessProfileRoleRow {
  id: string;
  profile_id: string;
  role_id: string;
  org_id: string | null;
  team_id: string | null;
  project_id: string | null;
  created_at: string;
}

/** the profile plus everything hanging off it, `#[serde(flatten)]`ed as above */
export interface AccessProfileDetail extends AccessProfileRow {
  roles: AccessProfileRoleRow[];
  assignments: AccessProfileAssignmentRow[];
  /** null until a policy has been written for the profile */
  policy: AccessProfilePolicy | null;
}

/** a composition as it is written: a role, and the scope it applies at */
export interface AccessProfileRoleInput {
  role_id: string;
  org_id?: string;
  team_id?: string;
  project_id?: string;
}

export interface AccessProfilePolicyInput {
  allowed_models: string[];
  denied_models: string[];
  allowed_routes: string[];
  denied_routes: string[];
}

export function fetchAccessProfiles(
  orgId: string,
): Promise<AccessProfileRow[]> {
  return getJson<AccessProfileRow[]>(`/api/v1/orgs/${orgId}/access-profiles`);
}

// the one call that answers "what does this profile actually carry": the roles
// composed into it, everyone it reaches, and the policy `setAccessProfilePolicy`
// wrote — none of which the list endpoint can show back
export function fetchAccessProfile(id: string): Promise<AccessProfileDetail> {
  return getJson<AccessProfileDetail>(`/api/v1/access-profiles/${id}`);
}

// a profile is created whole: the roles it composes and the policy it carries go
// in the same request, so it is never assignable in a half-written state
export function createAccessProfile(
  orgId: string,
  input: {
    name: string;
    slug?: string;
    description?: string | null;
    roles?: AccessProfileRoleInput[];
    policy?: AccessProfilePolicyInput;
  },
): Promise<AccessProfileDetail> {
  return sendJson<AccessProfileDetail>(
    "POST",
    `/api/v1/orgs/${orgId}/access-profiles`,
    input,
  );
}

// `roles` absent leaves the composition alone, present replaces it wholesale —
// which is also how a role is detached before it can be deleted
export function updateAccessProfile(
  id: string,
  input: {
    name?: string;
    description?: string | null;
    roles?: AccessProfileRoleInput[];
    policy?: AccessProfilePolicyInput;
  },
): Promise<AccessProfileDetail> {
  return sendJson<AccessProfileDetail>(
    "PUT",
    `/api/v1/access-profiles/${id}`,
    input,
  );
}

export function deleteAccessProfile(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/access-profiles/${id}`);
}

export function fetchAccessProfileAssignments(
  id: string,
): Promise<AccessProfileAssignmentRow[]> {
  return getJson<AccessProfileAssignmentRow[]>(
    `/api/v1/access-profiles/${id}/assignments`,
  );
}

// exactly one of user_id / team_id is set; a team assignment reaches every
// member, which is the point of a profile over a direct grant
export function createAccessProfileAssignment(
  id: string,
  input: { user_id?: string; team_id?: string },
): Promise<AccessProfileAssignmentRow> {
  return sendJson<AccessProfileAssignmentRow>(
    "POST",
    `/api/v1/access-profiles/${id}/assignments`,
    input,
  );
}

export function deleteAccessProfileAssignment(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/access-profile-assignments/${id}`);
}

// an empty allow-list means "no restriction"; deny beats allow. entries are
// exact names or a trailing-* prefix glob
export function setAccessProfilePolicy(
  id: string,
  policy: {
    allowed_models: string[];
    denied_models: string[];
    allowed_routes: string[];
    denied_routes: string[];
  },
): Promise<AccessProfilePolicy> {
  return sendJson<AccessProfilePolicy>(
    "PUT",
    `/api/v1/access-profiles/${id}/policy`,
    policy,
  );
}

export function fetchBusinessUnits(orgId: string): Promise<BusinessUnitRow[]> {
  return getJson<BusinessUnitRow[]>(`/api/v1/orgs/${orgId}/business-units`);
}

export function createBusinessUnit(
  orgId: string,
  input: { name: string; slug?: string },
): Promise<BusinessUnitRow> {
  return sendJson<BusinessUnitRow>(
    "POST",
    `/api/v1/orgs/${orgId}/business-units`,
    input,
  );
}

// a slug change breaks whatever already attributes spend by it, so the server
// demands allow_slug_change before it will move one
export function updateBusinessUnit(
  id: string,
  input: {
    name?: string;
    slug?: string;
    allow_slug_change?: boolean;
    retired?: boolean;
  },
): Promise<BusinessUnitRow> {
  return sendJson<BusinessUnitRow>(
    "PUT",
    `/api/v1/business-units/${id}`,
    input,
  );
}

export function deleteBusinessUnit(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/business-units/${id}`);
}

export function fetchCustomers(orgId: string): Promise<CustomerRow[]> {
  return getJson<CustomerRow[]>(`/api/v1/orgs/${orgId}/customers`);
}

export function createCustomer(
  orgId: string,
  input: { name: string; slug?: string; business_unit_id?: string | null },
): Promise<CustomerRow> {
  return sendJson<CustomerRow>(
    "POST",
    `/api/v1/orgs/${orgId}/customers`,
    input,
  );
}

export function updateCustomer(
  id: string,
  // business_unit_id follows the server's three-state contract: omit to leave
  // unchanged, null to unassign, an id to move it
  input: {
    name?: string;
    slug?: string;
    allow_slug_change?: boolean;
    business_unit_id?: string | null;
    retired?: boolean;
  },
): Promise<CustomerRow> {
  return sendJson<CustomerRow>("PUT", `/api/v1/customers/${id}`, input);
}

export function deleteCustomer(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/customers/${id}`);
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
  /** extra egress proxies the upstream client rotates through; never null */
  egress_proxies: string[];
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
  /**
   * new slug. the server rejects it unless `allow_slug_change` is also true,
   * because `provider-slug/model` addresses depend on the old one
   */
  slug?: string;
  allow_slug_change?: boolean;
  kind?: string;
  api_base?: string;
  api_key?: string;
  api_key_env?: string;
  egress_proxy?: string;
  /** omit to leave unchanged; an empty array clears the list */
  egress_proxies?: string[];
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

/** What a "Test connection" attempt found. */
export interface ProviderTestResult {
  reachable: boolean;
  /** the URL actually probed, so the operator sees what was tried */
  probed_url: string;
  /** absent when the request never completed (DNS, TLS, refused, timeout) */
  status?: number | null;
  latency_ms: number;
  /** how the credential was resolved: "stored", "env", "none", … */
  credential: string;
  models_found?: number | null;
  error?: string | null;
}

/**
 * Probe a stored provider and report whether it answers.
 *
 * Hits the same free model-list endpoint the gateway's health sweep uses, so a
 * green result here means the sweep will agree. Costs nothing to run — it is a
 * catalogue call, not a completion.
 */
export function testProvider(id: string): Promise<ProviderTestResult> {
  return sendJson<ProviderTestResult>("POST", `/api/v1/providers/${id}/test`);
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

export function fetchProviderGroups(
  orgId: string,
): Promise<ProviderGroupRow[]> {
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
  return sendJson<ProviderGroupRow>(
    "PUT",
    `/api/v1/provider-groups/${id}`,
    input,
  );
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
  /**
   * catalog metadata and per-model execution policy, written by
   * `setRouteAdvanced`. an `AdvancedModelConfig` object; every field of it is
   * `#[serde(default)]` on the backend, so a route nobody has edited carries
   * `{}` rather than null
   */
  advanced: Record<string, unknown>;
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
  /// empty means the key may reach every provider on an allowed route
  providers: string[];
  disabled: boolean;
  expires_at?: string | null;
  /// per-key response-cache override; null inherits the route decision
  cache_enabled?: boolean | null;
  /// local account that minted the key from the self-service panel; null for
  /// admin-created and bootstrap-config keys
  created_by: string | null;
  /// business unit this key's spend rolls up to; null leaves the key
  /// attributed to its tenancy chain only
  business_unit_id: string | null;
  /// customer this key's spend rolls up to; null when unattributed
  customer_id: string | null;
  created_at: string;
}

// returned only from createVirtualKey — carries the plaintext secret, shown
// once and never persisted beyond the create mutation's immediate result
export interface CreatedVirtualKey extends VirtualKeyRow {
  key: string;
}

export interface CreateVirtualKeyInput {
  /** required — see MintKeyInput; the same rule applies on the admin path */
  name: string;
  models?: string[];
  /** upstream provider allow-list, by slug; empty permits every provider */
  providers?: string[];
  cache?: boolean | null;
  /** key lifetime in days; omitted means "never expires" */
  expires_in_days?: number;
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

/**
 * Narrow a key to a set of upstream providers, by slug. An empty list restores
 * the permissive default rather than locking the key out of everything.
 */
export function setVirtualKeyProviders(
  id: string,
  providers: string[],
): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>(
    "PUT",
    `/api/v1/virtual-keys/${id}/providers`,
    { providers },
  );
}

/**
 * Point a key's spend at a business unit and/or a customer.
 *
 * Both fields follow the server's three-state contract: omit to leave
 * unchanged, `null` to clear, a uuid to set. The editor always sends both, so
 * clearing one dimension is a thing the operator can actually do — omitting a
 * field they emptied would silently keep the old attribution.
 */
export function setVirtualKeyAttribution(
  id: string,
  attribution: {
    business_unit_id?: string | null;
    customer_id?: string | null;
  },
): Promise<VirtualKeyRow> {
  return sendJson<VirtualKeyRow>(
    "PUT",
    `/api/v1/virtual-keys/${id}/attribution`,
    attribution,
  );
}

export function deleteVirtualKey(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/virtual-keys/${id}`);
}

// --- prompt template repository (crates/rolter-control/src/crud.rs) ---

export interface PromptTemplateVariable {
  name: string;
  required: boolean;
  default?: string;
}

export type PromptDecoratorRole = "system" | "assistant" | "user";
export type PromptDecoratorPosition = "prepend" | "append";

export interface PromptTemplateDecorator {
  role: PromptDecoratorRole;
  position: PromptDecoratorPosition;
  content: string;
}

export interface PromptTemplateRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  description?: string | null;
  published_version?: number | null;
  created_at: string;
}

export interface PromptTemplateVersionRow {
  template_id: string;
  version: number;
  variables: PromptTemplateVariable[];
  decorators: PromptTemplateDecorator[];
  created_at: string;
}

export type PromptTemplateScopeType =
  "org" | "project" | "route" | "virtual_key";

export interface PromptTemplateScopeRow {
  template_id: string;
  version: number;
  scope_type: PromptTemplateScopeType;
  scope_id: string;
  created_at: string;
}

export interface PromptTemplateScopeInput {
  scope_type: PromptTemplateScopeType;
  scope_id: string;
}

export function fetchPromptTemplates(
  orgId: string,
): Promise<PromptTemplateRow[]> {
  return getJson<PromptTemplateRow[]>(`/api/v1/orgs/${orgId}/prompt-templates`);
}

export function createPromptTemplate(
  orgId: string,
  input: { name: string; slug?: string; description?: string },
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>(
    "POST",
    `/api/v1/orgs/${orgId}/prompt-templates`,
    input,
  );
}

export function updatePromptTemplate(
  templateId: string,
  input: { name?: string; description?: string },
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>(
    "PUT",
    `/api/v1/prompt-templates/${templateId}`,
    input,
  );
}

export function deletePromptTemplate(templateId: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/prompt-templates/${templateId}`);
}

export function fetchPromptTemplateVersions(
  templateId: string,
): Promise<PromptTemplateVersionRow[]> {
  return getJson<PromptTemplateVersionRow[]>(
    `/api/v1/prompt-templates/${templateId}/versions`,
  );
}

export function createPromptTemplateVersion(
  templateId: string,
  input: {
    variables: PromptTemplateVariable[];
    decorators: PromptTemplateDecorator[];
  },
): Promise<PromptTemplateVersionRow> {
  return sendJson<PromptTemplateVersionRow>(
    "POST",
    `/api/v1/prompt-templates/${templateId}/versions`,
    input,
  );
}

export function fetchPromptTemplateScopes(
  templateId: string,
  version: number,
): Promise<PromptTemplateScopeRow[]> {
  return getJson<PromptTemplateScopeRow[]>(
    `/api/v1/prompt-templates/${templateId}/versions/${version}/scopes`,
  );
}

export function setPromptTemplateScopes(
  templateId: string,
  version: number,
  scopes: PromptTemplateScopeInput[],
): Promise<PromptTemplateScopeRow[]> {
  return sendJson<PromptTemplateScopeRow[]>(
    "PUT",
    `/api/v1/prompt-templates/${templateId}/versions/${version}/scopes`,
    { scopes },
  );
}

export function publishPromptTemplateVersion(
  templateId: string,
  version: number,
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>(
    "PUT",
    `/api/v1/prompt-templates/${templateId}/publish`,
    { version },
  );
}

export function rollbackPromptTemplateVersion(
  templateId: string,
  version: number,
): Promise<PromptTemplateRow> {
  return sendJson<PromptTemplateRow>(
    "PUT",
    `/api/v1/prompt-templates/${templateId}/rollback`,
    { version },
  );
}

// --- organization skills repository (crates/rolter-control/src/crud.rs) ---

export type SkillMinimumRole = "viewer" | "member" | "admin";

export interface SkillRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  description: string;
  retired_at?: string | null;
  published_version?: number | null;
  allowed_team_ids: string[];
  minimum_role: SkillMinimumRole;
  created_at: string;
}

export interface SkillVersionRow {
  skill_id: string;
  version: number;
  content?: string | null;
  content_ref?: string | null;
  metadata: Record<string, unknown>;
  created_at: string;
}

export interface CreateSkillInput {
  name: string;
  slug?: string;
  description?: string;
  allowed_team_ids?: string[];
  minimum_role?: SkillMinimumRole;
}

export interface UpdateSkillInput {
  name?: string;
  description?: string;
  retired?: boolean;
  allowed_team_ids?: string[];
  minimum_role?: SkillMinimumRole;
}

export function fetchSkills(orgId: string): Promise<SkillRow[]> {
  return getJson<SkillRow[]>(`/api/v1/orgs/${orgId}/skills`);
}

export function createSkill(
  orgId: string,
  input: CreateSkillInput,
): Promise<SkillRow> {
  return sendJson<SkillRow>("POST", `/api/v1/orgs/${orgId}/skills`, input);
}

export function updateSkill(
  id: string,
  input: UpdateSkillInput,
): Promise<SkillRow> {
  return sendJson<SkillRow>("PUT", `/api/v1/skills/${id}`, input);
}

export function deleteSkill(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/skills/${id}`);
}

export function fetchSkillVersions(id: string): Promise<SkillVersionRow[]> {
  return getJson<SkillVersionRow[]>(`/api/v1/skills/${id}/versions`);
}

export function createSkillVersion(
  id: string,
  input:
    | {
        content: string;
        content_ref?: never;
        metadata: Record<string, unknown>;
      }
    | {
        content?: never;
        content_ref: string;
        metadata: Record<string, unknown>;
      },
): Promise<SkillVersionRow> {
  return sendJson<SkillVersionRow>(
    "POST",
    `/api/v1/skills/${id}/versions`,
    input,
  );
}

export function publishSkillVersion(
  id: string,
  version: number,
): Promise<SkillRow> {
  return sendJson<SkillRow>("PUT", `/api/v1/skills/${id}/publish`, { version });
}

export function rollbackSkillVersion(
  id: string,
  version: number,
): Promise<SkillRow> {
  return sendJson<SkillRow>("PUT", `/api/v1/skills/${id}/rollback`, {
    version,
  });
}

// --- budgets, rate limits, model pricing (crates/rolter-control/src/crud.rs) ---

// every scope a budget or rate limit may hang off, in the order of the
// `SCOPE_TYPES` allowlist in `crates/rolter-control/src/crud.rs`. business unit
// and customer are the governance dimensions a key's spend rolls up to (#539);
// leaving them out made those budgets uncreatable from the dashboard
export const SCOPE_TYPES = [
  "org",
  "team",
  "project",
  "virtual_key",
  "business_unit",
  "customer",
] as const;

export type ScopeType = (typeof SCOPE_TYPES)[number];

/**
 * A budget's own answer to traffic against a model with no price row, which
 * accrues zero spend and so can never exhaust the budget. `null` inherits the
 * deployment-wide setting; an override can only tighten it, because the gateway
 * resolves most-restrictive-wins across the scope chain (#996).
 */
export const UNPRICED_POLICIES = ["ignore", "warn", "block"] as const;

export type UnpricedPolicy = (typeof UNPRICED_POLICIES)[number];

export interface BudgetRow {
  id: string;
  scope_type: string;
  scope_id: string;
  /// decimal, returned as text
  limit_usd: string;
  period: string;
  unpriced_policy: UnpricedPolicy | null;
  created_at: string;
}

export interface CreateBudgetInput {
  scope_type: string;
  scope_id: string;
  limit_usd: string;
  period?: string;
  unpriced_policy?: UnpricedPolicy | null;
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

/**
 * The deployment's settlement currency and the codes it can price in.
 *
 * `codes` is the base currency plus every code in the operator's rate table —
 * exactly the set the control plane accepts on a price write. The dashboard
 * used to hardcode seven ISO-4217 codes, which made a configured RUB
 * unselectable and offered a JPY the API would reject (#965).
 */
export interface CurrencySettings {
  base: string;
  codes: string[];
  /** units of `base` per unit of the keyed currency; base is implicitly 1.0 */
  rates: Record<string, number>;
}

export function fetchCurrencySettings(): Promise<CurrencySettings> {
  return getJson<CurrencySettings>("/api/v1/currency");
}

/**
 * `GET /api/v1/version`: the running build and the latest stable release the
 * control plane has heard of (#902). The control plane asks GitHub once at
 * boot and every few hours; the browser never does. `latest`, `release_url`
 * and `checked_at` are null until a check has succeeded, and `enabled` is
 * false when `ROLTER_UPDATE_CHECK=false` opted the deployment out.
 */
export interface VersionStatus {
  current: string;
  latest: string | null;
  release_url: string | null;
  update_available: boolean;
  checked_at: string | null;
  enabled: boolean;
}

export function fetchVersion(): Promise<VersionStatus> {
  return getJson<VersionStatus>("/api/v1/version");
}

/**
 * Whether `code` can be converted into the settlement currency.
 *
 * A price already stored in a code the rate table no longer carries must still
 * display — dropping it would hide real pricing — but it cannot be converted,
 * so spend that includes it is understated. Callers surface that rather than
 * silently treating the number as base currency.
 */
export function isConvertible(
  settings: CurrencySettings | undefined,
  code: string,
): boolean {
  if (!settings) return true;
  const normalized = code.trim().toUpperCase();
  return settings.codes.some((c) => c.toUpperCase() === normalized);
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
  user: {
    id: string;
    email: string;
    is_superadmin: boolean;
    /// set when an admin deactivated the account; a non-null value blocks login
    deactivated_at: string | null;
    created_at: string;
  };
}

// authenticate a local account; returns a session token. rejects (throws) on
// bad credentials or when local accounts aren't configured.
export function login(email: string, password: string): Promise<LoginResponse> {
  return sendJson<LoginResponse>("POST", "/api/v1/auth/login", {
    email,
    password,
  });
}

/** what the login screen may offer; see crates/rolter-control/src/auth_policy.rs */
export interface AuthMethods {
  /** render the email + password form */
  password: boolean;
  /** one entry per enabled identity provider; empty when no IdP is configured */
  sso: { slug: string; name: string; start_url: string }[];
}

// unauthenticated by necessity: read before anyone has a session, so the login
// screen knows whether this deployment uses passwords, sso, or both
export function getAuthMethods(): Promise<AuthMethods> {
  return getJson<AuthMethods>("/api/v1/auth/methods");
}

export function logout(): Promise<void> {
  return sendJson<void>("POST", "/api/v1/auth/logout");
}

/**
 * A membership as `/auth/me` serialises it (rolter-store `Membership`).
 *
 * Same row as [`MembershipRow`] plus `source`, which the admin CRUD screens
 * have no use for: `manual` (invitation, seed, admin api) or `sso` (an IdP
 * group mapping).
 */
export interface MeMembership extends MembershipRow {
  source: string;
}

/** `{user, memberships}` — crates/rolter-control/src/auth.rs `MeResponse` */
export interface MeResponse {
  user: UserRow;
  memberships: MeMembership[];
}

/**
 * Who the stored session token belongs to.
 *
 * The shell calls this on boot: a 200 says the token is still live and carries
 * the account's superadmin flag and memberships straight from the server, a
 * 401 says it is dead and the session is cleared (#1196). Not mounted at all
 * on an open-mode control plane with no store, which answers 404 — a
 * deployment shape, not a rejected session.
 */
export function fetchMe(): Promise<MeResponse> {
  return getJson<MeResponse>("/api/v1/auth/me");
}

// --- invitations (crates/rolter-control/src/invitations.rs, #712) ---

export interface Invitation {
  id: string;
  org_id: string;
  email: string;
  role: Role;
  team_id: string | null;
  project_id: string | null;
  invited_by: string | null;
  expires_at: string;
  accepted_at: string | null;
  revoked_at: string | null;
  created_at: string;
}

export interface CreatedInvitation {
  invitation: Invitation;
  /** shown once; the server keeps only its digest */
  token: string;
  accept_url: string;
}

export function listInvitations(orgId: string): Promise<Invitation[]> {
  return getJson<Invitation[]>(`/api/v1/orgs/${orgId}/invitations`);
}

export function createInvitation(
  orgId: string,
  body: {
    email: string;
    role: Role;
    scope_type?: "org" | "team" | "project";
    scope_id?: string;
  },
): Promise<CreatedInvitation> {
  return sendJson<CreatedInvitation>(
    "POST",
    `/api/v1/orgs/${orgId}/invitations`,
    body,
  );
}

export function revokeInvitation(id: string): Promise<Invitation> {
  return sendJson<Invitation>("DELETE", `/api/v1/invitations/${id}`);
}

export interface InvitationPreview {
  org_name: string;
  email: string;
  role: Role;
  expires_at: string;
}

// unauthenticated: the invitee has no account yet, the token is the credential
export function previewInvitation(token: string): Promise<InvitationPreview> {
  return getJson<InvitationPreview>(
    `/api/v1/invitations/accept/${encodeURIComponent(token)}`,
  );
}

export function acceptInvitation(
  token: string,
  password: string,
): Promise<LoginResponse> {
  return sendJson<LoginResponse>(
    "POST",
    `/api/v1/invitations/accept/${encodeURIComponent(token)}/accept`,
    { password },
  );
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

// --- the rbac capability matrix (crates/rolter-control/src/rbac_matrix.rs) ---
//
// `CAPABILITIES` is the one table that backs the guard *and* this endpoint, so
// what the dashboard renders is by construction what the control plane
// enforces. The screen used to draw a hardcoded 3x7 matrix that had drifted
// away from the real 50-odd resources (#1178).

export type RbacAction = "read" | "create" | "update" | "delete";

/**
 * What one action on one resource takes.
 *
 * An action the resource does not have at all is *absent* from `actions`
 * rather than present with a null authority — the backend `filter_map`s it
 * away — so a UI must render a missing entry as "not applicable" and never as
 * "denied".
 */
export interface RbacActionView {
  action: RbacAction;
  /** minimum scoped role; null when the action is superadmin- or auth-only */
  minimum_role: Role | null;
  superadmin_only: boolean;
  /** no membership required: any authenticated caller may perform it */
  authenticated_only: boolean;
}

export interface RbacResourceView {
  resource: string;
  /** `deployment` | `org` | `team` | `project` — where the resource lives */
  scope: string;
  actions: RbacActionView[];
}

export interface RbacRoleView {
  role: Role;
  /** total order over roles: viewer 0 < member 1 < admin 2 */
  rank: number;
}

/** one `resource:action` pair an org-defined role grants explicitly */
export interface RbacCustomGrant {
  resource: string;
  action: string;
}

export interface RbacCustomRoleView {
  id: string;
  slug: string;
  name: string;
  description: string | null;
  /** the built-in role this custom role is at least equivalent to */
  base_role: Role;
  base_rank: number;
  /** the pairs it grants on top of `base_role` */
  grants: RbacCustomGrant[];
  /** pairs this build's capability table does not define; they grant nothing */
  unknown_grants: RbacCustomGrant[];
}

export interface RbacMatrix {
  roles: RbacRoleView[];
  resources: RbacResourceView[];
  /** org-defined roles; empty when no org was asked for, or none are defined */
  custom_roles: RbacCustomRoleView[];
}

// any authenticated principal may read it: it describes the rules, not anyone's
// access. `orgId` adds that org's custom roles and needs a membership there
export function fetchRbacMatrix(orgId?: string): Promise<RbacMatrix> {
  const query = orgId ? `?org_id=${encodeURIComponent(orgId)}` : "";
  return getJson<RbacMatrix>(`/api/v1/rbac/matrix${query}`);
}

/** A custom role the caller holds at the requested scope. */
export interface RbacHeldRole {
  profile_id: string;
  role_id: string;
  role_slug: string;
  base_role: Role;
}

/** The model/route visibility a caller's access profiles impose. */
export interface RbacModelPolicy {
  allowed_models?: string[];
  denied_models?: string[];
  allowed_routes?: string[];
  denied_routes?: string[];
}

/**
 * What *this* caller may do at one org/team/project chain.
 *
 * The counterpart to [`RbacMatrix`], which describes the rules for everyone:
 * this is the caller's own answer, evaluated server-side from their
 * memberships and access profiles. The dashboard disables controls with it
 * (#1183); enforcement stays on the control plane, so the answer is advisory
 * here and authoritative only there.
 */
export interface RbacEffective {
  /** the admin token, a superadmin user, or any caller in open mode */
  superadmin: boolean;
  /** the resolved role at the chain, null when no membership reaches it */
  role: Role | null;
  /** the `resource:action` pairs the caller may perform, default-deny */
  allowed: string[];
  /** the custom roles behind any pair `role` alone does not explain */
  custom_roles: RbacHeldRole[];
  /** absent when no access profile restricts anything, which means "all" */
  model_policy: RbacModelPolicy | null;
}

/** The org/team/project chain a capability question is asked at. */
export interface RbacScope {
  orgId?: string;
  teamId?: string;
  projectId?: string;
}

// every part of the chain is optional: the deployment-scoped resources need no
// chain at all, and a caller who is not in an org yet still gets an answer —
// an empty one, which is exactly what `authorize` would decide
export function fetchEffective(scope: RbacScope = {}): Promise<RbacEffective> {
  const params = new URLSearchParams();
  if (scope.orgId) params.set("org_id", scope.orgId);
  if (scope.teamId) params.set("team_id", scope.teamId);
  if (scope.projectId) params.set("project_id", scope.projectId);
  const query = params.size ? `?${params}` : "";
  return getJson<RbacEffective>(`/api/v1/rbac/effective${query}`);
}

// --- scim provisioning tokens (crates/rolter-control/src/scim.rs, #540) ---
//
// the dashboard only manages the tokens an IdP authenticates with; the SCIM
// resource endpoints (/scim/v2/Users) are driven by the IdP itself and are
// deliberately not called from here. every one of these requires Admin on the
// org (rolter_auth::Role::Admin).

// an issued provisioning token as the server stores it. `token_hash` is never
// serialised, so there is no field carrying the secret — a listed token can be
// identified and revoked, never read back.
export interface ScimTokenRow {
  id: string;
  org_id: string;
  name: string;
  created_by: string | null;
  created_at: string;
  /** last time an IdP presented this token; null until it is first used */
  last_used_at: string | null;
  /** set once revoked; a revoked token never authenticates again */
  revoked_at: string | null;
}

// the create response: the stored row plus the one and only sighting of the
// plaintext bearer value
export interface CreatedScimToken extends ScimTokenRow {
  /** the bearer value the IdP is configured with; not stored, not recoverable */
  secret: string;
}

export function fetchScimTokens(orgId: string): Promise<ScimTokenRow[]> {
  return getJson<ScimTokenRow[]>(`/api/v1/orgs/${orgId}/scim-tokens`);
}

export function createScimToken(
  orgId: string,
  input: { name: string },
): Promise<CreatedScimToken> {
  return sendJson<CreatedScimToken>(
    "POST",
    `/api/v1/orgs/${orgId}/scim-tokens`,
    input,
  );
}

// revocation returns the updated row rather than 204 — the screen uses it to
// show the revoked timestamp without a refetch race
export function revokeScimToken(id: string): Promise<ScimTokenRow> {
  return sendJson<ScimTokenRow>("DELETE", `/api/v1/scim-tokens/${id}`);
}

// --- scim group mappings (crates/rolter-control/src/scim_groups.rs, #540) ---
//
// the operator-facing half of SCIM Groups: the IdP pushes /scim/v2/Groups, and
// these mappings decide what a pushed group is worth. deliberately the same
// model as the SSO one above, down to the vocabulary — one thing to learn, one
// place a privilege bug can hide.

/** an IdP group name granting a role at a scope inside the org that owns it */
export interface ScimGroupMappingRow {
  id: string;
  org_id: string;
  group_name: string;
  /** the most specific non-null scope wins; both null grants at the org */
  team_id: string | null;
  project_id: string | null;
  role: string;
  created_at: string;
}

export interface CreateScimGroupMappingInput {
  group_name: string;
  /** `admin` | `member` | `viewer`; `parse_role` refuses anything else */
  role: string;
  /** omit both to grant at the org; the scope must live in the same org */
  team_id?: string;
  project_id?: string;
}

export function fetchScimGroupMappings(
  orgId: string,
): Promise<ScimGroupMappingRow[]> {
  return getJson<ScimGroupMappingRow[]>(
    `/api/v1/orgs/${orgId}/scim-group-mappings`,
  );
}

// creating one reconciles the group's members straight away rather than at the
// next sync, so the list the screen refetches is already the new truth
export function createScimGroupMapping(
  orgId: string,
  input: CreateScimGroupMappingInput,
): Promise<ScimGroupMappingRow> {
  return sendJson<ScimGroupMappingRow>(
    "POST",
    `/api/v1/orgs/${orgId}/scim-group-mappings`,
    input,
  );
}

export function deleteScimGroupMapping(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/scim-group-mappings/${id}`);
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
  /** required: the plaintext is shown once, so an unnamed key is
   *  unattributable forever after (#945). the server rejects a blank one */
  name: string;
  models?: string[];
  providers?: string[];
  cache?: boolean | null;
  /** key lifetime in days. omitted means "never expires" — a choice the
   *  operator makes deliberately, never the result of leaving a field alone */
  expires_in_days?: number;
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
  return sendJson<SecuritySettingsDto>(
    "PUT",
    "/api/v1/security-settings",
    input,
  );
}

// ---------------------------------------------------------------------------
// compatibility policy: values applied when translating between the OpenAI and
// Anthropic wire formats

export interface CompatibilityPolicyDto {
  anthropic_version: string;
  default_max_tokens: number;
  updated_at: string;
  // fields that only take effect after a gateway restart; empty today, but the
  // server owns the list so the screen can warn without knowing which they are
  restart_required: string[];
}

export interface UpdateCompatibilityPolicyInput {
  anthropic_version: string;
  default_max_tokens: number;
}

export function fetchCompatibilityPolicy(): Promise<CompatibilityPolicyDto> {
  return getJson<CompatibilityPolicyDto>("/api/v1/compatibility-policy");
}

export function updateCompatibilityPolicy(
  input: UpdateCompatibilityPolicyInput,
): Promise<CompatibilityPolicyDto> {
  return sendJson<CompatibilityPolicyDto>(
    "PUT",
    "/api/v1/compatibility-policy",
    input,
  );
}

// ---------------------------------------------------------------------------
// runtime policy: retry, timeout and admission-queue controls

export const BACKPRESSURE_POLICIES = ["drop", "block", "error"] as const;

export type BackpressurePolicy = (typeof BACKPRESSURE_POLICIES)[number];

export interface RuntimePolicyDto {
  retry_max_retries: number;
  retry_base_ms: number;
  retry_max_ms: number;
  timeout_connect_s: number;
  timeout_request_s: number;
  queue_enabled: boolean;
  queue_capacity: number;
  queue_workers: number;
  queue_backpressure: BackpressurePolicy;
  queue_block_ms: number;
  updated_at: string;
}

export type UpdateRuntimePolicyInput = Omit<RuntimePolicyDto, "updated_at">;

export function fetchRuntimePolicy(): Promise<RuntimePolicyDto> {
  return getJson<RuntimePolicyDto>("/api/v1/runtime-policy");
}

export function updateRuntimePolicy(
  input: UpdateRuntimePolicyInput,
): Promise<RuntimePolicyDto> {
  return sendJson<RuntimePolicyDto>("PUT", "/api/v1/runtime-policy", input);
}

// ---------------------------------------------------------------------------
// client settings: advertised base URL and upstream header handling (#564)

export interface ClientSettingsDto {
  public_base_url: string | null;
  forwarded_headers: string[];
  injected_headers: Record<string, string>;
  request_id_header: string;
  updated_at: string;
  /// header names propagated whether or not they are listed
  always_propagated: string[];
  /// header names that can never be forwarded or injected
  reserved: string[];
}

export type UpdateClientSettingsInput = Pick<
  ClientSettingsDto,
  | "public_base_url"
  | "forwarded_headers"
  | "injected_headers"
  | "request_id_header"
>;

export function fetchClientSettings(): Promise<ClientSettingsDto> {
  return getJson<ClientSettingsDto>("/api/v1/client-settings");
}

export function updateClientSettings(
  input: UpdateClientSettingsInput,
): Promise<ClientSettingsDto> {
  return sendJson<ClientSettingsDto>("PUT", "/api/v1/client-settings", input);
}

// ---------------------------------------------------------------------------
// model defaults: inference parameters filled in when a client omits them (#564)

export interface ModelDefaultsDto {
  enabled: boolean;
  default_model: string | null;
  default_temperature: number | null;
  default_top_p: number | null;
  default_max_tokens: number | null;
  updated_at: string;
}

export type UpdateModelDefaultsInput = Omit<ModelDefaultsDto, "updated_at">;

export function fetchModelDefaults(): Promise<ModelDefaultsDto> {
  return getJson<ModelDefaultsDto>("/api/v1/model-defaults");
}

export function updateModelDefaults(
  input: UpdateModelDefaultsInput,
): Promise<ModelDefaultsDto> {
  return sendJson<ModelDefaultsDto>("PUT", "/api/v1/model-defaults", input);
}

// ---------------------------------------------------------------------------
// logging settings: request-log sampling, payload capture and retention

export interface LoggingSettingsDto {
  sample_rate: number;
  payload_capture_enabled: boolean;
  payload_capture_max_bytes: number;
  payload_capture_redact_fields: string[];
  payload_capture_models: string[];
  payload_capture_virtual_key_ids: string[];
  retention_days: number;
  payload_retention_hours: number;
  updated_at: string;
}

export type UpdateLoggingSettingsInput = Omit<LoggingSettingsDto, "updated_at">;

export function fetchLoggingSettings(): Promise<LoggingSettingsDto> {
  return getJson<LoggingSettingsDto>("/api/v1/logging-settings");
}

export function updateLoggingSettings(
  input: UpdateLoggingSettingsInput,
): Promise<LoggingSettingsDto> {
  return sendJson<LoggingSettingsDto>("PUT", "/api/v1/logging-settings", input);
}

// ---------------------------------------------------------------------------
// feature flags: global switches for hot-reloadable gateway subsystems

// a flag whose subsystem this deployment cannot run. the screen renders these
// as unavailable rather than as a switch that silently does nothing (#535)
export interface UnavailableFlagDto {
  flag: FeatureFlagKey;
  reason: string;
}

export const FEATURE_FLAG_KEYS = [
  "response_cache",
  "cache_aware_routing",
  "circuit_breaker",
  "active_health_checks",
  "complexity_routing",
  "guardrails",
] as const;

export type FeatureFlagKey = (typeof FEATURE_FLAG_KEYS)[number];

export type FeatureFlagValues = Record<FeatureFlagKey, boolean>;

export type FeatureFlagsDto = FeatureFlagValues & {
  updated_at: string;
  unavailable: UnavailableFlagDto[];
};

export function fetchFeatureFlags(): Promise<FeatureFlagsDto> {
  return getJson<FeatureFlagsDto>("/api/v1/feature-flags");
}

export function updateFeatureFlags(
  input: FeatureFlagValues,
): Promise<FeatureFlagsDto> {
  return sendJson<FeatureFlagsDto>("PUT", "/api/v1/feature-flags", input);
}

// ---------------------------------------------------------------------------
// cluster inventory: nodes register themselves by identifying on their snapshot
// poll, so this is a read + drain/forget surface, never an enrolment one

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

export function fetchClusterNodes(): Promise<ClusterNodeRow[]> {
  return getJson<ClusterNodeRow[]>("/api/v1/cluster/nodes");
}

export function setClusterNodeDrain(
  id: string,
  draining: boolean,
): Promise<ClusterNodeRow> {
  return sendJson<ClusterNodeRow>(
    "PUT",
    `/api/v1/cluster/nodes/${encodeURIComponent(id)}/drain`,
    { draining },
  );
}

export function forgetClusterNode(id: string): Promise<void> {
  return sendJson<void>(
    "DELETE",
    `/api/v1/cluster/nodes/${encodeURIComponent(id)}`,
  );
}

// ---------------------------------------------------------------------------
// adaptive routing policy: the global kill switch and blend weights every route
// on the `adaptive` strategy is governed by

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
  return sendJson<AdaptiveRoutingPolicyDto>(
    "PUT",
    "/api/v1/adaptive-routing-policy",
    input,
  );
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
  return getJson<AdaptiveRoutingTelemetryDto>(
    "/api/v1/adaptive-routing-telemetry",
  );
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
  return sendJson<AlertChannelRow>(
    "PUT",
    `/api/v1/alert-channels/${id}`,
    input,
  );
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

// the probe's own verdict. `health_error` is `skip_serializing_if = "none"` on
// the control plane, so it is absent on a delivered probe and carries the
// sink's refusal — the reason delivery failed — on a rejected one
export interface ConnectorTestResult {
  delivered: boolean;
  health_status: string;
  health_checked_at: string;
  health_error?: string | null;
}

export function testConnector(id: string): Promise<ConnectorTestResult> {
  return sendJson<ConnectorTestResult>(
    "POST",
    `/api/v1/connectors/${id}/test`,
  );
}

/**
 * The OpenTelemetry Collector configuration rendered from the enabled
 * connectors (`crates/rolter-control/src/collector_config.rs`).
 *
 * A YAML document, not JSON: the collector's `confmap` HTTP provider reads it
 * verbatim from `--config=http://...`. ADR-0026 put the per-destination
 * fan-out in the collector rather than in the data plane, so this document is
 * the delivery path — the connectors are only the rows it is rendered from.
 * Superadmin-only, like the rest of the connector surface.
 */
export function fetchCollectorConfig(): Promise<string> {
  return getText("/api/v1/connectors/collector-config");
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
  // clickhouse averages an empty window to null (and quantile to nan); the
  // control plane forwards the JSON as-is
  avg_latency_ms: number | null;
  p95_latency_ms: number | null;
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
  // by-id: a 404 here is "no such event", not "analytics is unavailable"
  return getAnalytics<McpLogDetail>(
    `/api/v1/mcp/logs/${encodeURIComponent(eventId)}`,
    { notFoundIsUnavailable: false },
  );
}

// ---------------------------------------------------------------------------
// mcp servers, oauth consent grants and token sessions
// (crates/rolter-control/src/mcp_oauth.rs, #561)
//
// three properties of that module are load-bearing for the screens below:
// no token material ever crosses this boundary (a session carries metadata and
// `has_refresh_token`, never a token); a grant is the unit of consent, so
// revoking one revokes its sessions in the same transaction; and a listing is
// owner-scoped — an org admin sees the whole org, everyone else sees only the
// rows they own

export interface McpServerRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  url: string;
  transport: string;
  description: string;
  enabled: boolean;
  tools: string[];
  source: "custom" | "library";
  required_scopes: string[];
  created_at: string;
  /// authorization endpoint a user's browser is sent to for consent (#707)
  authorize_url: string | null;
  /// token endpoint the code, refresh and exchange grants are posted to
  token_url: string | null;
  /// the OAuth client rolter presents. the matching secret is sealed and
  /// deliberately never crosses this boundary
  client_id: string | null;
  /// scopes requested when a consent flow does not name its own
  default_scopes: string[];
  /// whether a sealed client secret is stored, so the UI can show the client is
  /// confidential without the control plane handing the secret out
  has_client_secret: boolean;
}

export interface McpServerInput {
  name: string;
  slug: string;
  url: string;
  transport: string;
  description: string;
  enabled: boolean;
  tools: string[];
  source: "custom" | "library";
  required_scopes: string[];
}

export interface McpLibraryItem {
  slug: string;
  name: string;
  description: string;
  url: string;
  transport: string;
  tools: string[];
  required_scopes: string[];
  installed: boolean;
}

export interface McpToolRef {
  server_id: string;
  tool: string;
}

export interface McpToolGroupRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  description: string;
  enabled: boolean;
  tools: McpToolRef[];
  created_at: string;
  updated_at: string;
}

export interface McpGatewaySettingsRow {
  org_id: string;
  default_transport: string;
  connect_timeout_ms: number;
  request_timeout_ms: number;
  max_retries: number;
  default_failure_mode: "fail_open" | "fail_closed";
  allow_unlisted_tools: boolean;
  updated_at: string;
}

export interface McpOAuthGrantRow {
  id: string;
  server_id: string;
  user_id: string;
  scopes: string[];
  granted_at: string;
  revoked_at: string | null;
  revoked_by: string | null;
  /** Resolved server-side so clients never infer state from a nullable timestamp. */
  active: boolean;
}

/** Session metadata for a grant without exposing sealed token material. */
export interface McpOAuthSessionRow {
  id: string;
  grant_id: string;
  scopes: string[];
  expires_at: string;
  refresh_expires_at: string | null;
  revoked_at: string | null;
  created_at: string;
  last_used_at: string | null;
  /** Whether a stored refresh token makes this session renewable. */
  has_refresh_token: boolean;
}

export function fetchMcpServers(orgId: string): Promise<McpServerRow[]> {
  return getJson<McpServerRow[]>(`/api/v1/orgs/${orgId}/mcp-servers`);
}

export function createMcpServer(
  orgId: string,
  input: McpServerInput,
): Promise<McpServerRow> {
  return sendJson<McpServerRow>(
    "POST",
    `/api/v1/orgs/${orgId}/mcp-servers`,
    input,
  );
}

export function updateMcpServer(
  id: string,
  input: Omit<McpServerInput, "slug" | "source">,
): Promise<McpServerRow> {
  return sendJson<McpServerRow>("PATCH", `/api/v1/mcp-servers/${id}`, input);
}

export function deleteMcpServer(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/mcp-servers/${id}`);
}

export function fetchMcpLibrary(orgId: string): Promise<McpLibraryItem[]> {
  return getJson<McpLibraryItem[]>(`/api/v1/orgs/${orgId}/mcp/library`);
}

export function fetchMcpToolGroups(orgId: string): Promise<McpToolGroupRow[]> {
  return getJson<McpToolGroupRow[]>(`/api/v1/orgs/${orgId}/mcp/tool-groups`);
}

export function createMcpToolGroup(
  orgId: string,
  input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at">,
): Promise<McpToolGroupRow> {
  return sendJson<McpToolGroupRow>(
    "POST",
    `/api/v1/orgs/${orgId}/mcp/tool-groups`,
    input,
  );
}

export function updateMcpToolGroup(
  id: string,
  input: Omit<
    McpToolGroupRow,
    "id" | "org_id" | "slug" | "created_at" | "updated_at"
  >,
): Promise<McpToolGroupRow> {
  return sendJson<McpToolGroupRow>(
    "PUT",
    `/api/v1/mcp/tool-groups/${id}`,
    input,
  );
}

export function deleteMcpToolGroup(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/mcp/tool-groups/${id}`);
}

export function fetchMcpSettings(
  orgId: string,
): Promise<McpGatewaySettingsRow> {
  return getJson<McpGatewaySettingsRow>(`/api/v1/orgs/${orgId}/mcp/settings`);
}

export function updateMcpSettings(
  orgId: string,
  input: Omit<McpGatewaySettingsRow, "org_id" | "updated_at">,
): Promise<McpGatewaySettingsRow> {
  return sendJson<McpGatewaySettingsRow>(
    "PUT",
    `/api/v1/orgs/${orgId}/mcp/settings`,
    input,
  );
}

export function fetchMcpGrants(orgId: string): Promise<McpOAuthGrantRow[]> {
  return getJson<McpOAuthGrantRow[]>(`/api/v1/orgs/${orgId}/mcp/grants`);
}

// revoking consent cascades to every session under the grant
export function revokeMcpGrant(id: string): Promise<McpOAuthGrantRow> {
  return sendJson<McpOAuthGrantRow>("DELETE", `/api/v1/mcp/grants/${id}`);
}

export function fetchMcpSessions(orgId: string): Promise<McpOAuthSessionRow[]> {
  return getJson<McpOAuthSessionRow[]>(`/api/v1/orgs/${orgId}/mcp/sessions`);
}

// revoking one session leaves the consent standing; a new session can be
// minted against the same grant without asking the user again
export function revokeMcpSession(id: string): Promise<McpOAuthSessionRow> {
  return sendJson<McpOAuthSessionRow>("DELETE", `/api/v1/mcp/sessions/${id}`);
}

// ---------------------------------------------------------------------------
// mcp oauth client registration and the consent flow it opens
// (crates/rolter-control/src/mcp_oauth_flow.rs, #707)
//
// the block above is the shelf — grants and sessions that already exist. this
// is the lifecycle that fills it, and three of its rules are visible from
// here: the client secret is write-only (a read answers with
// `has_client_secret` and nothing else), the redirect uri is deployment-derived
// rather than sent by the caller, and nothing minted under a grant may ever
// carry scopes that grant does not

/** A server's registered OAuth client as it reads back — never the secret. */
export interface McpOAuthClientRow {
  server_id: string;
  authorize_url: string | null;
  token_url: string | null;
  client_id: string | null;
  default_scopes: string[];
  has_client_secret: boolean;
  /** the callback to register upstream; derived from the deployment, so it is
   * returned rather than guessed from the browser's own origin */
  redirect_uri: string;
}

export interface McpOAuthClientInput {
  authorize_url: string;
  token_url: string;
  client_id: string;
  /** omitted leaves the sealed secret alone; `""` clears it, which is how a
   * confidential client is downgraded to a public one */
  client_secret?: string;
  default_scopes?: string[];
}

/** Where to send the browser for consent, and how long that url stays good. */
export interface McpAuthorizeStarted {
  authorization_url: string;
  state: string;
  expires_in: number;
}

export function fetchMcpOAuthClient(serverId: string): Promise<McpOAuthClientRow> {
  return getJson<McpOAuthClientRow>(`/api/v1/mcp-servers/${serverId}/oauth-client`);
}

export function setMcpOAuthClient(
  serverId: string,
  input: McpOAuthClientInput,
): Promise<McpOAuthClientRow> {
  return sendJson<McpOAuthClientRow>(
    "PUT",
    `/api/v1/mcp-servers/${serverId}/oauth-client`,
    input,
  );
}

// the control plane does not redirect here: the caller is a `fetch` from the
// dashboard, which cannot usefully follow a cross-origin 302, so the url comes
// back for the browser to open itself
export function startMcpOAuth(
  serverId: string,
  scopes?: string[],
): Promise<McpAuthorizeStarted> {
  return sendJson<McpAuthorizeStarted>(
    "POST",
    `/api/v1/mcp-servers/${serverId}/oauth/authorize`,
    scopes ? { scopes } : {},
  );
}

// renews one session from its stored refresh token. a refusal from the
// authorization server revokes the session rather than being retried, so a
// failure here can legitimately mean "consent is gone, ask for it again"
export function refreshMcpSession(id: string): Promise<McpOAuthSessionRow> {
  return sendJson<McpOAuthSessionRow>("POST", `/api/v1/mcp/sessions/${id}/refresh`);
}

export interface McpSessionExchangeInput {
  scopes?: string[];
  /** RFC 8693 `audience` — the downstream service the token is for */
  audience?: string;
}

// on-behalf-of exchange (RFC 8693): a narrower session descending from the
// same grant, revocable on its own and never broader than the consent
export function exchangeMcpSession(
  id: string,
  input: McpSessionExchangeInput = {},
): Promise<McpOAuthSessionRow> {
  return sendJson<McpOAuthSessionRow>(
    "POST",
    `/api/v1/mcp/sessions/${id}/exchange`,
    input,
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

// ---------------------------------------------------------------------------
// plugin configuration registry (#567; gateway dispatch is tracked by #509)

export interface PluginInstanceRow {
  id: string;
  org_id: string;
  project_id: string | null;
  name: string;
  slug: string;
  description: string;
  kind: "webhook";
  stage: "pre_route" | "pre_upstream" | "post_response";
  enabled: boolean;
  position: number;
  failure_mode: "fail_open" | "fail_closed";
  endpoint: string;
  secret_env: string | null;
  config: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export type PluginInstanceInput = Omit<
  PluginInstanceRow,
  "id" | "org_id" | "slug" | "created_at" | "updated_at"
> & { slug?: string };

export const fetchPlugins = (orgId: string) =>
  getJson<PluginInstanceRow[]>(`/api/v1/orgs/${orgId}/plugins`);
export const createPlugin = (orgId: string, body: PluginInstanceInput) =>
  sendJson<PluginInstanceRow>("POST", `/api/v1/orgs/${orgId}/plugins`, body);
export const updatePlugin = (id: string, body: PluginInstanceInput) =>
  sendJson<PluginInstanceRow>("PUT", `/api/v1/plugins/${id}`, body);
export const deletePlugin = (id: string) =>
  sendJson<void>("DELETE", `/api/v1/plugins/${id}`);

// ---------------------------------------------------------------------------
// deployment-wide guardrail registry

export interface GuardrailRuleRow {
  id: string;
  name: string;
  enabled: boolean;
  source_type: "builtin" | "pattern";
  builtin: "email" | "phone" | "api_token" | "payment_card" | null;
  pattern: string | null;
  stage: "pre_call" | "post_call";
  action: "annotate" | "block" | "redact";
  replacement: string | null;
  include_system: boolean;
  position: number;
  created_at: string;
  updated_at: string;
}

export type GuardrailRuleInput = Omit<
  GuardrailRuleRow,
  "id" | "created_at" | "updated_at"
>;

export interface GuardrailProviderRow {
  id: string;
  name: string;
  enabled: boolean;
  url: string;
  stage: "pre_call" | "post_call";
  timeout_ms: number;
  max_retries: number;
  failure_mode: "fail_open" | "fail_closed";
  max_body_bytes: number;
  auth_kind: "none" | "bearer" | "shared_secret";
  auth_env: string | null;
  created_at: string;
  updated_at: string;
}

export type GuardrailProviderInput = Omit<
  GuardrailProviderRow,
  "id" | "created_at" | "updated_at"
>;

export const fetchGuardrailRules = () =>
  getJson<GuardrailRuleRow[]>("/api/v1/guardrails/rules");
export const createGuardrailRule = (body: GuardrailRuleInput) =>
  sendJson<GuardrailRuleRow>("POST", "/api/v1/guardrails/rules", body);
export const updateGuardrailRule = (id: string, body: GuardrailRuleInput) =>
  sendJson<GuardrailRuleRow>("PUT", `/api/v1/guardrails/rules/${id}`, body);
export const deleteGuardrailRule = (id: string) =>
  sendJson<void>("DELETE", `/api/v1/guardrails/rules/${id}`);

export const fetchGuardrailProviders = () =>
  getJson<GuardrailProviderRow[]>("/api/v1/guardrails/providers");
export const createGuardrailProvider = (body: GuardrailProviderInput) =>
  sendJson<GuardrailProviderRow>("POST", "/api/v1/guardrails/providers", body);
export const updateGuardrailProvider = (
  id: string,
  body: GuardrailProviderInput,
) =>
  sendJson<GuardrailProviderRow>(
    "PUT",
    `/api/v1/guardrails/providers/${id}`,
    body,
  );
export const deleteGuardrailProvider = (id: string) =>
  sendJson<void>("DELETE", `/api/v1/guardrails/providers/${id}`);

/**
 * A dashboard UX event (#805).
 *
 * Structural only: every field is a key, an enum, a duration or an id. There is
 * no field a form value, prompt or free-text body may travel in — `target`
 * carries the *name* of a form, control or validation rule, never what was
 * typed into it. Keep it that way; the `ui_events` table has nowhere to put a
 * value, and the schema is the privacy guarantee rather than a policy applied
 * on write.
 *
 * `user_id` is deliberately absent: the control plane fills it from the
 * authenticated session, so it cannot be forged by a caller.
 */
export interface UiEvent {
  event_id: string;
  /** stable screen key (a route id), never a URL — URLs carry ids and filters */
  screen: string;
  action:
    | "screen_view"
    | "time_to_interactive"
    | "navigate"
    | "back_out"
    | "form_submit"
    | "form_abandon"
    | "validation_error"
    | "empty_state"
    | "error_state"
    | "save_confirmed";
  outcome?: "ok" | "error" | "cancelled";
  /** the name of the form, control or validation rule — never its value */
  target?: string;
  from_screen?: string;
  duration_ms?: number;
  /** joins the event to the gateway request it caused; empty when there was none */
  trace_id?: string;
  session_id?: string;
  org_id?: string;
  team_id?: string;
  project_id?: string;
  app_version?: string;
}

/** The server rejects a batch larger than this. */
export const UI_EVENTS_MAX_BATCH = 100;

/**
 * File a batch of UX events. Browsers batch these — a `visibilitychange`
 * beacon flushes whatever queued since the last one — so a batch is the normal
 * shape rather than an optimization.
 *
 * Returns 202 whether or not the deployment stores them: `logging.ui_events`
 * is a server-side opt-out, and the endpoint is inert without `clickhouse_url`.
 */
export const sendUiEvents = (events: UiEvent[]) =>
  sendJson<void>("POST", "/api/v1/ui-events", { events });

// --- single sign-on (crates/rolter-control/src/sso.rs, #240) ---
//
// an org registers an OIDC provider, and a user hits `/auth/sso/{slug}/start`
// to be bounced through the authorization-code flow. everything here is
// org-scoped and admin-gated (`sso_provider`, `sso_group_mapping` in
// rbac_matrix.rs), so a lesser principal is refused with 403 rather than
// handed an empty list.

/**
 * A registered identity provider, as `SsoProvider` serialises it.
 *
 * The sealed client secret is absent from the payload — it is never redacted,
 * it simply never leaves the control plane. What comes in its place is the
 * derived `has_client_secret` boolean, so the screen can tell a provider that
 * is ready to exchange tokens from one whose first symptom would otherwise be
 * a failed login (#1231).
 */
export interface SsoProviderRow {
  id: string;
  org_id: string;
  name: string;
  slug: string;
  issuer: string;
  client_id: string;
  /**
   * whether a client secret is sealed in the store. derived on the server from
   * `secret_ciphertext.is_some()`; the bytes themselves are never serialised.
   */
  has_client_secret: boolean;
  scopes: string[];
  /** claim in the id token carrying the user's groups; `groups` by default */
  group_claim: string;
  /** role granted when no mapping matches; null refuses that user entirely */
  default_role: string | null;
  enabled: boolean;
  created_at: string;
}

export interface CreateSsoProviderInput {
  name: string;
  slug: string;
  issuer: string;
  client_id: string;
  /** write-only: sealed with the KEK before storage and never returned */
  client_secret?: string;
  /** defaults to openid/email/profile when omitted */
  scopes?: string[];
  group_claim?: string;
  default_role?: string;
}

export function fetchSsoProviders(orgId: string): Promise<SsoProviderRow[]> {
  return getJson<SsoProviderRow[]>(`/api/v1/orgs/${orgId}/sso-providers`);
}

export function createSsoProvider(
  orgId: string,
  input: CreateSsoProviderInput,
): Promise<SsoProviderRow> {
  return sendJson<SsoProviderRow>(
    "POST",
    `/api/v1/orgs/${orgId}/sso-providers`,
    input,
  );
}

/**
 * The editable half of a provider. `slug` is absent on purpose: it is in the
 * login URL, so the server refuses to change it (#1233).
 *
 * `client_secret` is three-valued. Omit it to leave the sealed secret alone,
 * send a value to rotate it, send `""` to clear it and make the provider a
 * public PKCE client.
 */
export interface UpdateSsoProviderInput {
  name: string;
  issuer: string;
  client_id: string;
  client_secret?: string;
  scopes?: string[];
  group_claim?: string;
  default_role?: string;
  enabled: boolean;
}

export function updateSsoProvider(
  id: string,
  input: UpdateSsoProviderInput,
): Promise<SsoProviderRow> {
  return sendJson<SsoProviderRow>("PUT", `/api/v1/sso-providers/${id}`, input);
}

export function deleteSsoProvider(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/sso-providers/${id}`);
}

/** an IdP group name granting a role at a scope inside the provider's own org */
export interface SsoGroupMappingRow {
  id: string;
  provider_id: string;
  group_name: string;
  org_id: string | null;
  team_id: string | null;
  project_id: string | null;
  role: string;
  created_at: string;
}

export interface CreateSsoGroupMappingInput {
  group_name: string;
  role: string;
  /** all three omitted grants at the provider's own org, which is what the
   * dashboard sends; a mapping may never reach outside that org */
  org_id?: string;
  team_id?: string;
  project_id?: string;
}

export function fetchSsoGroupMappings(
  providerId: string,
): Promise<SsoGroupMappingRow[]> {
  return getJson<SsoGroupMappingRow[]>(
    `/api/v1/sso-providers/${providerId}/group-mappings`,
  );
}

export function createSsoGroupMapping(
  providerId: string,
  input: CreateSsoGroupMappingInput,
): Promise<SsoGroupMappingRow> {
  return sendJson<SsoGroupMappingRow>(
    "POST",
    `/api/v1/sso-providers/${providerId}/group-mappings`,
    input,
  );
}

export function deleteSsoGroupMapping(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/sso-group-mappings/${id}`);
}

/**
 * Where a provider's "Continue with …" button points.
 *
 * The same string `auth_policy.rs` builds for `GET /api/v1/auth/methods`, from
 * the slug alone — the admin screen has to show the URL for a provider it just
 * created, which that unauthenticated endpoint only lists once the login screen
 * next reloads.
 */
export function ssoStartPath(slug: string): string {
  return `/auth/sso/${slug}/start`;
}

// --- org sign-in policy (crates/rolter-control/src/auth_policy.rs, #240) ---

/** `OrgAuthPolicy`; the pair decides what the login screen offers this org */
export interface OrgAuthPolicy {
  org_id: string;
  allow_password_login: boolean;
  allow_sso: boolean;
  updated_at: string;
}

export function fetchAuthPolicy(orgId: string): Promise<OrgAuthPolicy> {
  return getJson<OrgAuthPolicy>(`/api/v1/orgs/${orgId}/auth-policy`);
}

/**
 * Both flags travel together, because the server refuses the *combination*
 * rather than either field: turning both off is an outage, and turning
 * passwords off before an enabled provider exists locks every non-superadmin
 * out. Each comes back 409 carrying its own message.
 */
export function updateAuthPolicy(
  orgId: string,
  input: { allow_password_login: boolean; allow_sso: boolean },
): Promise<OrgAuthPolicy> {
  return sendJson<OrgAuthPolicy>(
    "PUT",
    `/api/v1/orgs/${orgId}/auth-policy`,
    input,
  );
}
