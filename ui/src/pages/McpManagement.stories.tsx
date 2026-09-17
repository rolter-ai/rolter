import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { McpCatalog, McpLibrary, McpSettings, ToolGroups } from "./McpManagement";
import { cancelConfirmation, confirmDestructive, expectEmptyState, expectLoadError, expectRefused, expectSkeleton, Harness as GatedHarness, recording } from "./story-harness";
import { Toaster } from "@/components/ui/toaster";
import type { McpGatewaySettingsRow, McpLibraryItem, McpServerRow, McpToolGroupRow } from "@/lib/api";
import { ToastProvider } from "@/lib/toast";

const ORG = { id: "org-1", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" };
const TEAM = { id: "team-1", org_id: ORG.id, name: "Platform", created_at: ORG.created_at };
const PROJECT = { id: "project-1", team_id: TEAM.id, name: "Gateway", created_at: ORG.created_at };
// what every server row carries since #952: no credential, no overrides
const NO_AUTH = { auth_kind: "none", auth_header_name: null, has_credential: false, connect_timeout_ms: null, request_timeout_ms: null, max_retries: null } as const;
const SERVERS: McpServerRow[] = [
  { id: "server-github", org_id: ORG.id, name: "GitHub", slug: "github", url: "https://api.githubcopilot.com/mcp/", transport: "streamable_http", description: "Repository and pull request operations.", enabled: true, tools: ["search_code", "create_issue", "get_pull_request"], source: "library", required_scopes: ["repo"], created_at: ORG.created_at, authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false, ...NO_AUTH },
  { id: "server-sentry", org_id: ORG.id, name: "Sentry", slug: "sentry", url: "https://mcp.sentry.dev/mcp", transport: "streamable_http", description: "Production issue investigation.", enabled: false, tools: ["list_issues", "get_issue"], source: "custom", required_scopes: ["org:read"], created_at: ORG.created_at, authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false, ...NO_AUTH },
];
// a server whose OAuth client is already registered: Connect is only offered
// once all three of authorize url, token url and client id are on the row
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
    });
    // a client alone does not make the gateway use it: the kind has to say oauth
    await expect(await clientSaves.sent("PUT", "/mcp-servers/server-github/auth")).toEqual({ auth_kind: "oauth" });
  },
};

// three fields that travel together: two of them is not a client, and the
// endpoints must be https before the control plane will take them
export const CatalogRefusesAHalfClient: Story = {
  render: () => <Harness fetchStub={routed()}><McpCatalog /></Harness>,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("button", { name: "Configure server Sentry" }));
    const dialog = within(await within(document.body).findByRole("dialog"));
    await userEvent.click(dialog.getByRole("radio", { name: /^OAuth consent/ }));
    await userEvent.type(dialog.getByLabelText("Client ID"), "Iv1.abc123");
    await expect(dialog.getByRole("button", { name: "Save server" })).toBeDisabled();
    await userEvent.type(dialog.getByLabelText("Authorization URL"), "http://sentry.example.com/authorize");
    await expect(dialog.getAllByText("Must be an https URL (http is accepted only on loopback).").length).toBeGreaterThan(0);
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
export const ToolGroupsEmpty: Story = { render: () => <Harness fetchStub={routed({ groups: async () => json([]) })}><ToolGroups /></Harness> };
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
