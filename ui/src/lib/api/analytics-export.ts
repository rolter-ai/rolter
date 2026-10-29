// CSV exports of the cost rollups and the request log (#1838); the routes are in
// crates/rolter-control/src/analytics_export.rs
import { requestResponse } from "./request";
import { setRowFilterParams, type AnalyticsRowFilters, type AnalyticsWindow } from "./analytics";
import type { AttributionDimension } from "./analytics";

/**
 * An export the control plane has started to send: the body is a stream, not
 * a Blob, so the caller decides where it goes (#3434) and nothing here holds the
 * whole file.
 */
export interface AnalyticsExportStream {
  stream: ReadableStream<Uint8Array> | null;
  /** names the window and currency; absent only if the server sent no name */
  name?: string;
}

/** Options every export takes: `signal` abandons it, the body included. */
export interface AnalyticsExportOptions {
  signal?: AbortSignal;
}

async function download(
  path: string,
  params: URLSearchParams,
  options: AnalyticsExportOptions,
): Promise<AnalyticsExportStream> {
  const qs = params.toString();
  // a session-token 401 signs the session out like its JSON twin (#3226)
  const res = await requestResponse(`${path}${qs ? `?${qs}` : ""}`, { signal: options.signal });
  const name = /filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1];
  return { stream: res.body, name };
}

function windowParams(window: AnalyticsWindow): URLSearchParams {
  const params = new URLSearchParams();
  if (window.since) params.set("since", window.since);
  if (window.until) params.set("until", window.until);
  return params;
}

/** The Cost Attribution rollup for one dimension, unattributed spend included. */
export function exportAttributionCsv(
  window: AnalyticsWindow,
  dimension: AttributionDimension,
  options: AnalyticsExportOptions = {},
): Promise<AnalyticsExportStream> {
  const params = windowParams(window);
  params.set("dimension", dimension);
  params.set("include_unattributed", "true");
  return download("/api/v1/analytics/by-attribution/export", params, options);
}

/** The request log under the LLM Logs filters, metadata only, streamed by the server. */
export function exportInvocationsCsv(
  query: AnalyticsWindow &
    AnalyticsRowFilters & {
      status?: "all" | "error" | "success";
      unpriced?: boolean;
      withheld?: boolean;
      usage_unknown?: boolean;
      guardrail?: string;
      plugin?: string;
      outcome?: string;
    },
  options: AnalyticsExportOptions = {},
): Promise<AnalyticsExportStream> {
  const params = windowParams(query);
  setRowFilterParams(params, query);
  if (query.status) params.set("status", query.status);
  if (query.unpriced) params.set("unpriced", "true");
  if (query.withheld) params.set("withheld", "true");
  if (query.usage_unknown) params.set("usage_unknown", "true");
  if (query.guardrail) params.set("guardrail", query.guardrail);
  if (query.plugin) params.set("plugin", query.plugin);
  if (query.outcome) params.set("outcome", query.outcome);
  return download("/api/v1/analytics/invocations/export", params, options);
}
