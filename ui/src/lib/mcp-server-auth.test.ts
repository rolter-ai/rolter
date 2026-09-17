import { describe, expect, it } from "bun:test";

import type { McpServerRow } from "@/lib/api";
import {
  authDraft,
  authDraftValid,
  authInput,
  dropsCredential,
  headerNameProblem,
  isKekMissing,
  overrideDraft,
  overridesPatch,
  overridesValid,
  parseOverride,
} from "@/lib/mcp-server-auth";

const row = (over: Partial<McpServerRow> = {}): McpServerRow => ({
  id: "s1",
  org_id: "o1",
  name: "GitHub",
  slug: "github",
  url: "https://example.com/mcp",
  transport: "streamable_http",
  description: "",
  enabled: true,
  tools: [],
  source: "custom",
  required_scopes: [],
  created_at: "2026-01-01T00:00:00Z",
  authorize_url: null,
  token_url: null,
  client_id: null,
  default_scopes: [],
  has_client_secret: false,
  auth_kind: "none",
  auth_header_name: null,
  has_credential: false,
  connect_timeout_ms: null,
  request_timeout_ms: null,
  max_retries: null,
  ...over,
});

describe("headerNameProblem", () => {
  it("accepts an ordinary api key header", () => {
    expect(headerNameProblem("X-Api-Key")).toBeNull();
  });

  it("refuses every reserved header, whatever its case", () => {
    for (const name of ["Authorization", "HOST", "content-length", "Content-Type", "Connection", "Transfer-Encoding", "Upgrade", "TE", "Trailer", "Proxy-Authorization"]) {
      expect(headerNameProblem(name)).toBe("reserved");
    }
  });

  it("refuses what is not an RFC 9110 field name", () => {
    expect(headerNameProblem("X Api Key")).toBe("shape");
    expect(headerNameProblem("X-Api:Key")).toBe("shape");
    expect(headerNameProblem("x".repeat(65))).toBe("shape");
    expect(headerNameProblem("")).toBe("required");
  });
});

describe("authInput", () => {
  it("sends nothing when the draft matches the row", () => {
    const server = row({ auth_kind: "header", auth_header_name: "X-Api-Key", has_credential: true });
    expect(authInput(authDraft(server), server)).toBeNull();
  });

  // the load-bearing rule: an empty credential input keeps the stored secret
  it("renames a header without sending the credential", () => {
    const server = row({ auth_kind: "header", auth_header_name: "X-Api-Key", has_credential: true });
    expect(authInput({ ...authDraft(server), headerName: "X-Token" }, server)).toEqual({ auth_kind: "header", auth_header_name: "X-Token" });
  });

  it("sends a typed credential and no header name for bearer", () => {
    expect(authInput({ kind: "bearer", headerName: "X-Left-Over", credential: "tok" }, row())).toEqual({ auth_kind: "bearer", credential: "tok" });
  });

  it("never sends a credential for a kind that carries none", () => {
    const server = row({ auth_kind: "bearer", has_credential: true });
    expect(authInput({ kind: "none", headerName: "", credential: "typed-then-switched" }, server)).toEqual({ auth_kind: "none" });
  });
});

describe("authDraftValid", () => {
  it("needs a credential to arm bearer on a server that stores none", () => {
    expect(authDraftValid({ kind: "bearer", headerName: "", credential: "" }, row())).toBe(false);
    expect(authDraftValid({ kind: "bearer", headerName: "", credential: "tok" }, row())).toBe(true);
  });

  it("lets a stored credential carry over from bearer to header", () => {
    const server = row({ auth_kind: "bearer", has_credential: true });
    expect(authDraftValid({ kind: "header", headerName: "X-Api-Key", credential: "" }, server)).toBe(true);
    expect(authDraftValid({ kind: "header", headerName: "Authorization", credential: "" }, server)).toBe(false);
  });
});

describe("dropsCredential", () => {
  it("is only true when a stored credential would be cleared", () => {
    const server = row({ auth_kind: "bearer", has_credential: true });
    expect(dropsCredential({ kind: "none", headerName: "", credential: "" }, server)).toBe(true);
    expect(dropsCredential({ kind: "oauth", headerName: "", credential: "" }, server)).toBe(true);
    expect(dropsCredential({ kind: "header", headerName: "X", credential: "" }, server)).toBe(false);
    expect(dropsCredential({ kind: "none", headerName: "", credential: "" }, row())).toBe(false);
  });
});

describe("transport overrides", () => {
  it("reads blank as inherit and refuses out-of-range or fractional values", () => {
    expect(parseOverride("request_timeout_ms", "")).toBeNull();
    expect(parseOverride("request_timeout_ms", "120000")).toBe(120000);
    expect(parseOverride("request_timeout_ms", "999")).toBeUndefined();
    expect(parseOverride("connect_timeout_ms", "1.5")).toBeUndefined();
    expect(parseOverride("max_retries", "0")).toBe(0);
    expect(parseOverride("max_retries", "6")).toBeUndefined();
    expect(overridesValid({ connect_timeout_ms: "", request_timeout_ms: "abc", max_retries: "" })).toBe(false);
  });

  // absent leaves an override, null drops it: the two must stay distinct
  it("omits untouched fields and sends null for a cleared one", () => {
    const server = row({ connect_timeout_ms: 2000, request_timeout_ms: 60000, max_retries: 2 });
    const draft = { ...overrideDraft(server), request_timeout_ms: "", max_retries: "0" };
    expect(overridesPatch(draft, server)).toEqual({ request_timeout_ms: null, max_retries: 0 });
    expect(overridesPatch(overrideDraft(server), server)).toEqual({});
  });

  it("sends nothing for a new server left inheriting", () => {
    expect(overridesPatch(overrideDraft(null), null)).toEqual({});
  });
});

describe("isKekMissing", () => {
  it("recognises the control plane's refusal by the variable it names", () => {
    expect(isKekMissing(new Error("storing an MCP credential requires the ROLTER_KEK environment variable on the control plane to seal it at rest"))).toBe(true);
    expect(isKekMissing(new Error("auth_kind 'bearer' requires a credential"))).toBe(false);
    expect(isKekMissing("ROLTER_KEK")).toBe(false);
  });
});
