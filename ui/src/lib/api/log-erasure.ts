import { getJson, sendJson } from "./request";

/**
 * What one erasure removes (`ScopeBody` in
 * crates/rolter-control/src/log_erasure.rs, #1085). `request` needs the
 * organization, and `ts` narrows a reused request id to one instant.
 */
export type LogErasureScope =
  | { kind: "virtual_key"; id: string }
  | { kind: "user"; id: string }
  | { kind: "org"; id: string }
  | { kind: "request"; request_id: string; org_id: string; ts?: string };

export type LogErasureStatus = "pending" | "completed" | "failed" | "unverified";

/**
 * The record of an erasure. ClickHouse applies a delete as a background
 * mutation, so `pending` is the honest answer to a submission and only a read
 * of the record moves it on (`mutations_done` of `statements` shows how far).
 */
/** one store an erasure searched or skipped, and why (#3436) */
export interface LogErasureStore {
  store: string;
  searched: boolean;
  /** the stable name of why it was skipped, which the dashboard translates */
  code?: string;
  /** the same in English, for a code the dashboard does not know */
  reason?: string;
}

export interface LogErasureRow {
  id: string;
  scope_kind: "virtual_key" | "user" | "org" | "request";
  scope_id: string;
  org_id: string | null;
  request_ts: string | null;
  include_logs: boolean;
  reason: string;
  requested_by: string | null;
  status: LogErasureStatus;
  statements: number;
  error: string | null;
  /** empty on a record from before the stores were kept */
  stores?: LogErasureStore[];
  created_at: string;
  completed_at: string | null;
  /** present on a read of one pending record */
  mutations_done?: number;
  last_error?: string;
}

/** a subject the erasure form can name (#3385) */
export interface LogErasureSubject {
  id: string;
  /** a key's name or prefix, a user's email, an organization's name */
  name: string;
  /** a key's `org / team / project`, a user's display name, an organization's slug */
  detail: string;
}

/** `truncated`: the list was cut at the control plane's cap; `q` narrows it */
export interface LogErasureSubjectList {
  data: LogErasureSubject[];
  truncated: boolean;
}

export type LogErasureSubjectKind = "virtual_key" | "user" | "org";

/**
 * The named subjects of one kind, by part of a name or email (`q`), or the one
 * with `id` to name an id already in hand. Superadmin only, as the erasure is.
 * Without `q` the control plane answers the first few hundred by name.
 */
export function findLogErasureSubjects(
  kind: LogErasureSubjectKind,
  { q, id }: { q?: string; id?: string } = {},
): Promise<LogErasureSubjectList> {
  const query = new URLSearchParams({ kind });
  if (q) query.set("q", q);
  if (id) query.set("id", id);
  return getJson<LogErasureSubjectList>(`/api/v1/log-erasures/subjects?${query}`);
}

export interface CreateLogErasureInput {
  scope: LogErasureScope;
  /** stored and audited: write the ticket reference, not the content */
  reason: string;
  /** `false` deletes the captured bodies only and keeps the metadata */
  include_logs?: boolean;
}

export function createLogErasure(input: CreateLogErasureInput): Promise<LogErasureRow> {
  return sendJson<LogErasureRow>("POST", "/api/v1/log-erasures", input);
}

/** the most recent hundred, newest first; a pending one is not re-read here */
export function fetchLogErasures(): Promise<{ data: LogErasureRow[] }> {
  return getJson<{ data: LogErasureRow[] }>("/api/v1/log-erasures");
}

/** reading a pending record asks ClickHouse how far its mutations got */
export function fetchLogErasure(id: string): Promise<LogErasureRow> {
  return getJson<LogErasureRow>(`/api/v1/log-erasures/${id}`);
}
