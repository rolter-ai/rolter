import { getJson, requestResponse } from "./request";

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

function auditLogQueryString(query: object): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== "") {
      params.set(key, String(value));
    }
  }
  const qs = params.toString();
  return qs ? `?${qs}` : "";
}

export function fetchAuditLogPage(orgId: string, query: AuditLogQuery = {}): Promise<AuditLogPage> {
  return getJson<AuditLogPage>(`/api/v1/orgs/${orgId}/audit-log${auditLogQueryString(query)}`);
}

// every audit row in the deployment, org-less account events included;
// superadmin-only (`deployment_audit_log:read`), a 403 for anyone else
export function fetchDeploymentAuditLogPage(query: AuditLogQuery = {}): Promise<AuditLogPage> {
  return getJson<AuditLogPage>(`/api/v1/audit-log${auditLogQueryString(query)}`);
}

/** the filtered audit log as a file the control plane streamed (#3051) */
export interface AuditLogExportFile {
  blob: Blob;
  /** the name the server gave it; absent when it sent no usable one */
  name?: string;
}

/**
 * Asks the control plane for the whole filtered audit log as CSV or JSON, with
 * no row cap: `GET /api/v1/orgs/{org}/audit-log/export` (or the deployment-wide
 * `/api/v1/audit-log/export`). The paging fields of `query` are ignored there.
 * A control plane that predates the endpoint answers `404`, which
 * `isEndpointNotMounted` recognises so the caller can read the pages itself.
 */
export async function fetchAuditLogExport(
  scope: string | "deployment",
  format: "csv" | "json",
  query: Pick<AuditLogQuery, "actor" | "action" | "target_type" | "from" | "to"> = {},
): Promise<AuditLogExportFile> {
  const base = scope === "deployment" ? "/api/v1/audit-log" : `/api/v1/orgs/${scope}/audit-log`;
  const url = `${base}/export${auditLogQueryString({ ...query, format })}`;
  const res = await requestResponse(url);
  const name = /filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1];
  return { blob: await res.blob(), name };
}

// ---------------------------------------------------------------------------
// security settings (superadmin-only global gateway policy)
