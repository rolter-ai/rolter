import {
  MCP_RESERVED_AUTH_HEADERS,
  type McpAuthKind,
  type McpServerAuthInput,
  type McpServerRow,
  type McpTransportOverridesPatch,
} from "@/lib/api";

// the MCP server form's wire rules, kept out of the component so each one can
// be pinned by a unit test (#1447). every rule here is a case where the obvious
// form behaviour is the wrong one: an empty credential input must not clear a
// stored secret, an empty timeout must mean "inherit" rather than zero, and a
// field left alone must not be sent at all

/** Whether `kind` presents a static, deployment-wide credential. */
export const carriesCredential = (kind: McpAuthKind): boolean =>
  kind === "bearer" || kind === "header";

export type HeaderNameProblem = "required" | "shape" | "reserved";

// RFC 9110 field-name token, the same class the control plane and the
// `mcp_servers_auth_header_name_shape` constraint accept
const FIELD_NAME = /^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,64}$/;

/** Why a header name would be refused, or `null` when the API would take it. */
export function headerNameProblem(name: string): HeaderNameProblem | null {
  if (!name) return "required";
  if (!FIELD_NAME.test(name)) return "shape";
  const lower = name.toLowerCase();
  if ((MCP_RESERVED_AUTH_HEADERS as readonly string[]).includes(lower)) return "reserved";
  return null;
}

export interface AuthDraft {
  kind: McpAuthKind;
  headerName: string;
  /** never pre-filled: the stored credential is sealed and never read back */
  credential: string;
}

export const authDraft = (server: McpServerRow | null): AuthDraft => ({
  kind: server?.auth_kind ?? "none",
  headerName: server?.auth_header_name ?? "",
  credential: "",
});

/**
 * Whether the draft is one the API would accept against `server`'s stored
 * state. A kind that carries a credential needs one typed unless one is
 * already stored — which it only ever is while the row is `bearer`/`header`.
 */
export function authDraftValid(draft: AuthDraft, server: McpServerRow | null): boolean {
  if (draft.kind === "header" && headerNameProblem(draft.headerName.trim())) return false;
  if (carriesCredential(draft.kind) && !server?.has_credential && !draft.credential) return false;
  return true;
}

/**
 * The `PUT .../auth` body the draft calls for, or `null` when nothing moved.
 *
 * Skipped when unchanged because every call writes an audit entry, and because
 * the endpoint needs `ROLTER_KEK` even for a kind that seals nothing — so an
 * untouched server must stay editable on a deployment without one. The
 * credential is only sent when typed: an empty input means "keep it".
 */
export function authInput(draft: AuthDraft, server: McpServerRow | null): McpServerAuthInput | null {
  const headerName = draft.headerName.trim();
  const current = server?.auth_kind ?? "none";
  const changed =
    draft.kind !== current ||
    (draft.kind === "header" && headerName !== (server?.auth_header_name ?? "")) ||
    (carriesCredential(draft.kind) && !!draft.credential);
  if (!changed) return null;
  const input: McpServerAuthInput = { auth_kind: draft.kind };
  if (draft.kind === "header") input.auth_header_name = headerName;
  if (carriesCredential(draft.kind) && draft.credential) input.credential = draft.credential;
  return input;
}

/**
 * Whether saving the draft deletes a stored credential. Moving to a kind that
 * carries none clears it server-side, and the API has no way to read the old
 * value back, so the form confirms first.
 */
export const dropsCredential = (draft: AuthDraft, server: McpServerRow | null): boolean =>
  !!server?.has_credential && !carriesCredential(draft.kind);

export type OverrideKey = "connect_timeout_ms" | "request_timeout_ms" | "max_retries";

/** The range the control plane enforces for each override, inclusive. */
export const OVERRIDE_BOUNDS: Record<OverrideKey, { min: number; max: number }> = {
  connect_timeout_ms: { min: 100, max: 60_000 },
  request_timeout_ms: { min: 1_000, max: 300_000 },
  max_retries: { min: 0, max: 5 },
};

export const OVERRIDE_KEYS = Object.keys(OVERRIDE_BOUNDS) as OverrideKey[];

export type OverrideDraft = Record<OverrideKey, string>;

export const overrideDraft = (server: McpServerRow | null): OverrideDraft => ({
  connect_timeout_ms: server?.connect_timeout_ms?.toString() ?? "",
  request_timeout_ms: server?.request_timeout_ms?.toString() ?? "",
  max_retries: server?.max_retries?.toString() ?? "",
});

/**
 * One input read as an override: `null` for blank (inherit), a number when it
 * is a whole number inside the bound, `undefined` when it is neither.
 */
export function parseOverride(key: OverrideKey, raw: string): number | null | undefined {
  const value = raw.trim();
  if (!value) return null;
  if (!/^\d+$/.test(value)) return undefined;
  const n = Number(value);
  const { min, max } = OVERRIDE_BOUNDS[key];
  return n >= min && n <= max ? n : undefined;
}

export const overridesValid = (draft: OverrideDraft): boolean =>
  OVERRIDE_KEYS.every((key) => parseOverride(key, draft[key]) !== undefined);

/**
 * Only the overrides that moved. A field left as it was is omitted, which
 * leaves the stored override alone; a field cleared is sent as `null`, which
 * drops it back to inheriting. Sending every field would make "leave it"
 * unsayable.
 */
export function overridesPatch(draft: OverrideDraft, server: McpServerRow | null): McpTransportOverridesPatch {
  const patch: McpTransportOverridesPatch = {};
  for (const key of OVERRIDE_KEYS) {
    const next = parseOverride(key, draft[key]);
    if (next === undefined) continue;
    if (next !== (server?.[key] ?? null)) patch[key] = next;
  }
  return patch;
}

/**
 * Whether an API refusal is the missing deployment KEK rather than anything
 * the operator typed. The control plane sends no machine code for it, so this
 * matches the environment variable's name, which is a stable identifier.
 */
export const isKekMissing = (error: unknown): boolean =>
  error instanceof Error && error.message.includes("ROLTER_KEK");
