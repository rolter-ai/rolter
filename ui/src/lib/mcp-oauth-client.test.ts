import { describe, expect, it } from "bun:test";

import type { McpServerRow } from "@/lib/api";
import {
  oauthChanged,
  oauthConnectable,
  oauthDraft,
  oauthProblem,
  oauthResetsDiscovery,
  oauthTouched,
  oauthValid,
  toOAuthInput,
  urlResetsDiscovery,
} from "@/lib/mcp-oauth-client";

const server = (over: Partial<McpServerRow> = {}): McpServerRow => ({
  id: "srv-1",
  org_id: "org-1",
  name: "Linear",
  slug: "linear",
  url: "https://mcp.linear.app/mcp",
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
  auth_kind: "oauth",
  auth_header_name: null,
  has_credential: false,
  connect_timeout_ms: null,
  request_timeout_ms: null,
  max_retries: null,
  oauth_issuer: null,
  oauth_discovery: "auto",
  oauth_discovered_issuer: null,
  oauth_discovered_authorize_url: null,
  oauth_discovered_token_url: null,
  oauth_discovered_iss_supported: false,
  oauth_discovered_at: null,
  ...over,
});

const MANUAL = server({
  client_id: "rolter",
  oauth_discovery: "manual",
  authorize_url: "https://auth.example.com/authorize",
  token_url: "https://auth.example.com/token",
  oauth_issuer: "https://auth.example.com",
});

describe("oauthProblem", () => {
  it("accepts a client id alone under auto discovery", () => {
    expect(oauthProblem({ ...oauthDraft(null), clientId: "rolter" })).toBeNull();
  });

  it("requires the endpoint pair under manual discovery", () => {
    expect(oauthProblem({ ...oauthDraft(null), clientId: "rolter", discovery: "manual" })).toBe(
      "manualNeedsEndpoints",
    );
    expect(oauthProblem(oauthDraft(MANUAL))).toBeNull();
  });

  it("refuses half a pair in either mode", () => {
    for (const discovery of ["auto", "manual"] as const) {
      expect(
        oauthProblem({
          ...oauthDraft(null),
          clientId: "rolter",
          discovery,
          tokenUrl: "https://a.example/token",
        }),
      ).toBe("pair");
    }
  });

  it("refuses a plaintext issuer or endpoint off loopback", () => {
    expect(
      oauthProblem({ ...oauthDraft(null), clientId: "rolter", issuer: "http://auth.example.com" }),
    ).toBe("endpoint");
    expect(
      oauthProblem({ ...oauthDraft(null), clientId: "rolter", issuer: "http://localhost:9000" }),
    ).toBeNull();
  });

  it("needs a client id", () => {
    expect(oauthProblem({ ...oauthDraft(null), issuer: "https://auth.example.com" })).toBe(
      "clientId",
    );
  });
});

describe("oauthValid", () => {
  it("lets an untouched section through, since nothing is sent", () => {
    expect(oauthValid(oauthDraft(null), null)).toBe(true);
  });

  it("counts a mode switch as touching the section", () => {
    const draft = { ...oauthDraft(null), discovery: "manual" as const };
    expect(oauthTouched(draft, null)).toBe(true);
    expect(oauthValid(draft, null)).toBe(false);
  });
});

describe("toOAuthInput", () => {
  it("always sends the discovery mode, so a manual row stays manual", () => {
    expect(toOAuthInput(oauthDraft(MANUAL))).toEqual({
      client_id: "rolter",
      discovery: "manual",
      issuer: "https://auth.example.com",
      authorize_url: "https://auth.example.com/authorize",
      token_url: "https://auth.example.com/token",
      default_scopes: [],
    });
  });

  it("leaves blank endpoints out and unpins a cleared issuer", () => {
    const input = toOAuthInput({ ...oauthDraft(null), clientId: "rolter" });
    expect(input).toEqual({
      client_id: "rolter",
      discovery: "auto",
      issuer: null,
      default_scopes: [],
    });
    expect("authorize_url" in input).toBe(false);
  });

  it("never clears a secret nobody was rotating", () => {
    expect("client_secret" in toOAuthInput(oauthDraft(MANUAL))).toBe(false);
    expect(toOAuthInput({ ...oauthDraft(MANUAL), clearSecret: true }).client_secret).toBe("");
  });
});

describe("oauthChanged", () => {
  it("is quiet for a re-save of what is stored", () => {
    expect(oauthChanged(oauthDraft(MANUAL), MANUAL)).toBe(false);
  });

  it("notices the mode and the issuer", () => {
    expect(oauthChanged({ ...oauthDraft(MANUAL), discovery: "auto" }, MANUAL)).toBe(true);
    expect(oauthChanged({ ...oauthDraft(MANUAL), issuer: "" }, MANUAL)).toBe(true);
  });
});

describe("oauthResetsDiscovery", () => {
  const found = server({
    client_id: "rolter",
    oauth_discovered_at: "2026-09-01T10:00:00Z",
    oauth_discovered_issuer: "https://auth.example.com",
  });

  it("warns only when there is a cache and the issuer or mode moves", () => {
    expect(oauthResetsDiscovery(oauthDraft(found), found, found.url)).toBe(false);
    expect(
      oauthResetsDiscovery(
        { ...oauthDraft(found), issuer: "https://other.example.com" },
        found,
        found.url,
      ),
    ).toBe(true);
    expect(
      oauthResetsDiscovery({ ...oauthDraft(found), clientId: "renamed" }, found, found.url),
    ).toBe(false);
    expect(
      oauthResetsDiscovery(
        { ...oauthDraft(null), issuer: "https://x.example.com" },
        server(),
        found.url,
      ),
    ).toBe(false);
  });

  it("warns when the url moves, as the store clears the cache for that too", () => {
    expect(oauthResetsDiscovery(oauthDraft(found), found, "https://mcp.example.com/mcp")).toBe(
      true,
    );
    expect(
      oauthResetsDiscovery(oauthDraft(server()), server(), "https://mcp.example.com/mcp"),
    ).toBe(false);
  });
});

describe("urlResetsDiscovery", () => {
  const found = server({ client_id: "rolter", oauth_discovered_at: "2026-09-01T10:00:00Z" });

  it("warns only for a server with a cache whose url moved", () => {
    expect(urlResetsDiscovery(found.url, found)).toBe(false);
    expect(urlResetsDiscovery("https://mcp.example.com/mcp", found)).toBe(true);
    expect(urlResetsDiscovery("https://mcp.example.com/mcp", server())).toBe(false);
    expect(urlResetsDiscovery("https://mcp.example.com/mcp", null)).toBe(false);
  });

  it("compares exactly, since the store does and the dialog does not trim", () => {
    expect(urlResetsDiscovery(`${found.url} `, found)).toBe(true);
  });
});

describe("oauthConnectable", () => {
  it("lets a discovering server connect with a client id alone", () => {
    expect(oauthConnectable(server({ client_id: "rolter" }))).toBe(true);
  });

  it("holds a manual server to its configured pair", () => {
    expect(oauthConnectable({ ...MANUAL, token_url: null })).toBe(false);
    expect(oauthConnectable(MANUAL)).toBe(true);
  });

  it("refuses without a client id", () => {
    expect(oauthConnectable(server())).toBe(false);
  });
});
