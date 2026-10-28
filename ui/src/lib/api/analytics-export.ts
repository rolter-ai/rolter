// CSV exports of the cost rollups and the request log (#1838); the routes are in
// crates/rolter-control/src/analytics_export.rs
import { requestResponse } from "./request";
import { setRowFilterParams, type AnalyticsRowFilters, type AnalyticsWindow } from "./analytics";
import type { AttributionDimension } from "./analytics";

/** a finished export: the file and the name the control plane gave it */
export interface AnalyticsExportFile {
  blob: Blob;
  /** names the window and currency; absent only if the server sent no name */
  name?: string;
}

async function download(path: string, params: URLSearchParams): Promise<AnalyticsExportFile> {
  const qs = params.toString();
  // an analytics read never signs the session out, like its JSON twin
  const res = await requestResponse(`${path}${qs ? `?${qs}` : ""}`, { expireSession: false });
  const name = /filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1];
  return { blob: await res.blob(), name };
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
): Promise<AnalyticsExportFile> {
  const params = windowParams(window);
  params.set("dimension", dimension);
  params.set("include_unattributed", "true");
  return download("/api/v1/analytics/by-attribution/export", params);
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
): Promise<AnalyticsExportFile> {
  const params = windowParams(query);
  setRowFilterParams(params, query);
  if (query.status) params.set("status", query.status);
  if (query.unpriced) params.set("unpriced", "true");
  if (query.withheld) params.set("withheld", "true");
  if (query.usage_unknown) params.set("usage_unknown", "true");
  if (query.guardrail) params.set("guardrail", query.guardrail);
  if (query.plugin) params.set("plugin", query.plugin);
  if (query.outcome) params.set("outcome", query.outcome);
  return download("/api/v1/analytics/invocations/export", params);
}
