import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { McpCatalog, McpLibrary, McpSettings, ToolGroups } from "./McpManagement";
import { cancelConfirmation, confirmDestructive, recording } from "./story-harness";
import { Toaster } from "@/components/ui/toaster";
import type { McpGatewaySettingsRow, McpLibraryItem, McpServerRow, McpToolGroupRow } from "@/lib/api";
import { ToastProvider } from "@/lib/toast";

const ORG = { id: "org-1", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" };
const TEAM = { id: "team-1", org_id: ORG.id, name: "Platform", created_at: ORG.created_at };
const PROJECT = { id: "project-1", team_id: TEAM.id, name: "Gateway", created_at: ORG.created_at };
const SERVERS: McpServerRow[] = [
  { id: "server-github", org_id: ORG.id, name: "GitHub", slug: "github", url: "https://api.githubcopilot.com/mcp/", transport: "streamable_http", description: "Repository and pull request operations.", enabled: true, tools: ["search_code", "create_issue", "get_pull_request"], source: "library", required_scopes: ["repo"], created_at: ORG.created_at, authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false },
  { id: "server-sentry", org_id: ORG.id, name: "Sentry", slug: "sentry", url: "https://mcp.sentry.dev/mcp", transport: "streamable_http", description: "Production issue investigation.", enabled: false, tools: ["list_issues", "get_issue"], source: "custom", required_scopes: ["org:read"], created_at: ORG.created_at, authorize_url: null, token_url: null, client_id: null, default_scopes: [], has_client_secret: false },
];
// a server whose OAuth client is already registered: Connect is only offered
// once all three of authorize url, token url and client id are on the row
const CONNECTABLE: McpServerRow = { ...SERVERS[0], id: "server-linear", name: "Linear", slug: "linear", url: "https://mcp.linear.app/mcp", enabled: true, authorize_url: "https://linear.app/oauth/authorize", token_url: "https://api.linear.app/oauth/token", client_id: "rolter-linear", default_scopes: ["read", "write"], has_client_secret: true };
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
function routed(over: Partial<Record<"servers" | "library" | "groups" | "settings" | "oauthClient" | "authorize", FetchStub>> = {}): FetchStub {
  return async (input, init) => {
    const url = urlOf(input);
    if (url.includes("/mcp/library")) return over.library?.(input, init) ?? json(LIBRARY);
    if (url.includes("/mcp/tool-groups")) return over.groups?.(input, init) ?? json(GROUPS);
    if (url.includes("/mcp/settings")) return over.settings?.(input, init) ?? json(SETTINGS);
    if (url.includes("/oauth-client")) return over.oauthClient?.(input, init) ?? json(OAUTH_CLIENT);
    if (url.includes("/oauth/authorize")) return over.authorize?.(input, init) ?? json(AUTHORIZE_STARTED);
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
export const CatalogLoading: Story = { render: () => <Harness fetchStub={routed({ servers: () => new Promise<Response>(() => {}) })}><McpCatalog /></Harness> };
export const CatalogEmpty: Story = { render: () => <Harness fetchStub={routed({ servers: async () => json([]) })}><McpCatalog /></Harness> };
export const CatalogForbidden: Story = { render: () => <Harness fetchStub={routed({ servers: async () => json({ error: { message: "forbidden" } }, 403) })}><McpCatalog /></Harness> };
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
