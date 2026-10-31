import { describe, expect, test } from "bun:test";

import type { McpOAuthGrantRow, McpOAuthSessionRow } from "@/lib/api";
import { heldScopes, missingScopes, parseScopes } from "@/lib/mcp-step-up";

const NOW = Date.parse("2026-10-01T00:00:00Z");

const grant = (over: Partial<McpOAuthGrantRow> = {}): McpOAuthGrantRow => ({
  id: "g-1",
  server_id: "srv-1",
  user_id: "u-1",
  scopes: ["read"],
  granted_at: "2026-09-01T00:00:00Z",
  revoked_at: null,
  revoked_by: null,
  active: true,
  ...over,
});

const session = (over: Partial<McpOAuthSessionRow> = {}): McpOAuthSessionRow => ({
  id: "s-1",
  grant_id: "g-1",
  scopes: ["read"],
  expires_at: "2026-10-02T00:00:00Z",
  refresh_expires_at: null,
  revoked_at: null,
  created_at: "2026-09-01T00:00:00Z",
  last_used_at: null,
  has_refresh_token: true,
  ...over,
});

describe("heldScopes", () => {
  test("reads the live session of the caller's own grant", () => {
    expect(heldScopes([grant()], [session()], "u-1", "srv-1", NOW)).toEqual(["read"]);
  });

  test("prefers the session that is live the longest", () => {
    const later = session({
      id: "s-2",
      scopes: ["read", "write"],
      expires_at: "2026-10-09T00:00:00Z",
    });
    expect(heldScopes([grant()], [session(), later], "u-1", "srv-1", NOW)).toEqual([
      "read",
      "write",
    ]);
  });

  test("ignores expired and revoked sessions, other people and other servers", () => {
    const sessions = [
      session({ expires_at: "2026-09-30T00:00:00Z" }),
      session({ id: "s-2", revoked_at: "2026-09-02T00:00:00Z" }),
    ];
    expect(heldScopes([grant()], sessions, "u-1", "srv-1", NOW)).toBeUndefined();
    expect(
      heldScopes([grant({ user_id: "u-2" })], [session()], "u-1", "srv-1", NOW),
    ).toBeUndefined();
    expect(heldScopes([grant()], [session()], "u-1", "srv-2", NOW)).toBeUndefined();
  });

  test("is unknown until both listings and the account are known", () => {
    expect(heldScopes(undefined, [session()], "u-1", "srv-1", NOW)).toBeUndefined();
    expect(heldScopes([grant()], undefined, "u-1", "srv-1", NOW)).toBeUndefined();
    expect(heldScopes([grant()], [session()], undefined, "srv-1", NOW)).toBeUndefined();
  });
});

describe("missingScopes", () => {
  const server = { required_scopes: ["read", "write", "admin"] };

  test("lists the required scopes the session does not hold, in the server's order", () => {
    expect(missingScopes(server, ["read"])).toEqual(["write", "admin"]);
  });

  test("is empty when the session covers them, or when what it holds is unknown", () => {
    expect(missingScopes(server, ["read", "write", "admin", "extra"])).toEqual([]);
    expect(missingScopes(server, undefined)).toEqual([]);
  });
});

describe("parseScopes", () => {
  test("splits on spaces and commas and drops repeats", () => {
    expect(parseScopes(" repo, read:org  repo\n")).toEqual(["repo", "read:org"]);
    expect(parseScopes("")).toEqual([]);
  });
});
