import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as React from "react";
import { MemoryRouter, useInRouterContext } from "react-router";

import { json } from "./story-harness";
import { Toaster } from "@/components/ui/toaster";
import { ToastProvider } from "@/lib/toast";
import type {
  McpGatewaySettingsRow,
  McpLibraryItem,
  McpServerRow,
  McpToolGroupRow,
} from "@/lib/api";

// the fixtures and fetch stub the MCP screens' stories share: the catalog, the
// library, tool groups and settings read one org's registry
export const ORG = { id: "org-1", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" };
export const TEAM = { id: "team-1", org_id: ORG.id, name: "Platform", created_at: ORG.created_at };
export const PROJECT = {
  id: "project-1",
  team_id: TEAM.id,
  name: "Gateway",
  team_name: TEAM.name,
  created_at: ORG.created_at,
};
// a virtual key of PROJECT, as the per-project key listing returns it
export const KEY = {
  id: "key-9",
  project_id: PROJECT.id,
  key_prefix: "sk-rl-ab",
  name: "ci runner",
  models: [],
  providers: [],
  disabled: false,
  created_by: null,
  business_unit_id: null,
  customer_id: null,
  created_at: ORG.created_at,
};
// what every server row carries since #952: no credential, no overrides
export const NO_AUTH = {
  auth_kind: "none",
  auth_header_name: null,
  has_credential: false,
  connect_timeout_ms: null,
  request_timeout_ms: null,
  max_retries: null,
} as const;
// and nothing discovered yet (#1347): a server that nobody has connected to
export const UNDISCOVERED = {
  oauth_issuer: null,
  oauth_discovery: "auto",
  oauth_discovered_issuer: null,
  oauth_discovered_authorize_url: null,
  oauth_discovered_token_url: null,
  oauth_discovered_iss_supported: false,
  oauth_discovered_at: null,
} as const;
// tools/list discovery has not answered (#1283): a never-attempted row, the
// state every server is in before the background discovery after an install
export const NOT_PROBED = {
  tools_discovery_status: "pending",
  tools_discovery_error: null,
  tools_discovered_at: null,
  tools_discovery_attempted_at: null,
  tools_stale: [] as string[],
} as const;
// the server answered tools/list; what a settled row looks like
export const DISCOVERED_OK = {
  tools_discovery_status: "discovered",
  tools_discovery_error: null,
  tools_discovered_at: "2026-09-12T08:30:00Z",
  tools_discovery_attempted_at: "2026-09-12T08:30:00Z",
  tools_stale: [] as string[],
} as const;
// no handshake and no failure ever recorded (#951): a server nobody has tested
export const NEVER_CONTACTED = {
  last_contact_at: null,
  last_protocol_version: null,
  last_server_name: null,
  last_error_kind: null,
  last_error_at: null,
} as const;
export const SERVERS: McpServerRow[] = [
  {
    id: "server-github",
    org_id: ORG.id,
    name: "GitHub",
    slug: "github",
    url: "https://api.githubcopilot.com/mcp/",
    transport: "streamable_http",
    description: "Repository and pull request operations.",
    enabled: true,
    tools: ["search_code", "create_issue", "get_pull_request"],
    source: "library",
    required_scopes: ["repo"],
    created_at: ORG.created_at,
    authorize_url: null,
    token_url: null,
    client_id: null,
    default_scopes: [],
    has_client_secret: false,
    ...NO_AUTH,
    ...UNDISCOVERED,
    ...DISCOVERED_OK,
    ...NEVER_CONTACTED,
  },
  {
    id: "server-sentry",
    org_id: ORG.id,
    name: "Sentry",
    slug: "sentry",
    url: "https://mcp.sentry.dev/mcp",
    transport: "streamable_http",
    description: "Production issue investigation.",
    enabled: false,
    tools: ["list_issues", "get_issue"],
    source: "custom",
    required_scopes: ["org:read"],
    created_at: ORG.created_at,
    authorize_url: null,
    token_url: null,
    client_id: null,
    default_scopes: [],
    has_client_secret: false,
    ...NO_AUTH,
    ...UNDISCOVERED,
    ...DISCOVERED_OK,
    ...NEVER_CONTACTED,
  },
];
// a server whose OAuth client is already registered: Connect needs a client id,
// plus the endpoint pair only when discovery is off (#1415)
export const CONNECTABLE: McpServerRow = {
  ...SERVERS[0],
  id: "server-linear",
  name: "Linear",
  slug: "linear",
  url: "https://mcp.linear.app/mcp",
  enabled: true,
  auth_kind: "oauth",
  authorize_url: "https://linear.app/oauth/authorize",
  token_url: "https://api.linear.app/oauth/token",
  client_id: "rolter-linear",
  default_scopes: ["read", "write"],
  has_client_secret: true,
};
// one fixture per auth kind (#1447). the credential never appears on a row,
// only `has_credential`, so none of these could leak one into a story either
export const BEARER: McpServerRow = {
  ...SERVERS[1],
  id: "server-context7",
  name: "Context7",
  slug: "context7",
  url: "https://mcp.context7.com/mcp",
  enabled: true,
  auth_kind: "bearer",
  has_credential: true,
  request_timeout_ms: 120000,
};
export const HEADER: McpServerRow = {
  ...SERVERS[1],
  id: "server-exa",
  name: "Exa",
  slug: "exa",
  url: "https://mcp.exa.ai/mcp",
  enabled: true,
  auth_kind: "header",
  auth_header_name: "X-Api-Key",
  has_credential: true,
  connect_timeout_ms: 2000,
  max_retries: 2,
};
export const OAUTH: McpServerRow = { ...CONNECTABLE, auth_kind: "oauth" };
export const BY_KIND = [SERVERS[1], BEARER, HEADER, OAUTH];
export const KEK_REFUSAL =
  "storing an MCP credential requires the ROLTER_KEK environment variable on the control plane to seal it at rest";
export const OAUTH_CLIENT = {
  server_id: "server-github",
  authorize_url: null,
  token_url: null,
  client_id: null,
  default_scopes: [],
  has_client_secret: false,
  redirect_uri: "https://control.example.com/auth/mcp/callback",
};
export const AUTHORIZE_STARTED = {
  authorization_url: "https://linear.app/oauth/authorize?client_id=rolter-linear&state=abc",
  state: "abc",
  expires_in: 600,
};
export const LIBRARY: McpLibraryItem[] = [
  {
    slug: "github",
    name: "GitHub",
    description: "Repository, issue, pull request, and code search tools.",
    url: "https://api.githubcopilot.com/mcp/",
    transport: "streamable_http",
    tools: ["search_code", "create_issue"],
    required_scopes: ["repo"],
    installed: true,
  },
  {
    slug: "notion",
    name: "Notion",
    description: "Search and update pages in an organization workspace.",
    url: "https://mcp.notion.com/mcp",
    transport: "streamable_http",
    tools: ["search", "notion-create-pages"],
    required_scopes: [],
    installed: false,
  },
  {
    slug: "linear",
    name: "Linear",
    description: "Browse and manage teams, issues, and projects.",
    url: "https://mcp.linear.app/mcp",
    transport: "streamable_http",
    tools: ["list_issues", "create_issue"],
    required_scopes: ["read", "write"],
    installed: false,
  },
];
export const GROUPS: McpToolGroupRow[] = [
  {
    id: "group-1",
    org_id: ORG.id,
    name: "Triage",
    slug: "triage",
    description: "Read-only incident and code investigation tools.",
    enabled: true,
    tools: [
      { server_id: "server-github", tool: "search_code" },
      { server_id: "server-sentry", tool: "list_issues" },
    ],
    created_at: ORG.created_at,
    updated_at: ORG.created_at,
  },
];
export const SETTINGS: McpGatewaySettingsRow = {
  org_id: ORG.id,
  default_transport: "streamable_http",
  connect_timeout_ms: 5000,
  request_timeout_ms: 30000,
  max_retries: 1,
  default_failure_mode: "fail_closed",
  allow_unlisted_tools: false,
  updated_at: ORG.created_at,
};

export type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
export const urlOf = (input: RequestInfo | URL) =>
  typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;

// both oauth routes sit *under* /mcp-servers/{id}, so they are matched before
// the listing: a substring match would answer a client read with the server
// array and leave the section rendering an undefined redirect uri
export function routed(
  over: Partial<
    Record<
      | "servers"
      | "library"
      | "groups"
      | "assignments"
      | "keys"
      | "settings"
      | "oauthClient"
      | "authorize"
      | "auth"
      | "grants"
      | "sessions",
      FetchStub
    >
  > = {},
): FetchStub {
  return async (input, init) => {
    const url = urlOf(input);
    // the catalog reads both to mark the servers the caller is connected to
    // (#2166); nobody has consented to anything unless a story says so
    if (url.includes("/mcp/grants")) return over.grants?.(input, init) ?? json([]);
    if (url.includes("/mcp/sessions")) return over.sessions?.(input, init) ?? json([]);
    if (url.includes("/mcp/library")) return over.library?.(input, init) ?? json(LIBRARY);
    if (url.includes("/mcp/tool-group-assignments") || url.includes("/assignments"))
      return over.assignments?.(input, init) ?? json([]);
    if (url.includes("/mcp/tool-groups")) return over.groups?.(input, init) ?? json(GROUPS);
    if (url.includes("/mcp/settings")) return over.settings?.(input, init) ?? json(SETTINGS);
    if (url.includes("/oauth-client")) return over.oauthClient?.(input, init) ?? json(OAUTH_CLIENT);
    if (url.includes("/oauth/authorize"))
      return over.authorize?.(input, init) ?? json(AUTHORIZE_STARTED);
    // `/auth` is a suffix of the server path, and `/oauth/authorize` above
    // contains it, so it is matched on the end of the url only
    if (url.endsWith("/auth"))
      return (
        over.auth?.(input, init) ??
        json({ ...SERVERS[0], ...JSON.parse(String(init?.body ?? "{}")) })
      );
    if (url.includes("/mcp-servers")) return over.servers?.(input, init) ?? json(SERVERS);
    if (url.includes("/virtual-keys")) return over.keys?.(input, init) ?? json([KEY]);
    if (url.includes("/projects")) return json([PROJECT]);
    if (url.includes("/teams")) return json([TEAM]);
    if (url.includes("/orgs")) return json([ORG]);
    return json([]);
  };
}

export function Harness({
  fetchStub,
  children,
}: {
  fetchStub: FetchStub;
  children: React.ReactNode;
}) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => {
    original.current ??= globalThis.fetch;
    globalThis.fetch = fetchStub as typeof globalThis.fetch;
    return new QueryClient({ defaultOptions: { queries: { retry: false } } });
  }, [fetchStub]);
  React.useEffect(
    () => () => {
      if (original.current) globalThis.fetch = original.current;
    },
    [],
  );
  // the library links to the catalog; a story that brings its own router keeps it
  const inRouter = useInRouterContext();
  return (
    <QueryClientProvider client={client}>
      {inRouter ? children : <MemoryRouter>{children}</MemoryRouter>}
    </QueryClientProvider>
  );
}

// the toast region is what outlives a closed sheet or dialog, so the stories
// that assert an outcome mount it
export function Toasts({ children }: { children: React.ReactNode }) {
  return (
    <ToastProvider>
      {children}
      <Toaster />
    </ToastProvider>
  );
}
