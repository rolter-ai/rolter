import {
  type AnalyticsRowFilters,
  type AnalyticsWindow,
  getAnalytics,
  setRowFilterParams,
} from "./analytics";

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
  /// what the request did upstream, beside what the caller was told in
  /// `status` (#2807). `upstream_status` is the HTTP status of the last
  /// upstream attempt that answered, 0 when none did (a refusal, a cache hit,
  /// the built-in model, a connection that failed first). `attempts` is how
  /// many upstream attempts were made, 0 when it never reached one. both are
  /// absent from a control plane that predates them, so a reader treats a
  /// missing field as 0 and says nothing extra
  upstream_status?: number | string;
  attempts?: number | string;
  /// the failure class that sent the request on to the target that answered
  /// it (#1584): `throttled`, `refusal:content_policy_violation` and so on.
  /// "" when the first dial answered, and absent from a control plane that
  /// predates it, so a reader shows the row only when it is non-empty
  fallback_reason?: string;
  cache_read_tokens: number | string;
  cache_write_tokens: number | string;
  /// the part of `cache_write_tokens` Anthropic reports as written to its
  /// 1 hour cache; the rest went to the 5 minute one (#2903). absent from a
  /// control plane that predates it or a ClickHouse without the column, and
  /// 0 on a row older than the column or from a provider that reports no split
  cache_write_1h_tokens?: number | string;
  prompt_tokens: number | string;
  completion_tokens: number | string;
  total_tokens: number | string;
  cost_usd: number | string;
  /// 1 when the model had no price row when the request was served, so
  /// `cost_usd` is unknown rather than zero. recorded per request by the
  /// gateway against the catalogue that applied then, which is why the
  /// dashboard reads it instead of re-deriving it from today's prices (#1226)
  unpriced: number | string;
  /// 1 when the provider answered and billed but an output guardrail or
  /// post_response plugin refused delivery: the caller got a 403, tokens and
  /// cost are what the provider billed, and `error` names the policy.
  /// absent from a control plane that predates it
  withheld?: number | string;
  /// 1 when the upstream answered successfully but reported no usage, so the
  /// zero tokens and cost are unknown rather than free
  usage_unknown?: number | string;
  latency_ms: number | string;
  ttft_ms: number | string;
  error: string;
  request_payload?: string;
  response_payload?: string;
  /// 1 when bodies were captured for this request but the caller's role on its
  /// project is below the payload floor, so the server blanked both. lets the
  /// screen say "hidden for your role" instead of "payload logging is off",
  /// which is what an empty body means otherwise (#1820)
  payload_withheld?: number | string;
  /// which call on a stored response (`/v1/responses/{id}`) the row records:
  /// `retrieve`, `delete`, `cancel`, `input_items`, `compact` or
  /// `input_tokens` (#2836). "" for a request that ran a model and for a row
  /// older than the column, and absent from a control plane that predates it,
  /// so a reader treats all three as "not a lifecycle call" and says nothing
  lifecycle_operation?: string;
  /// what each guardrail did to the request, as `<action>:<rule>` entries with
  /// the action `blocked`, `redacted` or `annotated`, plus the bare
  /// `provider_blocked:<name>` / `provider_transformed:<name>` for the guardrail provider (bare on rows from before the name, #3038)
  /// (#2158). rule names only, never the matched text. absent from a control
  /// plane that predates it, empty for a request no guardrail touched
  guardrail_events?: string[];
  /// what each plugin did to the request, as `<outcome>:<slug>` or
  /// `error:<slug>:<kind>` entries (#2243); absent from a control plane that
  /// predates it, empty for a request no plugin touched
  plugin_events?: string[];
  /**
   * Where the request began in its gateway process's order of starts (#1392), which
   * `ts` cannot say inside one millisecond. Meaningful only among the rows of one
   * gateway instance; `0` on a row older than `clickhouse/021_request_log_seq.sql`.
   * ClickHouse quotes a 64-bit integer, so it may arrive as a string
   */
  seq?: number | string;
  /// the request parameters dialect translation did not forward as sent, as
  /// `<disposition>:<param>` entries (`dropped:top_k`, #1586, #3323); absent
  /// from a control plane that predates it, empty when every parameter went
  /// through
  param_adjustments?: string[];
  /// what happened to the request, by the name the gateway recorded (#3307):
  /// `served`, `upstream_error`, `withheld`, `cancelled` or a `refused_*`
  /// reason; `unknown` on a row older than the column, and absent from a
  /// control plane that predates it
  outcome?: string;
}

/** The outcomes a gateway records, in the order the filter lists them (#3307). */
export const REQUEST_OUTCOMES = [
  "served",
  "upstream_error",
  "withheld",
  "cancelled",
  "refused_budget",
  "refused_rate_limit",
  "refused_guardrail",
  "refused_plugin",
  "refused_access",
  "refused_invalid",
  "refused",
  "unknown",
] as const;

export type RequestOutcome = (typeof REQUEST_OUTCOMES)[number];

export interface InvocationsQuery extends AnalyticsWindow, AnalyticsRowFilters {
  /**
   * Only the requests a guardrail acted on, named as the `guardrails` rollup
   * names it: `blocked:<rule>`, `redacted:<rule>`, `annotated:<rule>`,
   * `provider_blocked:<name>` or `provider_transformed:<name>`, or the bare forms (#2158)
   */
  guardrail?: string;
  /**
   * Only the requests a plugin acted on, named as the `plugins` rollup names
   * its entries: `blocked:<slug>`, `transformed:<slug>`, `annotated:<slug>` or
   * `error:<slug>:<kind>` (#2243)
   */
  plugin?: string;
  /** only the requests the gateway recorded with this outcome (#3307) */
  outcome?: RequestOutcome;
  status?: "all" | "error" | "success";
  /**
   * The `x-request-id` the gateway returned to the client, matched exactly
   * (#1861). Sent without a `since`, it searches every retained row rather than
   * the default window, and it combines with every other filter, so a lookup
   * leaves the rest out.
   */
  request_id?: string;
  /** a W3C trace id, matched exactly; searches every retained row the same way */
  trace_id?: string;
  /**
   * Only the requests the gateway recorded as unpriced. The server narrows on
   * the flag it stored per request, before the page is cut, so a page of 50 is
   * 50 unpriced requests
   */
  unpriced?: boolean;
  /**
   * Only the requests whose response a guardrail or plugin withheld after the
   * provider billed them (#3302), narrowed by the server like `unpriced`
   */
  withheld?: boolean;
  /**
   * Only the requests the upstream answered without reporting usage, so their
   * tokens and cost are unknown rather than zero (#3302)
   */
  usage_unknown?: boolean;
  limit?: number;
  /**
   * The previous page's `next_cursor`, handed back unchanged; omitted for the
   * first page. A keyset position rather than a row offset, so requests the
   * gateway logs while a reader pages cannot shift a row onto two pages or
   * between them (#1410)
   */
  cursor?: string;
}

/** One page of the invocation log and the cursor that resumes after it. */
export interface InvocationsPage {
  data: InvocationRow[];
  /** `null` only for an empty page; a short page still carries one */
  next_cursor: string | null;
}

export function fetchInvocations(query: InvocationsQuery = {}): Promise<InvocationRow[]> {
  return fetchInvocationsPage(query).then((r) => r.data);
}

export function fetchInvocationsPage(query: InvocationsQuery = {}): Promise<InvocationsPage> {
  const params = new URLSearchParams();
  if (query.since) params.set("since", query.since);
  if (query.until) params.set("until", query.until);
  setRowFilterParams(params, query);
  if (query.status) params.set("status", query.status);
  if (query.request_id) params.set("request_id", query.request_id);
  if (query.trace_id) params.set("trace_id", query.trace_id);
  if (query.unpriced) params.set("unpriced", "true");
  if (query.withheld) params.set("withheld", "true");
  if (query.usage_unknown) params.set("usage_unknown", "true");
  if (query.guardrail) params.set("guardrail", query.guardrail);
  if (query.plugin) params.set("plugin", query.plugin);
  if (query.outcome) params.set("outcome", query.outcome);
  if (query.limit != null) params.set("limit", String(query.limit));
  if (query.cursor) params.set("cursor", query.cursor);
  const qs = params.toString();
  return getAnalytics<Partial<InvocationsPage>>(
    `/api/v1/analytics/invocations${qs ? `?${qs}` : ""}`,
  ).then((r) => ({ data: r.data ?? [], next_cursor: r.next_cursor ?? null }));
}
