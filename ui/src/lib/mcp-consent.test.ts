import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import type { McpOAuthGrantRow, McpOAuthSessionRow } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import { CONSENT_FAILURES, connectedServers, readConsentResult } from "@/lib/mcp-consent";

const FLOW = fileURLToPath(
  new URL("../../../crates/rolter-control/src/mcp_oauth_flow.rs", import.meta.url),
);

// the string arms of `ConsentFailure::code`, in declaration order
function rustCodes(source: string): string[] {
  const body = source.match(/const fn code\(self\) -> &'static str \{([\s\S]*?)\n {4}\}/);
  if (!body) throw new Error("ConsentFailure::code not found in mcp_oauth_flow.rs");
  return [...body[1].matchAll(/Self::\w+ => "([a-z_]+)"/g)].map((m) => m[1]);
}

describe("the consent failure codes", () => {
  it("match the ones the control plane sends", () => {
    expect<string[]>([...CONSENT_FAILURES]).toEqual(rustCodes(readFileSync(FLOW, "utf8")));
  });

  it("each have copy, as does a code this build has never heard of", () => {
    const reasons = en.pages.mcpOAuth.consent.reasons as Record<string, string>;
    for (const code of [...CONSENT_FAILURES, "unknown"]) {
      expect(reasons[code], code).toBeTruthy();
    }
  });
});

describe("readConsentResult", () => {
  it("reads a completed consent", () => {
    expect(readConsentResult("?consent=completed&session=s-1&server=srv-1")).toEqual({
      outcome: "completed",
      session: "s-1",
      server: "srv-1",
    });
  });

  it("reads a failure, with or without the server", () => {
    expect(readConsentResult("?consent=failed&reason=issuer_mismatch&server=srv-1")).toEqual({
      outcome: "failed",
      reason: "issuer_mismatch",
      server: "srv-1",
    });
    expect(readConsentResult("?consent=failed&reason=state_invalid")).toEqual({
      outcome: "failed",
      reason: "state_invalid",
      server: null,
    });
  });

  it("keeps a code it does not know as unknown rather than as a raw key", () => {
    expect(readConsentResult("?consent=failed&reason=from_the_future")).toMatchObject({
      reason: "unknown",
    });
    expect(readConsentResult("?consent=failed")).toMatchObject({ reason: "unknown" });
  });

  it("finds nothing in a plain visit or a half-written url", () => {
    expect(readConsentResult("")).toBeNull();
    expect(readConsentResult("?consent=completed")).toBeNull();
    expect(readConsentResult("?consent=maybe&session=s-1")).toBeNull();
  });
});

describe("connectedServers", () => {
  const NOW = Date.parse("2026-09-30T12:00:00Z");
  const grant = (over: Partial<McpOAuthGrantRow>): McpOAuthGrantRow => ({
    id: "g-1",
    server_id: "srv-1",
    user_id: "me",
    scopes: [],
    granted_at: "2026-09-30T10:00:00Z",
    revoked_at: null,
    revoked_by: null,
    active: true,
    ...over,
  });
  const session = (over: Partial<McpOAuthSessionRow>): McpOAuthSessionRow => ({
    id: "s-1",
    grant_id: "g-1",
    scopes: [],
    expires_at: "2026-09-30T13:00:00Z",
    refresh_expires_at: null,
    revoked_at: null,
    created_at: "2026-09-30T10:00:00Z",
    last_used_at: null,
    has_refresh_token: false,
    ...over,
  });

  it("names a server the caller holds a live session on", () => {
    expect(connectedServers([grant({})], [session({})], "me", NOW)).toEqual(new Set(["srv-1"]));
  });

  it("ignores a colleague's consent, which an admin's listing also carries", () => {
    expect(connectedServers([grant({ user_id: "them" })], [session({})], "me", NOW).size).toBe(0);
  });

  it("ignores a revoked grant, a revoked session and an expired one", () => {
    const live = [session({})];
    expect(connectedServers([grant({ active: false })], live, "me", NOW).size).toBe(0);
    const revoked = [session({ revoked_at: "2026-09-30T11:00:00Z" })];
    expect(connectedServers([grant({})], revoked, "me", NOW).size).toBe(0);
    const expired = [session({ expires_at: "2026-09-30T11:59:59Z" })];
    expect(connectedServers([grant({})], expired, "me", NOW).size).toBe(0);
  });

  it("knows nothing without an account or before the listings answer", () => {
    expect(connectedServers([grant({})], [session({})], undefined, NOW).size).toBe(0);
    expect(connectedServers(undefined, [session({})], "me", NOW).size).toBe(0);
  });
});
