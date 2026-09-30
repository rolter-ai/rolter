// finding one request in the log by the id someone was handed (#1861).
//
// the control plane looks a request up by its `request_id` (the `x-request-id`
// the gateway returned to the client) or by its `trace_id` (the W3C trace the
// spans were recorded under). the two are separate parameters, and a pasted
// string does not say which it is, so the shape decides. kept out of the
// screen so the rule is testable on its own and the command palette reads the
// same one.

/** which column of the log a pasted id names */
export type LogLookupKind = "request_id" | "trace_id";

/** an id to look up, and the parameter it is sent as */
export interface LogLookup {
  kind: LogLookupKind;
  value: string;
}

// a W3C trace id is 32 lowercase hex characters, and all zeros is the one
// value the spec rules out
const TRACE_ID = /^[0-9a-f]{32}$/;
const NO_TRACE = /^0+$/;

// `version-traceid-parentid-flags`, as a `traceparent` header carries it, with
// the header's own name allowed in front so a line copied out of `curl -i`
// still reads. the gateway lower-cases a trace id it adopts, so a header
// pasted in capitals is the same trace
const TRACEPARENT = /^(?:traceparent\s*:\s*)?[0-9a-f]{2}-([0-9a-f]{32})-[0-9a-f]{16}-[0-9a-f]{2}$/i;

function isTraceId(value: string): boolean {
  return TRACE_ID.test(value) && !NO_TRACE.test(value);
}

/**
 * What a pasted string is an id of, or `null` when there is nothing to look up.
 *
 * - a `traceparent` header value is reduced to the trace id inside it
 * - 32 lowercase hex characters are a trace id
 * - anything else is a request id, verbatim: a client chooses its own
 *   `x-request-id`, so no other shape can be ruled out
 *
 * A request id that happens to be 32 lowercase hex characters, such as a UUID
 * with its dashes removed, reads as a trace id; `/logs?request_id=…` names it
 * explicitly.
 */
export function parseLogLookup(pasted: string): LogLookup | null {
  const text = pasted.trim();
  if (text === "") return null;
  const header = TRACEPARENT.exec(text)?.[1]?.toLowerCase();
  if (header !== undefined && isTraceId(header)) return { kind: "trace_id", value: header };
  if (isTraceId(text)) return { kind: "trace_id", value: text };
  return { kind: "request_id", value: text };
}

/** The query string that opens the LLM Logs screen on `lookup`. */
export function logLookupSearch(lookup: LogLookup): string {
  return `?${new URLSearchParams({ [lookup.kind]: lookup.value }).toString()}`;
}
