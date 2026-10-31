import { getJson, sendJson } from "./request";

/**
 * What the sheet needs to know about a provider kind: whether `api_base` must
 * already end in the API version prefix, which endpoint a request reaches and
 * which header carries the key.
 *
 * Served by the control plane rather than restated here: the version rule
 * covers ~38 of 49 kinds, and a copy in TypeScript would drift the moment a
 * kind is added (#947). The endpoint and the header come from the functions the
 * gateway's forwarder calls (`ProviderKind::primary_upstream_path`,
 * `ProviderKind::auth_header`), so the preview cannot name another address than
 * the one that is called (#2811).
 */
export interface ProviderKindInfo {
  kind: string;
  base_includes_v1: boolean;
  /**
   * what the kind is chiefly called for; a text-embeddings server is not asked
   * for chat, and a System One decision server answers only decisions
   */
  request: "chat" | "embeddings" | "decision";
  /**
   * the upstream path that request is sent to, before the `/v1` rule is applied;
   * `{model}` stands for the upstream model name where the path carries one
   */
  request_path: string;
  /** the lowercase header the key travels in; `authorization` is a bearer token */
  auth_header: string;
  /**
   * the base a hosted kind is pinned to unless the provider opts out with
   * `allow_custom_api_base`; `null`, or absent from an older control plane, for
   * a kind with no pin (#1133)
   */
  pinned_api_base?: string | null;
  /**
   * the specification registry records no located API reference for the kind,
   * so whether it speaks an OpenAI-compatible API at a stable base is
   * unverified (#3428); absent from an older control plane
   */
  unverified?: boolean;
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
  /** the kind's own `request_path`; there is no default, since one for every kind was the bug (#2811) */
  path: string,
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
 * `PROVIDER_KINDS` allowlist in `crates/rolter-control/src/crud/providers.rs`.
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
  "system_one",
  "infinity",
] as const;

/**
 * Where a provider publishes KV-cache events, read by `precise_cache_aware`
 * (#2236). A row always carries every key; a request may leave all but
 * `endpoint` out and the control plane fills the defaults.
 */
export interface KvEventsConfig {
  /** a `tcp://` address */
  endpoint: string;
  topic: string;
  max_blocks: number;
  stale_secs: number;
}

/** Where an LMCache controller is polled, read by `lmcache_aware` (#2236). */
export interface LmCacheConfig {
  /** an http(s) address */
  endpoint: string;
  refresh_secs: number;
  stale_secs: number;
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
  /**
   * The project this provider is scoped to: only keys minted in it reach the
   * provider, through a route or `slug/model`. `null` (or absent, from a control
   * plane that predates scoping) is org-wide (#1919)
   */
  project_id?: string | null;
  /** the KV-events source `precise_cache_aware` reads; `null` when none is set */
  kv_events?: KvEventsConfig | null;
  /** the LMCache source `lmcache_aware` reads; `null` when none is set */
  lmcache?: LmCacheConfig | null;
  /**
   * Whether the provider opted out of its kind's host pin, so `api_base` may
   * name another host; absent from a control plane older than #1133
   */
  allow_custom_api_base?: boolean;
  /**
   * Whether a sealed key is stored for the provider. An `api_key_env` does not
   * count, and the key itself is never returned. Absent from a control plane
   * that predates the field
   */
  has_stored_key?: boolean;
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
  /** scope the provider to a project of the org; omit for org-wide */
  project_id?: string;
  /** omit for no KV-events source; keys other than `endpoint` take their defaults */
  kv_events?: Partial<KvEventsConfig> & Pick<KvEventsConfig, "endpoint">;
  /** omit for no LMCache source; keys other than `endpoint` take their defaults */
  lmcache?: Partial<LmCacheConfig> & Pick<LmCacheConfig, "endpoint">;
  /** opt out of the kind's host pin; omit for the pinned default */
  allow_custom_api_base?: boolean;
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
  /** omit to leave the scope unchanged; a project id scopes it; `null` makes it org-wide */
  project_id?: string | null;
  /** omit to keep; `null` clears the source; an object replaces the whole setting */
  kv_events?: (Partial<KvEventsConfig> & Pick<KvEventsConfig, "endpoint">) | null;
  /** omit to keep; `null` clears the source; an object replaces the whole setting */
  lmcache?: (Partial<LmCacheConfig> & Pick<LmCacheConfig, "endpoint">) | null;
  /** omit to keep */
  allow_custom_api_base?: boolean;
}

export function fetchProviders(orgId: string): Promise<ProviderRow[]> {
  return getJson<ProviderRow[]>(`/api/v1/orgs/${orgId}/providers`);
}

export function createProvider(orgId: string, input: CreateProviderInput): Promise<ProviderRow> {
  return sendJson<ProviderRow>("POST", `/api/v1/orgs/${orgId}/providers`, input);
}

export function updateProvider(id: string, input: UpdateProviderInput): Promise<ProviderRow> {
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
  /**
   * whether the catalogue lists the model the test was asked about: true or
   * false when the provider answered with a catalogue, null (or absent) when no
   * model was asked about or the provider has no catalogue to check (#2008)
   */
  model_listed?: boolean | null;
  error?: string | null;
}

/**
 * Probe a stored provider and report whether it answers.
 *
 * Hits the same free model-list endpoint the gateway's health sweep uses, so a
 * green result here means the sweep will agree. Costs nothing to run — it is a
 * catalogue call, not a completion.
 */
export function testProvider(id: string, model?: string): Promise<ProviderTestResult> {
  const query = model ? `?model=${encodeURIComponent(model)}` : "";
  return sendJson<ProviderTestResult>("POST", `/api/v1/providers/${id}/test${query}`);
}

/** What a probe established about one provider: the stored verdict row. */
export interface ProviderCacheVerdict {
  provider_id: string;
  /** whether the provider's responses carried a prompt-cache usage field */
  reports: boolean;
  /** which usage field named it, `null` when the provider reports nothing */
  spelling: string | null;
  /** the upstream model the probe asked */
  model: string;
  /** when the probe ran (ISO) */
  probed_at: string;
}

/** The answer of one cache probe. `recorded: false` learned nothing and left the old verdict. */
export interface ProviderCacheProbeResult {
  recorded: boolean;
  verdict: ProviderCacheVerdict | null;
  probed_url: string;
  status: number | null;
  latency_ms: number;
  /** why nothing was learned, when `recorded` is false */
  error: string | null;
}

/**
 * Kinds the cache probe refuses with a 400: they are not reached by an
 * OpenAI-shaped completion (or complete nothing at all), so there is nothing
 * to ask. Mirrors the control plane's list in the docs for `probe-cache`.
 */
const UNPROBEABLE_KINDS: ReadonlySet<string> = new Set([
  "bedrock",
  "vertex",
  "azure_openai",
  "gemini_native",
  "gemini_interactions",
  "tei",
  "infinity",
  "system_one",
]);

export function canProbeCache(kind: string): boolean {
  return !UNPROBEABLE_KINDS.has(kind);
}

/**
 * The stored cache-reporting verdicts of an org. A provider with no row has
 * never been probed, which is not the same as one that does not report.
 */
export function fetchProviderCacheProbes(orgId: string): Promise<ProviderCacheVerdict[]> {
  return getJson<ProviderCacheVerdict[]>(`/api/v1/orgs/${orgId}/provider-cache-probes`);
}

/**
 * Probe whether a provider reports prompt-cache metrics. This sends a real
 * one-token completion to the upstream, so it costs a request.
 */
export function probeProviderCache(id: string, model: string): Promise<ProviderCacheProbeResult> {
  return sendJson<ProviderCacheProbeResult>(
    "POST",
    `/api/v1/providers/${id}/probe-cache?model=${encodeURIComponent(model)}`,
  );
}

/** The upstream model ids a stored provider lists, sorted. Empty when it listed none. */
export interface ProviderModelList {
  models: string[];
}

/**
 * Ask a stored provider's upstream which models it serves.
 *
 * It is the catalogue the connection test counts, with the ids kept: what a
 * route target can name as its upstream model. An upstream that is down, or
 * answers with something other than a catalogue, gives an empty list rather
 * than an error, since the list only ever suggests; "Test connection" is the
 * call that says why.
 */
export function fetchProviderModels(id: string): Promise<ProviderModelList> {
  return getJson<ProviderModelList>(`/api/v1/providers/${id}/models`);
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
  /** the project the group is scoped to; `null` or absent is org-wide (#1919) */
  project_id?: string | null;
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
  /** scope the group to a project of the org; omit for org-wide */
  project_id?: string;
}

export interface UpdateProviderGroupInput {
  name?: string;
  slug?: string;
  /** required to change the otherwise-immutable slug */
  allow_slug_change?: boolean;
  strategy?: string;
  /** present = replace the whole membership; omit = leave unchanged */
  members?: GroupMemberInput[];
  /** omit to leave the scope unchanged; a project id scopes it; `null` makes it org-wide */
  project_id?: string | null;
}

export function fetchProviderGroups(orgId: string): Promise<ProviderGroupRow[]> {
  return getJson<ProviderGroupRow[]>(`/api/v1/orgs/${orgId}/provider-groups`);
}

export function createProviderGroup(
  orgId: string,
  input: CreateProviderGroupInput,
): Promise<ProviderGroupRow> {
  return sendJson<ProviderGroupRow>("POST", `/api/v1/orgs/${orgId}/provider-groups`, input);
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
