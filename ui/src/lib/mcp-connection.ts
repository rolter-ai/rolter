import {
  MCP_CONNECTION_ERRORS,
  MCP_PROBE_STATUSES,
  type McpServerRow,
  type McpToolCallOutcome,
} from "@/lib/api";
import type { BadgeTone } from "@/components/ui/badge";

/** the server row's contact fields, which is all the card's status reads */
export type McpContactFields = Pick<
  McpServerRow,
  | "last_contact_at"
  | "last_protocol_version"
  | "last_server_name"
  | "last_error_kind"
  | "last_error_at"
>;

/** a failure kind as the card names it; `unknown` is a code this build has not heard of */
export type ConnectionFailure = (typeof MCP_CONNECTION_ERRORS)[number] | "unknown";

export interface ConnectionView {
  /**
   * `unchecked`: no handshake and no failure ever recorded. `reachable`: the
   * last contact is the newest event. `failing`: the last failure is.
   */
  state: "unchecked" | "reachable" | "failing";
  tone: BadgeTone;
  /** when the current state began: the last contact, or the last failure */
  at: string | null;
  /** the current failure, while `failing` */
  failure: ConnectionFailure | null;
  /** a failure older than the last contact, kept after the server recovered */
  earlier: { failure: ConnectionFailure; at: string } | null;
}

const failureKind = (kind: string | null): ConnectionFailure | null =>
  kind === null
    ? null
    : (MCP_CONNECTION_ERRORS as readonly string[]).includes(kind)
      ? (kind as ConnectionFailure)
      : "unknown";

const stamp = (value: string | null) => (value ? Date.parse(value) : Number.NaN);

/**
 * The card's connection state from the last contact and the last failure.
 *
 * The control plane keeps the last failure after a later success, so neither
 * field alone says how the server is now: whichever is later does. A server
 * that answered after it last failed is reachable, and the old failure is
 * still reported as history. A tie reads as reachable, since a probe records
 * one or the other.
 */
export function connectionView(server: McpContactFields): ConnectionView {
  const failure = server.last_error_at ? failureKind(server.last_error_kind) : null;
  const failedAt = failure ? stamp(server.last_error_at) : Number.NaN;
  const contactAt = stamp(server.last_contact_at);
  const hasContact = !Number.isNaN(contactAt);
  const hasFailure = failure !== null && !Number.isNaN(failedAt);

  if (hasFailure && (!hasContact || failedAt > contactAt)) {
    return { state: "failing", tone: "danger", at: server.last_error_at, failure, earlier: null };
  }
  if (hasContact) {
    return {
      state: "reachable",
      tone: "success",
      at: server.last_contact_at,
      failure: null,
      earlier: hasFailure ? { failure, at: server.last_error_at as string } : null,
    };
  }
  return { state: "unchecked", tone: "neutral", at: null, failure: null, earlier: null };
}

/** the statuses a connection test and a tool call can end in, plus a tool's own error */
const PROBE_TONE: Record<string, BadgeTone> = {
  reachable: "success",
  success: "success",
  unreachable: "danger",
  handshake_failed: "danger",
  auth_rejected: "danger",
  egress_denied: "warning",
  needs_session: "info",
  credential_unavailable: "warning",
  // the server answered; the tool is what failed
  tool_errored: "warning",
};

/** a status or outcome code from the API; one this build does not know reads as `unknown` */
export type ProbeLabel = McpToolCallOutcome | "reachable" | "unchecked" | "unknown";

/**
 * The key (under `pages.mcpCatalog.probe.statuses`) naming a status. Every code
 * has its own text, so a state is never told by its colour alone, and a code a
 * newer control plane adds falls back to `unknown` instead of a raw string.
 */
export function probeLabel(code: string | null | undefined): ProbeLabel {
  if (!code) return "unknown";
  return code === "success" || code === "tool_errored" || code === "unchecked"
    ? code
    : (MCP_PROBE_STATUSES as readonly string[]).includes(code)
      ? (code as ProbeLabel)
      : "unknown";
}

export function probeTone(code: string | null | undefined): BadgeTone {
  return PROBE_TONE[code ?? ""] ?? "neutral";
}

/** the card's label for a failure kind; shares the probe vocabulary */
export const failureLabel = (failure: ConnectionFailure): ProbeLabel => probeLabel(failure);

export type ArgumentsParse =
  | { ok: true; value: Record<string, unknown> }
  | { ok: false; reason: "invalid_json" | "not_object"; detail: string };

/**
 * Tool arguments the operator typed. Blank means no arguments, an empty object;
 * anything that parses to something other than an object (an array, a number,
 * `null`) is refused here instead of as a 400 from the control plane.
 */
export function parseToolArguments(text: string): ArgumentsParse {
  if (!text.trim()) return { ok: true, value: {} };
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    return {
      ok: false,
      reason: "invalid_json",
      detail: error instanceof Error ? error.message : String(error),
    };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed))
    return { ok: false, reason: "not_object", detail: "" };
  return { ok: true, value: parsed as Record<string, unknown> };
}

const PLACEHOLDER: Record<string, unknown> = {
  string: "",
  number: 0,
  integer: 0,
  boolean: false,
  array: [],
  object: {},
};

/**
 * Starting arguments for a tool: its required properties, each given the empty
 * value of its type, so the operator edits a shape instead of writing one. A
 * schema that names none (or is not an object schema at all) gives `{}`.
 */
export function argumentsSkeleton(schema: unknown): string {
  const out: Record<string, unknown> = {};
  if (schema && typeof schema === "object") {
    const { required, properties } = schema as { required?: unknown; properties?: unknown };
    const props = properties && typeof properties === "object" ? properties : {};
    if (Array.isArray(required)) {
      for (const name of required) {
        if (typeof name !== "string") continue;
        const type = (props as Record<string, { type?: unknown } | undefined>)[name]?.type;
        out[name] = typeof type === "string" && type in PLACEHOLDER ? PLACEHOLDER[type] : "";
      }
    }
  }
  return JSON.stringify(out, null, 2);
}

/** the shape the control plane swaps in for a payload too large to keep */
export function truncatedPayload(value: unknown): { sizeBytes: number } | null {
  if (!value || typeof value !== "object") return null;
  const { truncated, size_bytes } = value as { truncated?: unknown; size_bytes?: unknown };
  return truncated === true && typeof size_bytes === "number" ? { sizeBytes: size_bytes } : null;
}

/** a raw JSON-RPC message for the code block; `null` (not recorded) has no text */
export const prettyPayload = (value: unknown): string | null =>
  value === null || value === undefined ? null : JSON.stringify(value, null, 2);
