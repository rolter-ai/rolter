import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { McpCatalog, McpLibrary, McpSettings, ToolGroups } from "./McpManagement";
import {
  cancelConfirmation,
  confirmDestructive,
  expectEmptyState,
  expectLoadError,
  expectRefused,
  expectSkeleton,
  Harness as GatedHarness,
  NEEDS_ADMIN,
  recording,
} from "./story-harness";
import { Toaster } from "@/components/ui/toaster";
import type { McpGatewaySettingsRow, McpLibraryItem, McpServerRow, McpToolGroupRow } from "@/lib/api";
import { ToastProvider } from "@/lib/toast";

const ORG = { id: "org-1", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" };
const TEAM = { id: "team-1", org_id: ORG.id, name: "Platform", created_at: ORG.created_at };
const PROJECT = { id: "project-1", team_id: TEAM.id, name: "Gateway", created_at: ORG.created_at };
// what every server row carries since #952: no credential, no overrides
const NO_AUTH = { auth_kind: "none", auth_header_name: null, has_credential: false, connect_timeout_ms: null, request_timeout_ms: null, max_retries: null } as const;
// and nothing discovered yet (#1347): a server that nobody has connected to
const UNDISCOVERED = { oauth_issuer: null, oauth_discovery: "auto", oauth_discovered_issuer: null, oauth_discovered_authorize_url: null, oauth_discovered_token_url: null, oauth_discovered_iss_supported: false, oauth_discovered_at: null } as const;
const SERVERS: McpServerRow[] = [
  { id: "server-github", org_id: ORG.id, name: "GitHub", slug: "github", url: "https://api.githubcopilot.com/mcp/", transport: "streamable_http", description: "Repository and pull request operations.", enabled: true, tools: ["search_code", "create_issue", "get_pull_request"], source: "library", required_scopes: ["repo"], created_at: ORG.created_at, authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false, ...NO_AUTH, ...UNDISCOVERED },
  { id: "server-sentry", org_id: ORG.id, name: "Sentry", slug: "sentry", url: "https://mcp.sentry.dev/mcp", transport: "streamable_http", description: "Production issue investigation.", enabled: false, tools: ["list_issues", "get_issue"], source: "custom", required_scopes: ["org:read"], created_at: ORG.created_at, authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false, ...NO_AUTH, ...UNDISCOVERED },
];
// a server whose OAuth client is already registered: Connect needs a client id,
// plus the endpoint pair only when discovery is off (#1415)
const CONNECTABLE: McpServerRow = { ...SERVERS[0], id: "server-linear", name: "Linear", slug: "linear", url: "https://mcp.linear.app/mcp", enabled: true, authorize_url: "https://linear.app/oauth/authorize", token_url: "https://api.linear.app/oauth/token", client_id: "rolter-linear", default_scopes: ["read", "write"], has_client_secret: true };
// one fixture per auth kind (#1447). the credential never appears on a row,
// only `has_credential`, so none of these could leak one into a story either
const BEARER: McpServerRow = { ...SERVERS[1], id: "server-context7", name: "Context7", slug: "context7", url: "https://mcp.context7.com/mcp", enabled: true, auth_kind: "bearer", has_credential: true, request_timeout_ms: 120000 };
const HEADER: McpServerRow = { ...SERVERS[1], id: "server-exa", name: "Exa", slug: "exa", url: "https://mcp.exa.ai/mcp", enabled: true, auth_kind: "header", auth_header_name: "X-Api-Key", has_credential: true, connect_timeout_ms: 2000, max_retries: 2 };
const OAUTH: McpServerRow = { ...CONNECTABLE, auth_kind: "oauth" };
const BY_KIND = [SERVERS[1], BEARER, HEADER, OAUTH];
const KEK_REFUSAL = "storing an MCP credential requires the ROLTER_KEK environment variable on the control plane to seal it at rest";
const OAUTH_CLIENT = { server_id: "server-github", authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false, redirect_uri: "https://control.example.com/auth/mcp/callback" };
const AUTHORIZE_STARTED = { authorization_url: "https://linear.app/oauth/authorize?client_id=rolter-linear&state=abc", state: "abc", expires_in: 600 };
const LIBRARY: McpLibraryItem[] = [
  { slug: "github", name: "GitHub", description: "Repository, issue, pull request, and code search tools.", url: "https://api.githubcopilot.com/mcp/", transport: "streamable_http", tools: ["search_code", "create_issue"], required_scopes: ["repo"], installed: true },
  { slug: "notion", name: "Notion", description: "Search and update pages in an organization workspace.", url: "https://mcp.notion.com/mcp", transport: "streamable_http", tools: ["search", "notion-create-pages"], required_scopes: [], installed: false },
  { slug: "linear", name: "Linear", description: "Browse and manage teams, issues, and projects.", url: "https://mcp.linear.app/mcp", transport: "streamable_http", tools: ["list_issues", "create_issue"], required_scopes: ["read", "write"], installed: false },
];
const GROUPS: McpToolGroupRow[] = [{ id: "group-1", org_id: ORG.id, name: "Triage", slug: "triage", description: "Read-only incident and code investigation tools.", enabled: true, tools: [{ server_id: "server-github", tool: "search_code" }, { server_id: "server-sentry", tool: "list_issues" }], created_at: ORG.created_at, updated_at: ORG.created_at }];
const SETTINGS: McpGatewaySettingsRow = { org_id: ORG.id, default_transport: "streamable_http", connect_timeout_ms: 5000, request_timeout_ms: 30000, max_retries: 1, default_failure_mode: "fail_closed", allow_unlisted_tools: false, updated_at: ORG.created_at };

type FetchStub = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const urlOf = (input: RequestInfo | URL) => typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;

// both oauth routes sit *under* /mcp-servers/{id}, so they are matched before
// the listing: a substring match would answer a client read with the server
// array and leave the section rendering an undefined redirect uri
function routed(over: Partial<Record<"servers" | "library" | "groups" | "settings" | "oauthClient" | "authorize" | "auth", FetchStub>> = {}): FetchStub {
  return async (input, init) => {
    const url = urlOf(input);
    if (url.includes("/mcp/library")) return over.library?.(input, init) ?? json(LIBRARY);
    if (url.includes("/mcp/tool-groups")) return over.groups?.(input, init) ?? json(GROUPS);
    if (url.includes("/mcp/settings")) return over.settings?.(input, init) ?? json(SETTINGS);
    if (url.includes("/oauth-client")) return over.oauthClient?.(input, init) ?? json(OAUTH_CLIENT);
    if (url.includes("/oauth/authorize")) return over.authorize?.(input, init) ?? json(AUTHORIZE_STARTED);
    // `/auth` is a suffix of the server path, and `/oauth/authorize` above
    // contains it, so it is matched on the end of the url only
    if (url.endsWith("/auth")) return over.auth?.(input, init) ?? json({ ...SERVERS[0], ...JSON.parse(String(init?.body ?? "{}")) });
    if (url.includes("/mcp-servers")) return over.servers?.(input, init) ?? json(SERVERS);
    if (url.includes("/projects")) return json([PROJECT]);
    if (url.includes("/teams")) return json([TEAM]);
    if (url.includes("/orgs")) return json([ORG]);
    return json([]);
  };
}

function Harness({ fetchStub, children }: { fetchStub: FetchStub; children: React.ReactNode }) {
  const original = React.useRef<typeof globalThis.fetch | null>(null);
  const client = React.useMemo(() => { original.current ??= globalThis.fetch; globalThis.fetch = fetchStub as typeof globalThis.fetch; return new QueryClient({ defaultOptions: { queries: { retry: false } } }); }, [fetchStub]);
  React.useEffect(() => () => { if (original.current) globalThis.fetch = original.current; }, []);
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

const meta = { title: "Screens/McpManagement", component: McpCatalog, parameters: { layout: "fullscreen" } } satisfies Meta<typeof McpCatalog>;
export default meta;
type Story = StoryObj<typeof meta>;

export const CatalogLoaded: Story = { render: () => <Harness fetchStub={routed()}><McpCatalog /></Harness>, play: async ({ canvasElement }) => { const canvas = within(canvasElement); await waitFor(() => expect(canvas.getByText("GitHub")).toBeVisible()); await expect(canvas.getByText("3 declared tools")).toBeVisible(); await expect(canvas.getByRole("switch", { name: "Enable Sentry" })).not.toBeChecked(); } };
export const CatalogLoading: Story = { render: () => <Harness fetchStub={routed({ servers: () => new Promise<Response>(() => {}) })}><McpCatalog /></Harness>, play: async ({ canvasElement }) => expectSkeleton(canvasElement) };
export const CatalogEmpty: Story = { render: () => <Harness fetchStub={routed({ servers: async () => json([]) })}><McpCatalog /></Harness>, play: async ({ canvasElement }) => expectEmptyState(canvasElement, /No MCP servers registered/, /Register server/) };
export const CatalogForbidden: Story = { render: () => <Harness fetchStub={routed({ servers: async () => json({ error: { message: "forbidden" } }, 403) })}><McpCatalog /></Harness>, play: async ({ canvasElement }) => expectLoadError(canvasElement, /You do not have access to/) };
export const CatalogValidatesEndpoint: Story = {
  render: () => <Harness fetchStub={routed()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Register server" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.type(dialog.getByLabelText("Name"), "Local tools");
    await userEvent.type(dialog.getByLabelText("Endpoint URL"), "ftp://invalid");
    await expect(dialog.getByRole("button", { name: "Register server" })).toBeDisabled();
  },
};
export const CatalogExplainsDeleteCascade: Story = { render: () => <Harness fetchStub={routed()}><McpCatalog /></Harness>, play: async ({ canvasElement }) => { const canvas = within(canvasElement); await userEvent.click(await canvas.findByRole("button", { name: "Delete server GitHub" })); const body = within(document.body); await expect(body.getByText(/removes every OAuth grant and token session/)).toBeVisible(); await expect(body.getByRole("button", { name: "Delete server" })).toBeEnabled(); } };

// the shared `recording` helper keeps method and url. the client save has to be
// asserted on its *body* too: a PUT that silently dropped the scopes, or that
// sent an empty `client_secret` and so cleared a secret nobody was rotating,
// passes a url-only check (#1194)
function bodyRecording(handler: FetchStub) {
  const calls: { method: string; url: string; body: unknown }[] = [];
  return {
    calls,
    stub: (async (input, init) => {
      calls.push({ method: (init?.method ?? "GET").toUpperCase(), url: urlOf(input), body: init?.body ? JSON.parse(String(init.body)) : undefined });
      return handler(input, init);
    }) as FetchStub,
    sent: async (method: string, fragment: string) => {
      let found: { method: string; url: string; body: unknown } | undefined;
      await waitFor(() => { found = calls.find((call) => call.method === method && call.url.includes(fragment)); expect(found).toBeDefined(); });
      return found?.body;
    },
  };
}

const clientSaves = bodyRecording(routed({ servers: async (_input, init) => init?.method === "PATCH" ? json(SERVERS[0]) : json(SERVERS) }));
export const CatalogRegistersOAuthClient: Story = {
  render: () => <Harness fetchStub={clientSaves.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server GitHub" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    // the client fields are the `oauth` branch of the auth picker now, not a
    // section every server shows (#1447)
    await expect(dialog.queryByLabelText("Client ID")).not.toBeInTheDocument();
    await userEvent.click(dialog.getByRole("radio", { name: /^OAuth consent/ }));
    // the redirect uri is deployment-derived, so it is read back rather than
    // guessed from the browser's origin
    await expect(dialog.getByLabelText("Redirect URI")).toHaveValue("https://control.example.com/auth/mcp/callback");
    await userEvent.type(dialog.getByLabelText("Authorization URL"), "https://github.com/login/oauth/authorize");
    await userEvent.type(dialog.getByLabelText("Token URL"), "https://github.com/login/oauth/access_token");
    await userEvent.type(dialog.getByLabelText("Client ID"), "Iv1.abc123");
    await userEvent.type(dialog.getByLabelText("Client secret"), "s3cr3t");
    await userEvent.type(dialog.getByLabelText("Default scopes"), "repo, read:org");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await clientSaves.sent("PUT", "/mcp-servers/server-github/oauth-client")).toEqual({
      authorize_url: "https://github.com/login/oauth/authorize",
      token_url: "https://github.com/login/oauth/access_token",
      client_id: "Iv1.abc123",
      default_scopes: ["repo", "read:org"],
      client_secret: "s3cr3t",
      // the mode and issuer always travel, since an omitted mode reads as auto
      discovery: "auto",
      issuer: null,
    });
    // a client alone does not make the gateway use it: the kind has to say oauth
    await expect(await clientSaves.sent("PUT", "/mcp-servers/server-github/auth")).toEqual({ auth_kind: "oauth" });
  },
};

// since #1347 a client id alone is a client — discovery supplies the rest — but
// the typed endpoints still travel as a pair, and must be https before the
// control plane will take them (#1415)
export const CatalogRefusesAHalfClient: Story = {
  render: () => <Harness fetchStub={routed()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server Sentry" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("radio", { name: /^OAuth consent/ }));
    await userEvent.type(dialog.getByLabelText("Client ID"), "Iv1.abc123");
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
    await userEvent.type(dialog.getByLabelText("Authorization URL"), "http://sentry.example.com/authorize");
    await expect(dialog.getAllByText("Must be an https URL (http is accepted only on loopback).").length).toBeGreaterThan(0);
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.clear(dialog.getByLabelText("Authorization URL"));
    await userEvent.type(dialog.getByLabelText("Authorization URL"), "https://sentry.io/oauth/authorize");
    await expect(dialog.getByRole("alert")).toHaveTextContent(/Enter both the authorization URL and the token URL/);
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
  },
};

const connects = bodyRecording(routed({ servers: async () => json([CONNECTABLE]) }));
export const CatalogConnectStartsConsent: Story = {
  render: () => <Harness fetchStub={connects.stub}><ToastProvider><McpCatalog /><Toaster /></ToastProvider></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // stubbed because a real new tab would take the test runner with it, and
    // the url the dashboard opens is the assertion that matters
    const opened: string[] = [];
    const original = window.open;
    window.open = ((url?: string | URL) => { opened.push(String(url)); return {} as Window; }) as typeof window.open;
    try {
      await userEvent.click(await canvas.findByRole("button", { name: "Start the consent flow for Linear" }));
      await connects.sent("POST", "/mcp-servers/server-linear/oauth/authorize");
      await waitFor(() => expect(opened).toEqual([AUTHORIZE_STARTED.authorization_url]));
      // the toast fades in, so it is momentarily transparent: waitFor rather
      // than a bare assertion, which would read opacity 0 on the first frame
      await waitFor(() => expect(canvas.getByText("Consent started for Linear")).toBeVisible());
      await expect(canvas.getByText(/appears on Auth Sessions/)).toBeVisible();
    } finally {
      window.open = original;
    }
  },
};

// there is nowhere to send the user until a client is registered, so the
// action is offered but refused rather than failing at the control plane
export const CatalogConnectNeedsAClient: Story = {
  render: () => <Harness fetchStub={routed()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: "Start the consent flow for GitHub" })).toBeDisabled();
  },
};

// ---------------------------------------------------------------------------
// authentication and transport overrides (#1447)

const byKind = () => routed({ servers: async (_input, init) => init?.method === "PATCH" ? json(SERVERS[1]) : json(BY_KIND) });
const openConfigure = async (canvasElement: HTMLElement, name: string) => {
  await userEvent.click(await within(canvasElement).findByRole("button", { name: `Configure server ${name}` }));
  return within(await within(document.body).findByRole("dialog"));
};

// the card says how each server authenticates, so an open server and an armed
// one no longer look the same from the catalog
export const AuthKindOnEveryCard: Story = {
  render: () => <Harness fetchStub={byKind()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Context7")).toBeVisible());
    for (const badge of ["no auth", "bearer", "api key", "oauth"]) await expect(canvas.getByText(badge)).toBeVisible();
  },
};

// `none` shows no credential field at all, and says it means unauthenticated
export const AuthNoneAsksForNothing: Story = {
  render: () => <Harness fetchStub={byKind()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Sentry");
    await expect(dialog.getByRole("radio", { name: /^None — unauthenticated/ })).toBeChecked();
    await expect(dialog.queryByLabelText("Bearer token")).not.toBeInTheDocument();
    await expect(dialog.queryByLabelText("Header name")).not.toBeInTheDocument();
    await expect(dialog.queryByLabelText("Client ID")).not.toBeInTheDocument();
  },
};

const bearerArms = bodyRecording(byKind());
export const AuthBearerNeedsACredential: Story = {
  render: () => <Harness fetchStub={bearerArms.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Sentry");
    await userEvent.click(dialog.getByRole("radio", { name: /^Bearer token/ }));
    await expect(dialog.getByText("No credential stored")).toBeVisible();
    // nothing stored and nothing typed is what the API refuses, so the form does first
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.type(dialog.getByLabelText("Bearer token"), "ctx7_live_abc");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await bearerArms.sent("PUT", "/mcp-servers/server-sentry/auth")).toEqual({ auth_kind: "bearer", credential: "ctx7_live_abc" });
    // untouched overrides are omitted, not echoed back as null
    const patch = (await bearerArms.sent("PATCH", "/mcp-servers/server-sentry")) as Record<string, unknown>;
    for (const key of ["connect_timeout_ms", "request_timeout_ms", "max_retries"]) await expect(patch).not.toHaveProperty(key);
  },
};

// renaming the header must not resend — or clear — a secret nobody can read back
const headerRename = bodyRecording(routed({ servers: async (_input, init) => init?.method === "PATCH" ? json(HEADER) : json(BY_KIND) }));
export const AuthHeaderRenameKeepsTheSecret: Story = {
  render: () => <Harness fetchStub={headerRename.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Exa");
    await expect(dialog.getByRole("radio", { name: /^API key header/ })).toBeChecked();
    await expect(dialog.getByText("Credential stored")).toBeVisible();
    await expect(dialog.getByLabelText("API key")).toHaveValue("");
    const header = dialog.getByLabelText("Header name");
    await userEvent.clear(header);
    await userEvent.type(header, "X-Exa-Token");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await headerRename.sent("PUT", "/mcp-servers/server-exa/auth")).toEqual({ auth_kind: "header", auth_header_name: "X-Exa-Token" });
  },
};

// the reserved list is checked in the form, with the reason, not left to a 400
export const AuthHeaderRefusesReservedNames: Story = {
  render: () => <Harness fetchStub={byKind()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Exa");
    const header = dialog.getByLabelText("Header name");
    await userEvent.clear(header);
    await userEvent.type(header, "Authorization");
    await expect(dialog.getByText(/Rolter sets Authorization itself/)).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.clear(header);
    await userEvent.type(header, "X Api Key");
    await expect(dialog.getByText(/valid HTTP header name/)).toBeVisible();
  },
};

// the oauth branch is the existing client section, shown for oauth alone
export const AuthOAuthShowsTheClient: Story = {
  render: () => <Harness fetchStub={byKind()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Linear");
    await expect(dialog.getByRole("radio", { name: /^OAuth consent/ })).toBeChecked();
    await expect(dialog.getByLabelText("Client ID")).toHaveValue("rolter-linear");
    await expect(dialog.queryByLabelText("Bearer token")).not.toBeInTheDocument();
  },
};

// confirm → cancel sends nothing; confirm → pending → done moves the server to
// `none`, which is the only shape the schema allows a server without a secret
let clearedOnce = false;
const clears = recording(routed({
  servers: async () => json(clearedOnce ? [SERVERS[1], { ...BEARER, auth_kind: "none", has_credential: false }] : BY_KIND),
  auth: async () => { await new Promise((resolve) => setTimeout(resolve, 400)); clearedOnce = true; return json({ ...BEARER, auth_kind: "none", has_credential: false }); },
}));
export const AuthClearCredentialConfirms: Story = {
  render: () => { clearedOnce = false; return <Harness fetchStub={clears.stub}><McpCatalog /></Harness>; },
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Context7");
    await userEvent.click(dialog.getByRole("button", { name: "Clear stored credential" }));
    const confirm = within(await within(document.body).findByRole("dialog"));
    await expect(confirm.getByText("Clear the stored credential for Context7?")).toBeVisible();
    await userEvent.click(confirm.getByRole("button", { name: "Cancel" }));
    clears.expectNotSent("PUT", "/auth");

    const again = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(again.getByRole("button", { name: "Clear stored credential" }));
    await confirmDestructive(/cannot be read back/, "Clear credential");
    await waitFor(() => expect(within(document.body).getByRole("button", { name: "Clear credential" })).toBeDisabled());
    await expect(await clears.expectSentBody("PUT", "/mcp-servers/server-context7/auth")).toEqual({ auth_kind: "none" });
    await waitFor(() => expect(within(document.body).queryByRole("button", { name: "Clear credential" })).not.toBeInTheDocument());
    const after = within(await within(document.body).findByRole("dialog"));
    await waitFor(() => expect(after.getByRole("radio", { name: /^None — unauthenticated/ })).toBeChecked());
    await expect(after.queryByText("Credential stored")).not.toBeInTheDocument();
  },
};

// switching a credentialed server to a kind that carries none deletes the
// secret on save, so the save asks first
const switches = bodyRecording(routed({ servers: async (_input, init) => init?.method === "PATCH" ? json(BEARER) : json(BY_KIND) }));
export const AuthSwitchingAwayConfirms: Story = {
  render: () => <Harness fetchStub={switches.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Context7");
    await userEvent.click(dialog.getByRole("radio", { name: /^None — unauthenticated/ }));
    await expect(dialog.getByText("Saving with this method deletes the stored credential.")).toBeVisible();
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await confirmDestructive(/Switching to None — unauthenticated deletes the stored credential/, "Clear credential");
    await expect(await switches.sent("PUT", "/mcp-servers/server-context7/auth")).toEqual({ auth_kind: "none" });
  },
};

// a missing KEK is a deployment problem, so it is named as one
export const AuthKekMissing: Story = {
  render: () => <Harness fetchStub={routed({ servers: async (_input, init) => init?.method === "PATCH" ? json(SERVERS[1]) : json(BY_KIND), auth: async () => json({ error: { message: KEK_REFUSAL } }, 400) })}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Sentry");
    await userEvent.click(dialog.getByRole("radio", { name: /^Bearer token/ }));
    await userEvent.type(dialog.getByLabelText("Bearer token"), "ctx7_live_abc");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await waitFor(() => expect(dialog.getByText("The control plane has no ROLTER_KEK")).toBeVisible());
    await expect(dialog.getByText(KEK_REFUSAL)).toBeVisible();
    // the dialog stays open with the typed value, so a retry needs no re-typing
    await expect(dialog.getByLabelText("Bearer token")).toHaveValue("ctx7_live_abc");
  },
};

// blank is inherit (null), a moved value is sent, an untouched one is omitted
const overrides = bodyRecording(routed({ servers: async (_input, init) => init?.method === "PATCH" ? json(HEADER) : json(BY_KIND) }));
export const OverridesKeepAbsentAndNullApart: Story = {
  render: () => <Harness fetchStub={overrides.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Exa");
    await expect(dialog.getByLabelText("Connect timeout (ms)")).toHaveValue("2000");
    await expect(dialog.getByLabelText("Request timeout (ms)")).toHaveAttribute("placeholder", "Inherit");
    const retries = dialog.getByLabelText("Maximum retries");
    await userEvent.clear(retries);
    await userEvent.type(retries, "9");
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.clear(retries);
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    const patch = (await overrides.sent("PATCH", "/mcp-servers/server-exa")) as Record<string, unknown>;
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).not.toBeInTheDocument());
    await expect(patch.max_retries).toBeNull();
    await expect(patch).not.toHaveProperty("connect_timeout_ms");
    await expect(patch).not.toHaveProperty("request_timeout_ms");
    // nothing about auth moved, so the KEK-gated route is not called at all
    await expect(overrides.calls.some((call) => call.url.endsWith("/auth"))).toBe(false);
  },
};

// a viewer reads the registry but can neither register nor configure a server
export const CatalogGatedForViewer: Story = {
  render: () => <GatedHarness role="viewer" fetchStub={byKind()}><McpCatalog /></GatedHarness>,
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Register server");
    await expectRefused(canvasElement, "Configure server Context7");
  },
};

// ---------------------------------------------------------------------------
// discovery, issuer and what discovery found (#1415)

// a client with nothing but an id, whose endpoints discovery already resolved
const DISCOVERED: McpServerRow = { ...OAUTH, id: "server-notion", name: "Notion", slug: "notion", url: "https://mcp.notion.com/mcp", authorize_url: null, token_url: null, client_id: "rolter-notion", has_client_secret: false, default_scopes: [], oauth_discovered_issuer: "https://api.notion.com", oauth_discovered_authorize_url: "https://api.notion.com/v1/oauth/authorize", oauth_discovered_token_url: "https://api.notion.com/v1/oauth/token", oauth_discovered_iss_supported: true, oauth_discovered_at: "2026-09-12T08:30:00Z" };
// the same, before anyone has connected: an id and no cache
const UNPROBED: McpServerRow = { ...DISCOVERED, id: "server-figma", name: "Figma", slug: "figma", url: "https://mcp.figma.com/mcp", client_id: "rolter-figma", ...UNDISCOVERED };
// a hand-configured server that publishes no metadata, with its issuer pinned
const MANUAL_OAUTH: McpServerRow = { ...OAUTH, id: "server-jira", name: "Jira", slug: "jira", url: "https://mcp.atlassian.com/v1/sse", authorize_url: "https://auth.atlassian.com/authorize", token_url: "https://auth.atlassian.com/oauth/token", client_id: "rolter-jira", oauth_discovery: "manual", oauth_issuer: "https://auth.atlassian.com" };
const DISCOVERY_ROWS = [DISCOVERED, UNPROBED, MANUAL_OAUTH];
// a PATCH answers with the row it was sent to, since the client PUT that
// follows is addressed by the id that comes back
const discoverySaves = () => bodyRecording(routed({ servers: async (input, init) => init?.method === "PATCH" ? json(DISCOVERY_ROWS.find((row) => urlOf(input).endsWith(`/${row.id}`)) ?? DISCOVERED) : json(DISCOVERY_ROWS) }));

// an operator can see what discovery resolved instead of guessing whether it
// worked, and a server that relies on it can connect with a client id alone
const shows = discoverySaves();
export const OAuthDiscoveryShowsWhatItFound: Story = {
  render: () => <Harness fetchStub={shows.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByRole("button", { name: "Start the consent flow for Notion" })).toBeEnabled();
    const dialog = await openConfigure(canvasElement, "Notion");
    await expect(dialog.getByRole("radio", { name: /^Discover automatically/ })).toBeChecked();
    const found = within(dialog.getByRole("group", { name: "Discovered endpoints" }));
    await expect(found.getByText("https://api.notion.com/v1/oauth/authorize")).toBeVisible();
    await expect(found.getByText("https://api.notion.com/v1/oauth/token")).toBeVisible();
    await expect(found.getByText("https://api.notion.com")).toBeVisible();
    await expect(found.getByText("Sends iss in callbacks")).toBeVisible();
    // the heading and the timestamp both start that way; the timestamp is the second
    await expect(found.getAllByText(/^Discovered /)).toHaveLength(2);
    // the typed pair is the fallback here, so blank is a valid answer
    await expect(dialog.getByRole("group", { name: "Fallback endpoints" })).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
    // nothing about the client moved, so the audited PUT is not sent
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await shows.sent("PATCH", "/mcp-servers/server-notion");
    await expect(shows.calls.some((call) => call.method === "PUT" && call.url.includes("/oauth-client"))).toBe(false);
  },
};

// a fresh server honestly says nothing has been found, and registering it
// needs only the client id — no endpoint keys go on the wire at all
const unprobed = discoverySaves();
export const OAuthDiscoveryNothingFoundYet: Story = {
  render: () => <Harness fetchStub={unprobed.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Figma");
    const found = within(dialog.getByRole("group", { name: "Discovered endpoints" }));
    await expect(found.getByText(/Nothing discovered yet/)).toBeVisible();
    await userEvent.type(dialog.getByLabelText("Default scopes"), "file:read");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await unprobed.sent("PUT", "/mcp-servers/server-figma/oauth-client")).toEqual({ client_id: "rolter-figma", discovery: "auto", issuer: null, default_scopes: ["file:read"] });
  },
};

// manual never probes, so the pair becomes required, the discovered panel goes
// away, and half a pair is refused with a message rather than a 400
const manual = discoverySaves();
export const OAuthManualRequiresEndpoints: Story = {
  render: () => <Harness fetchStub={manual.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Figma");
    await userEvent.click(dialog.getByRole("radio", { name: /^Configure manually/ }));
    await expect(dialog.queryByRole("group", { name: "Discovered endpoints" })).not.toBeInTheDocument();
    await expect(dialog.getByRole("group", { name: "Endpoints" })).toBeVisible();
    await expect(dialog.getByRole("alert")).toHaveTextContent(/Manual discovery needs an authorization URL and a token URL/);
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.type(dialog.getByLabelText("Authorization URL"), "https://www.figma.com/oauth");
    await expect(dialog.getByRole("alert")).toHaveTextContent(/Enter both the authorization URL and the token URL/);
    await userEvent.type(dialog.getByLabelText("Token URL"), "https://api.figma.com/v1/oauth/token");
    await userEvent.type(dialog.getByLabelText("Issuer"), "https://www.figma.com");
    await expect(dialog.queryByRole("alert")).not.toBeInTheDocument();
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await manual.sent("PUT", "/mcp-servers/server-figma/oauth-client")).toEqual({
      client_id: "rolter-figma",
      discovery: "manual",
      issuer: "https://www.figma.com",
      authorize_url: "https://www.figma.com/oauth",
      token_url: "https://api.figma.com/v1/oauth/token",
      default_scopes: [],
    });
  },
};

// re-saving a manual server used to omit the mode, which the control plane
// reads as auto. the mode and the pinned issuer now round-trip untouched
const resave = discoverySaves();
export const OAuthManualResaveKeepsTheMode: Story = {
  render: () => <Harness fetchStub={resave.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Jira");
    await expect(dialog.getByRole("radio", { name: /^Configure manually/ })).toBeChecked();
    await expect(dialog.getByLabelText("Issuer")).toHaveValue("https://auth.atlassian.com");
    await userEvent.type(dialog.getByLabelText("Default scopes"), "read:jira-work");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    const body = await resave.sent("PUT", "/mcp-servers/server-jira/oauth-client");
    await expect(body).toMatchObject({ discovery: "manual", issuer: "https://auth.atlassian.com", authorize_url: MANUAL_OAUTH.authorize_url, token_url: MANUAL_OAUTH.token_url });
  },
};

// re-pinning the issuer drops what discovery cached, so the form says so before
// the operator saves rather than leaving the panel empty afterwards unexplained
export const OAuthIssuerChangeResetsDiscovery: Story = {
  render: () => <Harness fetchStub={discoverySaves().stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Notion");
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await userEvent.type(dialog.getByLabelText("Issuer"), "http://api.notion.com");
    await expect(dialog.getByText("Must be an https URL (http is accepted only on loopback).")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.clear(dialog.getByLabelText("Issuer"));
    await userEvent.type(dialog.getByLabelText("Issuer"), "https://api.notion.com");
    await expect(dialog.getByText(/clears these results/)).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
  },
};

// the store drops the discovery cache when the url moves too (#1416), so the
// url field itself says so, and the discovered panel warns the same way it
// does for an issuer or mode change (#1572). moving the url back clears both
const moved = discoverySaves();
export const OAuthUrlChangeResetsDiscovery: Story = {
  render: () => <Harness fetchStub={moved.stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Notion");
    const url = dialog.getByLabelText("Endpoint URL");
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await expect(dialog.queryByText(/clears the OAuth endpoints discovered/)).not.toBeInTheDocument();
    await userEvent.clear(url);
    await userEvent.type(url, "https://mcp.notion.so/mcp");
    await expect(url).toHaveAccessibleDescription(/Saving a new URL clears the OAuth endpoints discovered for this server/);
    const found = within(dialog.getByRole("group", { name: "Discovered endpoints" }));
    await expect(found.getByText(/Saving a new server URL, issuer or discovery mode clears these results/)).toBeVisible();
    await userEvent.clear(url);
    await userEvent.type(url, DISCOVERED.url);
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await expect(url).not.toHaveAccessibleDescription(/clears the OAuth endpoints/);
    await userEvent.clear(url);
    await userEvent.type(url, "https://mcp.notion.so/mcp");
    await userEvent.click(dialog.getByRole("button", { name: "Save server" }));
    await expect(await moved.sent("PATCH", "/mcp-servers/server-notion")).toMatchObject({ url: "https://mcp.notion.so/mcp" });
  },
};

// a server nothing was ever discovered for loses nothing, so a new url is not
// worth a warning there
export const OAuthUrlChangeWithoutDiscoveryStaysQuiet: Story = {
  render: () => <Harness fetchStub={discoverySaves().stub}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Figma");
    const url = dialog.getByLabelText("Endpoint URL");
    await userEvent.clear(url);
    await userEvent.type(url, "https://mcp.figma.com/v2/mcp");
    await expect(dialog.queryByText(/clears these results/)).not.toBeInTheDocument();
    await expect(dialog.queryByText(/clears the OAuth endpoints discovered/)).not.toBeInTheDocument();
  },
};

// the client read is best-effort: refused, the section still edits off the
// server row and only the redirect uri, which nothing else can supply, is gone
export const OAuthClientReadRefused: Story = {
  render: () => <Harness fetchStub={routed({ servers: async () => json(DISCOVERY_ROWS), oauthClient: async () => json({ error: { message: "forbidden" } }, 403) })}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const dialog = await openConfigure(canvasElement, "Jira");
    await expect(dialog.getByLabelText("Client ID")).toHaveValue("rolter-jira");
    await expect(dialog.queryByLabelText("Redirect URI")).not.toBeInTheDocument();
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeEnabled();
  },
};

export const LibraryLoaded: Story = { render: () => <Harness fetchStub={routed()}><McpLibrary /></Harness> };
// a curated entry now arrives with the manifest the upstream server publishes
// (#1252), which is what an install stores, what the tool tally counts, and
// what a tool group can pick from
export const LibraryShowsCuratedTools: Story = {
  render: () => <Harness fetchStub={routed()}><McpLibrary /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Linear")).toBeVisible());
    await expect(canvas.getByText("search_code")).toBeVisible();
    await expect(canvas.getAllByText("create_issue")).toHaveLength(2);
    await expect(canvas.queryByText("No tools declared")).not.toBeInTheDocument();
    // Notion has tools but no scopes to request — its consent screen grants
    // selected pages — so that line stays honest instead of inventing one
    await expect(canvas.getByText("notion-create-pages")).toBeVisible();
    await expect(canvas.getByText("No OAuth scopes")).toBeVisible();
  },
};
// a control plane older than #1252 still answers with empty lists; every entry
// then claimed "No tools declared" — a statement about the catalog that was
// not true of the servers behind it (#1194)
export const LibraryHidesAnEmptyToolList: Story = {
  render: () => <Harness fetchStub={routed({ library: async () => json(LIBRARY.map((item) => ({ ...item, tools: [] }))) })}><McpLibrary /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Linear")).toBeVisible());
    await expect(canvas.queryByText("No tools declared")).not.toBeInTheDocument();
  },
};
// notion is filtered out so exactly one entry is uninstalled and "Install"
// names a single button
export const LibraryInstallsServer: Story = { render: () => { let installed = false; const stub = routed({ library: async () => json(LIBRARY.filter((item) => item.slug !== "notion").map((item) => item.slug === "linear" ? { ...item, installed } : item)), servers: async (_input, init) => { if (init?.method === "POST") { installed = true; return json({ ...SERVERS[0], id: "server-linear", name: "Linear", slug: "linear" }); } return json(SERVERS); } }); return <Harness fetchStub={stub}><McpLibrary /></Harness>; }, play: async ({ canvasElement }) => { const canvas = within(canvasElement); await userEvent.click(await canvas.findByRole("button", { name: "Install" })); await waitFor(() => expect(canvas.getAllByRole("button", { name: "Installed" })).toHaveLength(2)); } };

export const ToolGroupsLoaded: Story = { render: () => <Harness fetchStub={routed()}><ToolGroups /></Harness>, play: async ({ canvasElement }) => { const canvas = within(canvasElement); await waitFor(() => expect(canvas.getByText("Triage")).toBeVisible()); await expect(canvas.getByText("GitHub/search_code")).toBeVisible(); } };
export const ToolGroupsEmpty: Story = {
  render: () => <Harness fetchStub={routed({ groups: async () => json([]) })}><ToolGroups /></Harness>,
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No tool groups/, /Create group/);
  },
};
export const ToolGroupSelectsTools: Story = { render: () => <Harness fetchStub={routed({ groups: async () => json([]) })}><ToolGroups /></Harness>, play: async ({ canvasElement }) => { const canvas = within(canvasElement); const create = await canvas.findByRole("button", { name: "Create group" }); await waitFor(() => expect(create).toBeEnabled()); await userEvent.click(create); const body = within(document.body); await userEvent.type(body.getByLabelText("Name"), "Builders"); await userEvent.click(body.getByRole("button", { name: "create_issue" })); await expect(body.getByRole("button", { name: "create_issue" })).toHaveAttribute("aria-pressed", "true"); await expect(body.getByRole("button", { name: "Save group" })).toBeEnabled(); } };

// the group delete was the last window.confirm on this screen, and it could not
// even name the group it was about (#1179)
const groupDeletes = recording(routed({ groups: async (_input, init) => init?.method === "DELETE" ? json({}, 204) : json(GROUPS) }));
export const ToolGroupConfirmsBeforeDeleting: Story = {
  render: () => <Harness fetchStub={groupDeletes.stub}><ToolGroups /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("Triage")).toBeVisible());
    // by name, not by position: the row control names the group (#1214)
    const del = () => canvas.getByRole("button", { name: "Delete tool group Triage" });

    await userEvent.click(del());
    await cancelConfirmation();
    groupDeletes.expectNotSent("DELETE", "/mcp/tool-groups/group-1");

    await userEvent.click(del());
    await confirmDestructive(/Triage/, "Delete");
    await groupDeletes.expectSent("DELETE", "/mcp/tool-groups/group-1");
  },
};

export const SettingsLoaded: Story = { render: () => <Harness fetchStub={routed()}><McpSettings /></Harness> };
export const SettingsSavesChanges: Story = { render: () => { let saved = SETTINGS; const stub = routed({ settings: async (_input, init) => { if (init?.method === "PUT") saved = { ...SETTINGS, ...JSON.parse(String(init.body)), updated_at: "2026-08-02T00:00:00Z" }; return json(saved); } }); return <Harness fetchStub={stub}><McpSettings /></Harness>; }, play: async ({ canvasElement }) => { const canvas = within(canvasElement); const retries = await canvas.findByLabelText("Maximum retries"); await userEvent.clear(retries); await userEvent.type(retries, "3"); await userEvent.click(canvas.getByRole("button", { name: "Save MCP settings" })); await waitFor(() => expect(retries).toHaveValue(3)); } };

// The catalog's other two gates (#1606). `CatalogGatedForViewer` above covers
// the create and the configure; the delete and the enable toggle are separate
// controls with separate gates, and a `GatedSwitch` is not a `GatedButton`, so
// neither is reached by that story.
export const CatalogDeleteAndToggleGatedForViewer: Story = {
  render: () => <GatedHarness role="viewer" fetchStub={routed()}><McpCatalog /></GatedHarness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRefused(canvasElement, "Delete server GitHub");
    // the switch carries the same refusal, said through `aria-disabled`: a
    // disabled Radix switch still has to be announced as a switch
    const toggle = await canvas.findByRole("switch", { name: "Enable Sentry" });
    await waitFor(() => expect(toggle).toBeDisabled());
    await expect(toggle).toHaveAttribute("title", NEEDS_ADMIN);
  },
};

// a tool group is its own resource, so the catalog's gates say nothing about it
export const ToolGroupsGatedForMember: Story = {
  render: () => <GatedHarness role="member" fetchStub={routed()}><ToolGroups /></GatedHarness>,
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Delete tool group Triage");
    await expectRefused(canvasElement, "Configure tool group Triage");
  },
};
