import type { AuditLogEntry, AuditLogPage, AuditLogQuery } from "./api";

// an audit export is evidence, so a row carries everything that identifies it
// whole: the full ids and the instant in UTC (`Z`), which no reader can take
// for a local time. the actor's e-mail is added where the dashboard can name it

/** the server's page ceiling; larger asks are clamped to it */
export const EXPORT_PAGE_SIZE = 500;

/** rows one export reads before it stops and says so (20 pages) */
export const EXPORT_ROW_LIMIT = 10_000;

export type AuditExportFormat = "csv" | "json";

export interface AuditExportRow {
  at: string;
  actor_email: string;
  actor_user_id: string;
  action: string;
  target_type: string;
  target_id: string;
  org_id: string;
  entry_id: string;
  detail: unknown;
}

export function auditExportRow(
  entry: AuditLogEntry,
  emailOf: (id: string) => string | undefined,
): AuditExportRow {
  return {
    at: entry.at,
    actor_email: entry.actor_user_id ? (emailOf(entry.actor_user_id) ?? "") : "",
    actor_user_id: entry.actor_user_id ?? "",
    action: entry.action,
    target_type: entry.target_type ?? "",
    target_id: entry.target_id ?? "",
    org_id: entry.org_id ?? "",
    entry_id: entry.id,
    detail: entry.detail ?? null,
  };
}

const COLUMNS: (keyof AuditExportRow)[] = [
  "at",
  "actor_email",
  "actor_user_id",
  "action",
  "target_type",
  "target_id",
  "org_id",
  "entry_id",
  "detail",
];

function cell(value: unknown): string {
  let text = value == null ? "" : typeof value === "string" ? value : JSON.stringify(value);
  // a spreadsheet runs a cell that starts with one of these as a formula, and
  // a target id or an e-mail is typed by someone other than the reader
  if (/^[=+\-@\t\r]/.test(text)) text = `'${text}`;
  return /[",\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
}

export function auditCsv(rows: AuditExportRow[]): string {
  const lines = [
    COLUMNS.join(","),
    ...rows.map((row) => COLUMNS.map((c) => cell(row[c])).join(",")),
  ];
  return lines.join("\r\n") + "\r\n";
}

export function auditJson(rows: AuditExportRow[]): string {
  return JSON.stringify(rows, null, 2) + "\n";
}

function slug(text: string): string {
  return (
    text
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "org"
  );
}

/**
 * `audit-log-<org>-<window>.<ext>`: the org the rows belong to and the window
 * they cover, `<from day>_to_<today>` or `all_to_<today>`, so two exports of
 * one org never overwrite each other and say what they hold.
 */
export function auditExportName(opts: {
  org: string | "deployment";
  from?: string;
  to?: string;
  format: AuditExportFormat;
}): string {
  const to = (opts.to ?? new Date().toISOString()).slice(0, 10);
  const from = opts.from ? opts.from.slice(0, 10) : "all";
  return `audit-log-${slug(opts.org)}-${from}_to_${to}.${opts.format}`;
}

export interface CollectedAuditLog {
  entries: AuditLogEntry[];
  /** the walk stopped at `limit` while the control plane still had more */
  truncated: boolean;
}

/**
 * Pages through a filtered audit read, newest first, until it runs out or
 * `limit` rows are in hand. The filters ride on every page; the cursor is the
 * server's own, so nothing is skipped or repeated while rows are being added.
 */
export async function collectAuditLog(
  fetchPage: (query: AuditLogQuery) => Promise<AuditLogPage>,
  filters: Omit<AuditLogQuery, "limit" | "cursor" | "include_total">,
  limit: number = EXPORT_ROW_LIMIT,
): Promise<CollectedAuditLog> {
  const entries: AuditLogEntry[] = [];
  let cursor: string | undefined;
  for (;;) {
    const page = await fetchPage({ ...filters, limit: EXPORT_PAGE_SIZE, cursor });
    entries.push(...page.items);
    if (!page.has_next || !page.next_cursor) return { entries, truncated: false };
    if (entries.length >= limit) {
      return { entries: entries.slice(0, limit), truncated: true };
    }
    cursor = page.next_cursor;
  }
}

/** hands `text` to the browser as a file; an object URL, not a data URI, for size */
export function downloadText(name: string, text: string, type: string): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}
