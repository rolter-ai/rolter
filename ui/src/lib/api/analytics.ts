import { type DataEnvelope, request } from "./request";

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

export async function getAnalytics<T>(
  url: string,
  // a 404 on a collection endpoint means the route is absent from this
  // binary; on a by-id endpoint it means that one record is missing, which
  // is a genuine not-found the caller should surface as such
  { notFoundIsUnavailable = true }: { notFoundIsUnavailable?: boolean } = {},
): Promise<T> {
  // an analytics read never signs the session out: a 401 here is the caller's
  // to show, as it always was
  return request<T>(url, {
    expireSession: false,
    mapError: (res, err) =>
      res.status === 503 || (res.status === 404 && notFoundIsUnavailable)
        ? new AnalyticsUnavailableError(err.message)
        : undefined,
  });
}

export interface AnalyticsWindow {
  since?: string;
  until?: string;
  bucket?: string;
}

/**
 * The row filters the summary, timeseries, by-model and invocation reads share
 * (#2453): the same four names and shapes on each, applied inside the caller's
 * row visibility. An omitted or empty one filters nothing.
 */
export interface AnalyticsRowFilters {
  /** one exact model name */
  model?: string;
  /** one exact virtual key id */
  key?: string;
  /** business unit ids to narrow to; omitted means every unit (#1247) */
  business_unit?: string[];
  /** customer ids to narrow to; omitted means every customer */
  customer?: string[];
  /** one exact project id; omitted means every project the caller may read (#3090) */
  project?: string;
}

// the control plane splits the two sets on commas, so a set travels as one param
export function setRowFilterParams(params: URLSearchParams, filters: AnalyticsRowFilters) {
  if (filters.model) params.set("model", filters.model);
  if (filters.key) params.set("key", filters.key);
  if (filters.project) params.set("project", filters.project);
  if (filters.business_unit?.length) params.set("business_unit", filters.business_unit.join(","));
  if (filters.customer?.length) params.set("customer", filters.customer.join(","));
}

export function windowParams(window: AnalyticsWindow & AnalyticsRowFilters): string {
  const params = new URLSearchParams();
  if (window.since) params.set("since", window.since);
  if (window.until) params.set("until", window.until);
  if (window.bucket) params.set("bucket", window.bucket);
  setRowFilterParams(params, window);
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
  /** requests the provider billed but a guardrail or plugin refused to deliver (403) */
  withheld_requests?: number | string;
  /** what those requests were billed; included in `cost_usd` */
  withheld_cost?: number | string;
  /** successful requests whose upstream reported no usage, so their cost is unknown */
  usage_unknown_requests?: number | string;
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
  withheld_requests?: number | string;
  withheld_cost?: number | string;
  usage_unknown_requests?: number | string;
  errors: number | string;
  p50_latency_ms: number | string;
  p95_latency_ms: number | string;
}

/**
 * the summary is an aggregate with no `group by`, so a healthy control plane
 * always answers exactly one row. an empty `data` still has to become `null`
 * rather than `undefined`: react-query v5 rejects a query function that
 * resolves to `undefined`, which turned a quiet deployment into the "cannot
 * reach the control plane" panel instead of zeroes (#1608)
 */
export function fetchAnalyticsSummary(
  window: AnalyticsWindow & AnalyticsRowFilters = {},
): Promise<AnalyticsSummary | null> {
  return getAnalytics<DataEnvelope<AnalyticsSummary>>(
    `/api/v1/analytics/summary${windowParams(window)}`,
  ).then((r) => r.data[0] ?? null);
}

export function fetchAnalyticsTimeseries(
  window: AnalyticsWindow & AnalyticsRowFilters = {},
): Promise<AnalyticsTimeseriesPoint[]> {
  return getAnalytics<DataEnvelope<AnalyticsTimeseriesPoint>>(
    `/api/v1/analytics/timeseries${windowParams(window)}`,
  ).then((r) => r.data);
}

export function fetchAnalyticsByModel(
  window: AnalyticsWindow & AnalyticsRowFilters = {},
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

/** what a guardrail did to a request: the three a rule can do, two a provider can */
export type GuardrailAction =
  "blocked" | "redacted" | "annotated" | "provider_blocked" | "provider_transformed";

/**
 * One `(action, rule)` of the guardrail outcome rollup (#2158): how many
 * requests the rule acted on over the window. `rule` is the provider's name for its
 * two actions, empty on rows from before the name was recorded. A request counts once per rule however
 * often it matched; the number may arrive as a string, like every ClickHouse
 * count.
 */
export interface GuardrailOutcomeRow {
  action: GuardrailAction | string;
  rule: string;
  requests: number | string;
}

export interface GuardrailOutcomes {
  data: GuardrailOutcomeRow[];
  /**
   * `false` when the ClickHouse table lacks the column the outcomes are kept
   * in (`clickhouse/018_guardrail_events.sql` not applied), so an empty `data`
   * means "not recorded" and not "nothing matched"
   */
  recorded: boolean;
}

/** the outcome rollup; throws AnalyticsUnavailableError without ClickHouse */
export function fetchGuardrailOutcomes(window: AnalyticsWindow = {}): Promise<GuardrailOutcomes> {
  return getAnalytics<Partial<GuardrailOutcomes>>(
    `/api/v1/analytics/guardrails${windowParams(window)}`,
  ).then((r) => ({
    data: Array.isArray(r.data) ? r.data : [],
    recorded: r.recorded !== false,
  }));
}

/**
 * One plugin of the outcome rollup (#2243): how many requests it was consulted
 * on over the window and how they ended. `plugin` is the plugin's slug. The
 * counts may arrive as strings, like every ClickHouse count. A call that got no
 * verdict is an `errors` one, whatever the plugin's failure policy then did to
 * the request, so a fail-closed plugin that refused traffic because its
 * endpoint was down counts there and not under `blocked`.
 */
export interface PluginOutcomeRow {
  /** a slug is unique within an organization only */
  org_id: string;
  plugin: string;
  calls: number | string;
  blocked: number | string;
  transformed: number | string;
  annotated: number | string;
  errors: number | string;
  /** the newest failure, `null` while `errors` is 0 */
  last_error_at: string | null;
  /** `refused`, `timeout`, `connect`, `body` or `status_<code>`; empty while `errors` is 0 */
  last_error_kind: string;
}

export interface PluginOutcomes {
  data: PluginOutcomeRow[];
  /**
   * `false` when the ClickHouse table lacks the column the calls are kept in
   * (`clickhouse/019_plugin_events.sql` not applied), so an empty `data` means
   * "not recorded" and not "no calls yet"
   */
  recorded: boolean;
}

/** the plugin rollup; throws AnalyticsUnavailableError without ClickHouse */
export function fetchPluginOutcomes(window: AnalyticsWindow = {}): Promise<PluginOutcomes> {
  return getAnalytics<Partial<PluginOutcomes>>(
    `/api/v1/analytics/plugins${windowParams(window)}`,
  ).then((r) => ({
    data: Array.isArray(r.data) ? r.data : [],
    recorded: r.recorded !== false,
  }));
}
