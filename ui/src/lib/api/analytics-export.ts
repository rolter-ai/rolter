// CSV exports of the cost rollups and the request log (#1838); the routes are in
// crates/rolter-control/src/analytics_export.rs, the download tickets (#3504) in
// crates/rolter-control/src/export_tickets.rs
import { getJson, requestResponse, sendJson } from "./request";
import { setRowFilterParams, type AnalyticsRowFilters, type AnalyticsWindow } from "./analytics";
import type { AttributionDimension } from "./analytics";

/**
 * One export as the control plane names it: the route and the query string it
 * is sent with. Built once, it is either fetched with the bearer and streamed to
 * a file the person picked, or handed to a download ticket (#3504).
 */
export interface ExportRequest {
  path: string;
  /** the query string, without the `?` */
  query: string;
}

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

/** Fetch `request` with the bearer, handing back its body as a stream. */
export async function streamExport(
  request: ExportRequest,
  options: AnalyticsExportOptions = {},
): Promise<AnalyticsExportStream> {
  const { path, query } = request;
  // a session-token 401 signs the session out like its JSON twin (#3226)
  const res = await requestResponse(`${path}${query ? `?${query}` : ""}`, {
    signal: options.signal,
  });
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
export function attributionExport(
  window: AnalyticsWindow,
  dimension: AttributionDimension,
): ExportRequest {
  const params = windowParams(window);
  params.set("dimension", dimension);
  params.set("include_unattributed", "true");
  return { path: "/api/v1/analytics/by-attribution/export", query: params.toString() };
}

/** The request log under the LLM Logs filters, metadata only, streamed by the server. */
export function invocationsExport(
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
): ExportRequest {
  const params = windowParams(query);
  setRowFilterParams(params, query);
  if (query.status) params.set("status", query.status);
  if (query.unpriced) params.set("unpriced", "true");
  if (query.withheld) params.set("withheld", "true");
  if (query.usage_unknown) params.set("usage_unknown", "true");
  if (query.guardrail) params.set("guardrail", query.guardrail);
  if (query.plugin) params.set("plugin", query.plugin);
  if (query.outcome) params.set("outcome", query.outcome);
  return { path: "/api/v1/analytics/invocations/export", query: params.toString() };
}

/** A minted download ticket (#3504). */
export interface ExportTicket {
  /** the non-secret id its status is read by */
  id: string;
  /** the same-origin link that downloads the export once; follow it, never fetch or log it */
  download_url: string;
  status_url: string;
  expires_at: string;
}

/** Why an export stopped before its last row. */
export type ExportTruncatedReason = "row_limit" | "time_limit" | "read_error" | "client_closed";

/** Where a ticket's export has got to, as the control plane reports it. */
export type ExportTicketState =
  | { state: "pending" }
  | { state: "streaming"; rows: number }
  | { state: "complete"; rows: number }
  | { state: "truncated"; rows: number; reason: ExportTruncatedReason }
  | { state: "refused"; status: number; retry_after?: number }
  | { state: "expired" };

export type ExportTicketStatus = ExportTicketState & {
  id: string;
  /** the audit log's two only on a control plane with a database (#3545) */
  export: "attribution" | "timeseries" | "invocations" | "audit_log" | "org_audit_log";
  expires_at: string;
  /** the state can no longer change */
  done: boolean;
};

/**
 * Mint a single-use download ticket for `request`. The control plane runs the
 * export's own checks first, so a full export gate is the usual `429` here.
 */
export function mintExportTicket(request: ExportRequest): Promise<ExportTicket> {
  return sendJson<ExportTicket>("POST", "/api/v1/analytics/exports/tickets", {
    path: request.path,
    query: request.query,
  });
}

/** How the download a ticket started is going; only its minter may ask. */
export function exportTicketStatus(id: string): Promise<ExportTicketStatus> {
  return getJson<ExportTicketStatus>(`/api/v1/analytics/exports/tickets/${encodeURIComponent(id)}`);
}
