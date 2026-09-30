import { describe, expect, it } from "bun:test";

import { logLookupSearch, parseLogLookup } from "./log-lookup";

const TRACE = "0af7651916cd43dd8448eb211c80319c";

describe("parseLogLookup", () => {
  it("reads 32 lowercase hex characters as a trace id", () => {
    expect(parseLogLookup(TRACE)).toEqual({ kind: "trace_id", value: TRACE });
  });

  it("takes the trace id out of a traceparent header value", () => {
    expect(parseLogLookup(`00-${TRACE}-b7ad6b7169203331-01`)).toEqual({
      kind: "trace_id",
      value: TRACE,
    });
  });

  it("reads a traceparent line copied with its header name", () => {
    expect(parseLogLookup(`traceparent: 00-${TRACE}-b7ad6b7169203331-01`)).toEqual({
      kind: "trace_id",
      value: TRACE,
    });
    expect(parseLogLookup(`Traceparent:00-${TRACE}-b7ad6b7169203331-01`)?.kind).toBe("trace_id");
  });

  it("lower-cases a traceparent pasted in capitals, the way the gateway stores it", () => {
    expect(parseLogLookup(`00-${TRACE.toUpperCase()}-B7AD6B7169203331-01`)).toEqual({
      kind: "trace_id",
      value: TRACE,
    });
  });

  it("trims what the clipboard carried around the id", () => {
    expect(parseLogLookup(`  ${TRACE}\n`)).toEqual({ kind: "trace_id", value: TRACE });
    expect(parseLogLookup("\t3f2c9a1e-7b4d-4f10-9c2e-0a1b2c3d4e5f ")).toEqual({
      kind: "request_id",
      value: "3f2c9a1e-7b4d-4f10-9c2e-0a1b2c3d4e5f",
    });
  });

  it("reads anything else as a request id, verbatim", () => {
    for (const id of ["req-1", "3f2c9a1e-7b4d-4f10-9c2e-0a1b2c3d4e5f", "Order_42/retry#3"]) {
      expect(parseLogLookup(id)).toEqual({ kind: "request_id", value: id });
    }
  });

  it("does not read a near miss as a trace id", () => {
    // one character short, one too many, and a capital in the bare form
    expect(parseLogLookup(TRACE.slice(1))?.kind).toBe("request_id");
    expect(parseLogLookup(`${TRACE}a`)?.kind).toBe("request_id");
    expect(parseLogLookup(TRACE.toUpperCase())?.kind).toBe("request_id");
    // a trace id of all zeros is the value W3C rules out
    expect(parseLogLookup("0".repeat(32))?.kind).toBe("request_id");
    expect(parseLogLookup(`00-${"0".repeat(32)}-b7ad6b7169203331-01`)?.kind).toBe("request_id");
  });

  it("finds nothing to look up in an empty paste", () => {
    expect(parseLogLookup("")).toBeNull();
    expect(parseLogLookup("  \n\t ")).toBeNull();
  });
});

describe("logLookupSearch", () => {
  it("names the parameter the control plane reads", () => {
    expect(logLookupSearch({ kind: "request_id", value: "req-1" })).toBe("?request_id=req-1");
    expect(logLookupSearch({ kind: "trace_id", value: TRACE })).toBe(`?trace_id=${TRACE}`);
  });

  it("encodes an id that is not address-safe", () => {
    const search = logLookupSearch({ kind: "request_id", value: "a&b=c d/é" });
    expect(new URLSearchParams(search).get("request_id")).toBe("a&b=c d/é");
    expect(search).not.toContain("&b=");
  });
});
