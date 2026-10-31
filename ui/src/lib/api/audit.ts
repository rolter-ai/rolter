import type { ExportRequest } from "./analytics-export";
import { getJson } from "./request";

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

/**
 * The whole filtered audit log as CSV or JSON, with no row cap, as an export
 * the control plane streams (#3051): `GET /api/v1/orgs/{org}/audit-log/export`,
 * or the deployment-wide `/api/v1/audit-log/export`. The paging fields of
 * `query` are not sent. Handed to `ExportCsvButton`, which streams it to a
 * picked file or downloads it through a single-use ticket, so the tab never
 * holds the file (#3545). A control plane that predates the endpoint answers
 * `404`, which `isEndpointNotMounted` recognises so the caller can read the
 * pages itself.
 */
export function auditLogExport(
  scope: string | "deployment",
  format: "csv" | "json",
  query: Pick<AuditLogQuery, "actor" | "action" | "target_type" | "from" | "to"> = {},
): ExportRequest {
  const base = scope === "deployment" ? "/api/v1/audit-log" : `/api/v1/orgs/${scope}/audit-log`;
  return {
    path: `${base}/export`,
    query: auditLogQueryString({ ...query, format }).replace(/^\?/, ""),
  };
}

// ---------------------------------------------------------------------------
// security settings (superadmin-only global gateway policy)
