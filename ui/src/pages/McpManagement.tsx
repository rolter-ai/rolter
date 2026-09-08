import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, KeyRound, Link2, Loader2, Plus, Puzzle, Server, Settings2, ShieldCheck, Wrench } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog as BaseDialog, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  createMcpServer,
  createMcpToolGroup,
  deleteMcpServer,
  deleteMcpToolGroup,
  fetchMcpLibrary,
  fetchMcpOAuthClient,
  fetchMcpServers,
  fetchMcpSettings,
  fetchMcpToolGroups,
  setMcpOAuthClient,
  startMcpOAuth,
  updateMcpServer,
  updateMcpSettings,
  updateMcpToolGroup,
  type McpGatewaySettingsRow,
  type McpLibraryItem,
  type McpOAuthClientInput,
  type McpServerInput,
  type McpServerRow,
  type McpToolGroupRow,
  type McpToolRef,
} from "@/lib/api";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const TRANSPORTS = ["streamable_http", "sse", "websocket"];
const slugify = (value: string) => value.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 63);
const lines = (value: string) => [...new Set(value.split(/[,\n]/).map((item) => item.trim()).filter(Boolean))];

function Dialog({ onClose, children }: { open?: boolean; onClose: () => void; children: React.ReactNode }) {
  return <BaseDialog open onOpenChange={(open) => !open && onClose()}>{children}</BaseDialog>;
}

function PageLead({ eyebrow, children, action }: { eyebrow: string; children: React.ReactNode; action?: React.ReactNode }) {
  return <div className="flex flex-col gap-4 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end"><div className="min-w-0 flex-1"><p className="font-mono text-[10px] uppercase tracking-[0.24em] text-[color:var(--red-folk-text)]">{eyebrow}</p><div className="mt-2 text-sm leading-relaxed text-muted-foreground">{children}</div></div>{action}</div>;
}

function ToolBadges({ tools }: { tools: string[] }) {
  if (!tools.length) return <span className="text-xs text-[color:var(--text-subtle)]">No tools declared</span>;
  return <div className="flex flex-wrap gap-1.5">{tools.map((tool) => <Badge key={tool} tone="outline">{tool}</Badge>)}</div>;
}

// ---------------------------------------------------------------------------
// the OAuth client rolter presents to one server's authorization server (#707).
// registering it is what makes the Connect action possible: without an
// authorize url, a token url and a client id there is nowhere to send the user

// the same rule mcp_oauth_flow.rs enforces on write — https, with http allowed
// only on loopback so a local stub stays usable. checked here as well so a typo
// disables the button instead of costing a round trip
const LOOPBACK = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/|$)/;
const oauthEndpoint = (value: string) => /^https:\/\//.test(value) || LOOPBACK.test(value);

interface OAuthDraft {
  authorizeUrl: string;
  tokenUrl: string;
  clientId: string;
  /** never pre-filled: the stored secret is sealed and is never read back */
  secret: string;
  clearSecret: boolean;
  scopes: string;
}

const oauthDraft = (server: McpServerRow | null): OAuthDraft => ({ authorizeUrl: server?.authorize_url ?? "", tokenUrl: server?.token_url ?? "", clientId: server?.client_id ?? "", secret: "", clearSecret: false, scopes: (server?.default_scopes ?? []).join(", ") });

// the three endpoint fields travel together: a client with two of them is not
// a client, so the section is either filled in or left alone entirely
const oauthTouched = (draft: OAuthDraft) => !!(draft.authorizeUrl.trim() || draft.tokenUrl.trim() || draft.clientId.trim());
const oauthComplete = (draft: OAuthDraft) => !!draft.authorizeUrl.trim() && !!draft.tokenUrl.trim() && !!draft.clientId.trim();
const oauthEndpointsValid = (draft: OAuthDraft) => (!draft.authorizeUrl.trim() || oauthEndpoint(draft.authorizeUrl.trim())) && (!draft.tokenUrl.trim() || oauthEndpoint(draft.tokenUrl.trim()));

// only send the PUT when something actually moved: the endpoint writes an audit
// entry on every call, and re-saving identical values would fill the log with
// changes nobody made
const oauthChanged = (draft: OAuthDraft, server: McpServerRow | null) =>
  draft.authorizeUrl.trim() !== (server?.authorize_url ?? "") ||
  draft.tokenUrl.trim() !== (server?.token_url ?? "") ||
  draft.clientId.trim() !== (server?.client_id ?? "") ||
  lines(draft.scopes).join(" ") !== (server?.default_scopes ?? []).join(" ") ||
  !!draft.secret ||
  draft.clearSecret;

// the secret is tri-state on the wire: omitted leaves it alone, "" clears it, a
// value rotates it. an empty input must never clear a secret the operator
// simply was not rotating
function toOAuthInput(draft: OAuthDraft): McpOAuthClientInput {
  const input: McpOAuthClientInput = { authorize_url: draft.authorizeUrl.trim(), token_url: draft.tokenUrl.trim(), client_id: draft.clientId.trim(), default_scopes: lines(draft.scopes) };
  if (draft.clearSecret) input.client_secret = "";
  else if (draft.secret) input.client_secret = draft.secret;
  return input;
}

function OAuthClientSection({ server, draft, onChange }: { server: McpServerRow | null; draft: OAuthDraft; onChange: (patch: Partial<OAuthDraft>) => void }) {
  const { t } = useTranslation();
  // read only for the redirect uri, which is deployment-derived and so cannot
  // be worked out from the browser's own origin. best-effort: the section still
  // works off the server row when the read is refused or the server is new
  const client = useQuery({ queryKey: ["mcp-oauth-client", server?.id], queryFn: () => fetchMcpOAuthClient(server?.id as string), enabled: !!server, retry: false });
  const stored = server?.has_client_secret ?? false;
  return <section className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-4">
    <div className="flex flex-wrap items-center gap-2"><KeyRound className="h-4 w-4 text-[color:var(--red-folk)]" aria-hidden /><h3 className="text-sm font-semibold">{t("pages.mcpCatalog.oauth.title")}</h3><Badge tone={stored && !draft.clearSecret ? "success" : "neutral"}>{stored && !draft.clearSecret ? t("pages.mcpCatalog.oauth.secretSet") : t("pages.mcpCatalog.oauth.secretMissing")}</Badge></div>
    <p className="mt-2 text-xs leading-relaxed text-muted-foreground">{t("pages.mcpCatalog.oauth.lead")}</p>
    <div className="mt-4 grid gap-3">
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("pages.mcpCatalog.oauth.authorizeUrl")} htmlFor="mcp-authorize-url" error={draft.authorizeUrl.trim() && !oauthEndpoint(draft.authorizeUrl.trim()) ? t("pages.mcpCatalog.oauth.endpointError") : undefined}><Input id="mcp-authorize-url" value={draft.authorizeUrl} onChange={(event) => onChange({ authorizeUrl: event.target.value })} /></Field>
        <Field label={t("pages.mcpCatalog.oauth.tokenUrl")} htmlFor="mcp-token-url" error={draft.tokenUrl.trim() && !oauthEndpoint(draft.tokenUrl.trim()) ? t("pages.mcpCatalog.oauth.endpointError") : undefined}><Input id="mcp-token-url" value={draft.tokenUrl} onChange={(event) => onChange({ tokenUrl: event.target.value })} /></Field>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("pages.mcpCatalog.oauth.clientId")} htmlFor="mcp-client-id"><Input id="mcp-client-id" value={draft.clientId} onChange={(event) => onChange({ clientId: event.target.value })} /></Field>
        <Field label={t("pages.mcpCatalog.oauth.clientSecret")} htmlFor="mcp-client-secret" hint={draft.clearSecret ? t("pages.mcpCatalog.oauth.clearingSecret") : t("pages.mcpCatalog.oauth.secretHint")}><Input id="mcp-client-secret" type="password" autoComplete="new-password" disabled={draft.clearSecret} placeholder={stored ? t("pages.mcpCatalog.oauth.secretPlaceholder") : undefined} value={draft.secret} onChange={(event) => onChange({ secret: event.target.value })} /></Field>
      </div>
      {stored && <div><Button type="button" variant={draft.clearSecret ? "default" : "outline"} aria-pressed={draft.clearSecret} onClick={() => onChange({ clearSecret: !draft.clearSecret, secret: "" })}>{t("pages.mcpCatalog.oauth.clearSecret")}</Button></div>}
      <Field label={t("pages.mcpCatalog.oauth.scopes")} htmlFor="mcp-default-scopes" hint={t("pages.mcpCatalog.oauth.scopesHint")}><Input id="mcp-default-scopes" value={draft.scopes} onChange={(event) => onChange({ scopes: event.target.value })} /></Field>
      {client.data && <Field label={t("pages.mcpCatalog.oauth.redirectUri")} htmlFor="mcp-redirect-uri" hint={t("pages.mcpCatalog.oauth.redirectHint")}><Input id="mcp-redirect-uri" readOnly value={client.data.redirect_uri} /></Field>}
      {oauthTouched(draft) && !oauthComplete(draft) && <p role="alert" className="text-xs text-[color:var(--danger-text)]">{t("pages.mcpCatalog.oauth.incomplete")}</p>}
    </div>
  </section>;
}

// starts consent and hands the browser the url the control plane minted. the
// dashboard never navigates itself there: the consent screen belongs to a third
// party, and the operator should come back to a page that kept its place
function ConnectButton({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const toast = useToast();
  const ready = !!(server.authorize_url && server.token_url && server.client_id);
  const connect = useMutation({
    mutationFn: () => startMcpOAuth(server.id),
    onSuccess: (started) => {
      const opened = window.open(started.authorization_url, "_blank", "noopener,noreferrer");
      // a blocked pop-up is silent otherwise: the request succeeded, a login
      // state row exists upstream, and nothing at all appeared on screen
      if (!opened) return void toast.push({ tone: "error", title: t("pages.mcpCatalog.connect.blocked"), detail: t("pages.mcpCatalog.connect.blockedDetail") });
      toast.push({ tone: "info", title: t("pages.mcpCatalog.connect.started", { name: server.name }), detail: t("pages.mcpCatalog.connect.startedDetail") });
    },
    onError: (error) => toast.push({ tone: "error", title: t("pages.mcpCatalog.connect.failed", { name: server.name }), detail: errorDetail(error) }),
  });
  return <Button variant="outline" disabled={!ready || connect.isPending} title={ready ? undefined : t("pages.mcpCatalog.connect.unconfigured")} aria-label={t("pages.mcpCatalog.connect.ready", { name: server.name })} onClick={() => connect.mutate()}>{connect.isPending ? <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden /> : <Link2 className="mr-2 h-4 w-4" aria-hidden />}{t("pages.mcpCatalog.connect.action")}</Button>;
}

function ConfirmDelete({ server, pending, error, onClose, onConfirm }: { server: McpServerRow; pending: boolean; error: Error | null; onClose: () => void; onConfirm: () => void }) {
  return <Dialog open onClose={onClose}><DialogHeader><DialogTitle>Delete {server.name}?</DialogTitle><DialogDescription>Deleting this server also removes every OAuth grant and token session attached to it. This cannot be undone.</DialogDescription></DialogHeader>{error && <p role="alert" className="text-sm text-[color:var(--danger-text)]">{error.message}</p>}<DialogFooter><Button variant="ghost" onClick={onClose}>Cancel</Button><Button variant="destructive" disabled={pending} onClick={onConfirm}>{pending ? "Deleting…" : "Delete server"}</Button></DialogFooter></Dialog>;
}

export function McpCatalog() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const query = useQuery({ queryKey: ["mcp-servers", orgId], queryFn: () => fetchMcpServers(orgId as string), enabled: !!orgId, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "mcp-servers");
  const [editing, setEditing] = React.useState<McpServerRow | null | undefined>(undefined);
  const [deleting, setDeleting] = React.useState<McpServerRow | null>(null);
  // the OAuth client lives behind its own endpoint, keyed by server id, so it
  // is written after the row — which is also how a client can be registered on
  // a server in the same breath as creating it
  const save = useMutation({
    mutationFn: async ({ initial, input, oauth }: { initial: McpServerRow | null; input: McpServerInput; oauth: McpOAuthClientInput | null }) => {
      const server = initial ? await updateMcpServer(initial.id, input) : await createMcpServer(orgId as string, input);
      if (oauth) await setMcpOAuthClient(server.id, oauth);
      return server;
    },
    onSuccess: () => { void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }); void client.invalidateQueries({ queryKey: ["mcp-oauth-client"] }); setEditing(undefined); },
  });
  const remove = useMutation({ mutationFn: deleteMcpServer, onSuccess: () => { void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }); setDeleting(null); } });
  const toggle = useMutation({ mutationFn: (server: McpServerRow) => updateMcpServer(server.id, { ...server, enabled: !server.enabled }), onSuccess: () => void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }) });
  if (!orgId) return <PageBody><EmptyState uxTarget="mcp-no-org" icon={<Server />} title="Choose an organization" description="MCP servers are isolated by organization." /></PageBody>;
  // only enabled servers reach the gateway snapshot, so the tool tally counts theirs
  const live = query.data?.filter((server) => server.enabled) ?? [];
  const liveTools = live.reduce((sum, server) => sum + server.tools.length, 0);
  return <PageBody>
    <PageLead eyebrow="live registry" action={<Button onClick={() => setEditing(null)}><Plus className="h-4 w-4" aria-hidden />Register server</Button>}>
      {query.data ? <><span>{live.length} enabled {live.length === 1 ? "server" : "servers"}</span>{" · "}<span>{liveTools} declared {liveTools === 1 ? "tool" : "tools"}</span></> : "Registered servers are projected into the gateway snapshot when enabled."}
    </PageLead>
    {query.isLoading ? <CardGridSkeleton cards={3} height={190} min={300} /> : query.error ? <LoadError error={query.error} resource={t("errors.resources.mcpServers")} onRetry={() => void query.refetch()} /> : !query.data?.length ? <EmptyState uxTarget="mcp-servers" icon={<Server />} title="No MCP servers registered" description="Register a custom endpoint or install a curated server from MCP Library." actions={<Button onClick={() => setEditing(null)}>Register server</Button>} /> : <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">{query.data.map((server) => <article key={server.id} className={`min-w-0 rounded-[10px] border border-[color:var(--border-default)] p-4 ${server.enabled ? "bg-card" : "bg-[color:var(--surface-subtle)]/60"}`}>
      <div className="flex items-start gap-3"><span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)]"><Server className="h-4 w-4" aria-hidden /></span><div className="min-w-0 flex-1"><div className="flex flex-wrap items-center gap-2"><h2 className="truncate font-mono text-sm font-semibold">{server.name}</h2><Badge tone={server.source === "library" ? "accent" : "neutral"}>{server.source}</Badge></div><p className="mt-1 truncate font-mono text-xs text-muted-foreground">{server.url}</p></div><GatedSwitch gate="mcp_server:update" checked={server.enabled} aria-label={t("pages.mcpCatalog.servers.toggleAria", { name: server.name })} onCheckedChange={() => toggle.mutate(server)} /></div>
      <p className="mt-3 min-h-10 text-xs leading-relaxed text-muted-foreground">{server.description || "No description provided."}</p><div className="mt-3"><ToolBadges tools={server.tools} /></div>
      <div className="mt-4 flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3"><Badge tone="info">{server.transport.replace("_", " ")}</Badge><span className="ml-auto flex flex-wrap justify-end gap-1"><ConnectButton server={server} /><GatedButton gate="mcp_server:delete" variant="ghost" aria-label={t("pages.mcpCatalog.servers.deleteAria", { name: server.name })} onClick={() => setDeleting(server)}>Delete</GatedButton><GatedButton gate="mcp_server:update" variant="outline" aria-label={t("pages.mcpCatalog.servers.configureAria", { name: server.name })} onClick={() => setEditing(server)}>Configure</GatedButton></span></div>
    </article>)}</div>}
    {editing !== undefined && <ServerDialog initial={editing} pending={save.isPending} error={save.error} onClose={() => setEditing(undefined)} onSave={(input, oauth) => save.mutate({ initial: editing, input, oauth })} />}
    {deleting && <ConfirmDelete server={deleting} pending={remove.isPending} error={remove.error} onClose={() => setDeleting(null)} onConfirm={() => remove.mutate(deleting.id)} />}
  </PageBody>;
}

function ServerDialog({ initial, pending, error, onClose, onSave }: { initial: McpServerRow | null; pending: boolean; error: Error | null; onClose: () => void; onSave: (input: McpServerInput, oauth: McpOAuthClientInput | null) => void }) {
  const [form, setForm] = React.useState<McpServerInput>(initial ?? { name: "", slug: "", url: "", transport: "streamable_http", description: "", enabled: true, tools: [], source: "custom", required_scopes: [] });
  const [tools, setTools] = React.useState(form.tools.join("\n"));
  const [scopes, setScopes] = React.useState(form.required_scopes.join("\n"));
  const [oauth, setOAuth] = React.useState<OAuthDraft>(() => oauthDraft(initial));
  const oauthValid = (!oauthTouched(oauth) || oauthComplete(oauth)) && oauthEndpointsValid(oauth);
  const valid = form.name.trim() && (initial || slugify(form.slug || form.name)) && /^https?:\/\//.test(form.url) && oauthValid;
  return <Dialog open onClose={onClose}><DialogHeader><DialogTitle>{initial ? "Configure MCP server" : "Register MCP server"}</DialogTitle><DialogDescription>Declare the endpoint and exact tool names exposed through the organization registry.</DialogDescription></DialogHeader><div className="grid gap-4 py-4">
    <div className="grid gap-3 sm:grid-cols-2"><Field label="Name" htmlFor="mcp-name"><Input id="mcp-name" value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} /></Field><Field label="Slug" htmlFor="mcp-slug" hint="Used in /mcp/{server}; immutable after registration."><Input id="mcp-slug" disabled={!!initial} value={initial?.slug ?? form.slug} onChange={(event) => setForm({ ...form, slug: event.target.value })} /></Field></div>
    <Field label="Endpoint URL" htmlFor="mcp-url"><Input id="mcp-url" value={form.url} onChange={(event) => setForm({ ...form, url: event.target.value })} /></Field>
    <div className="grid gap-3 sm:grid-cols-2"><Field label="Transport" htmlFor="mcp-transport"><Select id="mcp-transport" value={form.transport} onChange={(event) => setForm({ ...form, transport: event.target.value })}>{TRANSPORTS.map((transport) => <option key={transport}>{transport}</option>)}</Select></Field><div className="flex items-end"><label className="flex min-h-9 w-full items-center justify-between rounded-md border border-[color:var(--border-default)] px-3 text-sm"><span id="mcp-server-enabled-label">Enabled in gateway</span><Switch checked={form.enabled} aria-labelledby="mcp-server-enabled-label" onCheckedChange={(enabled) => setForm({ ...form, enabled })} /></label></div></div>
    <Field label="Description" htmlFor="mcp-description"><Textarea id="mcp-description" rows={2} value={form.description} onChange={(event) => setForm({ ...form, description: event.target.value })} /></Field>
    <div className="grid gap-3 sm:grid-cols-2"><Field label="Tools" htmlFor="mcp-tools" hint="One tool name per line."><Textarea id="mcp-tools" rows={5} value={tools} onChange={(event) => setTools(event.target.value)} /></Field><Field label="Required OAuth scopes" htmlFor="mcp-scopes" hint="One scope per line."><Textarea id="mcp-scopes" rows={5} value={scopes} onChange={(event) => setScopes(event.target.value)} /></Field></div>
    <OAuthClientSection server={initial} draft={oauth} onChange={(patch) => setOAuth((current) => ({ ...current, ...patch }))} />
    {error && <p role="alert" className="text-sm text-[color:var(--danger-text)]">{error.message}</p>}
  </div><DialogFooter><Button variant="ghost" onClick={onClose}>Cancel</Button><Button disabled={!valid || pending} onClick={() => onSave({ ...form, slug: initial?.slug ?? slugify(form.slug || form.name), tools: lines(tools), required_scopes: lines(scopes) }, oauthTouched(oauth) && oauthChanged(oauth, initial) ? toOAuthInput(oauth) : null)}>{pending ? "Saving…" : initial ? "Save server" : "Register server"}</Button></DialogFooter></Dialog>;
}

export function McpLibrary() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const query = useQuery({ queryKey: ["mcp-library", orgId], queryFn: () => fetchMcpLibrary(orgId as string), enabled: !!orgId, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "mcp-library");
  const install = useMutation({ mutationFn: (item: McpLibraryItem) => createMcpServer(orgId as string, { ...item, enabled: true, source: "library" }), onSuccess: () => { void client.invalidateQueries({ queryKey: ["mcp-library", orgId] }); void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }); } });
  return <PageBody><PageLead eyebrow="curated endpoints">Install a reviewed server definition into this organization. OAuth consent remains per user; installation never stores a token.</PageLead>
    {/* every curated definition in mcp_oauth.rs carries its tool manifest as
        of #1252, so this row normally has something in it: it is what an
        installed server arrives with, what the tool tally counts, and what a
        tool group can pick from. the guard stays for a control plane older
        than that change, which still answers with empty lists — saying "No
        tools declared" about those stated something false about the catalog
        rather than about the server (#1194) */}
    {query.isLoading ? <CardGridSkeleton cards={4} height={190} min={300} /> : query.error ? <LoadError error={query.error} resource={t("errors.resources.mcpLibrary")} onRetry={() => void query.refetch()} /> : <div className="grid gap-3 md:grid-cols-2">{query.data?.map((item) => <article key={item.slug} className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-5"><div className="flex items-start gap-3"><span className="flex h-10 w-10 items-center justify-center rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)]"><Boxes className="h-5 w-5" aria-hidden /></span><div className="min-w-0 flex-1"><div className="flex flex-wrap items-center gap-2"><h2 className="font-semibold">{item.name}</h2><Badge tone="info">{item.transport.replace("_", " ")}</Badge></div><p className="mt-1 text-sm text-muted-foreground">{item.description}</p></div></div>{item.tools.length > 0 && <div className="mt-4"><ToolBadges tools={item.tools} /></div>}<div className="mt-5 flex items-center border-t border-[color:var(--border-subtle)] pt-4"><span className="text-xs text-muted-foreground">{item.required_scopes.length ? `${item.required_scopes.length} OAuth scopes` : "No OAuth scopes"}</span><Button className="ml-auto" variant={item.installed ? "outline" : "default"} disabled={item.installed || install.isPending} onClick={() => install.mutate(item)}>{install.isPending && install.variables?.slug === item.slug && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}{item.installed ? "Installed" : "Install"}</Button></div></article>)}</div>}
  </PageBody>;
}

export function ToolGroups() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const groups = useQuery({ queryKey: ["mcp-tool-groups", orgId], queryFn: () => fetchMcpToolGroups(orgId as string), enabled: !!orgId, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!groups.isLoading);
  useErrorState(!!groups.error, "tool-groups");
  const servers = useQuery({ queryKey: ["mcp-servers", orgId], queryFn: () => fetchMcpServers(orgId as string), enabled: !!orgId, retry: false });
  const [editing, setEditing] = React.useState<McpToolGroupRow | null | undefined>(undefined);
  const save = useMutation({ mutationFn: ({ initial, input }: { initial: McpToolGroupRow | null; input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at"> }) => initial ? updateMcpToolGroup(initial.id, input) : createMcpToolGroup(orgId as string, input), onSuccess: () => { void client.invalidateQueries({ queryKey: ["mcp-tool-groups", orgId] }); setEditing(undefined); } });
  const remove = useMutation({ mutationFn: deleteMcpToolGroup, onSuccess: () => void client.invalidateQueries({ queryKey: ["mcp-tool-groups", orgId] }) });
  const serverName = (id: string) => servers.data?.find((server) => server.id === id)?.name ?? id.slice(0, 8);
  // was a bare window.confirm that could not name the group it was about (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<McpToolGroupRow | null>(null);
  const startDelete = (group: McpToolGroupRow) => { remove.reset(); setDeleteTarget(group); };
  return <PageBody><PageLead eyebrow="governed manifests" action={<Button disabled={!servers.data?.length} onClick={() => setEditing(null)}><Plus className="h-4 w-4" aria-hidden />Create group</Button>}>Bundle exact server/tool pairs into reusable policy manifests. Groups define intended access; project assignment and gateway enforcement remain separate policy work.</PageLead>
    {groups.isLoading || servers.isLoading ? <CardGridSkeleton cards={3} height={190} min={300} /> : groups.error ? <LoadError error={groups.error} resource={t("errors.resources.toolGroups")} onRetry={() => void groups.refetch()} /> : !groups.data?.length ? <EmptyState uxTarget="tool-groups" icon={<Puzzle />} title={t("pages.tool-groups.emptyTitle")} description={servers.data?.length ? "Create a governed bundle from registered server tools." : "Register MCP servers and declare their tools before creating a group."} actions={servers.data?.length ? <Button onClick={() => setEditing(null)}>Create group</Button> : undefined} /> : <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">{groups.data.map((group) => <article key={group.id} className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"><div className="flex items-center gap-2"><Puzzle className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden /><h2 className="font-semibold">{group.name}</h2><Badge className="ml-auto" tone={group.enabled ? "success" : "neutral"} dot>{group.enabled ? "enabled" : "paused"}</Badge></div><p className="mt-2 min-h-10 text-xs leading-relaxed text-muted-foreground">{group.description || "No description provided."}</p><div className="mt-3 flex flex-wrap gap-1.5">{group.tools.map((item) => <Badge key={`${item.server_id}/${item.tool}`} tone="outline">{serverName(item.server_id)}/{item.tool}</Badge>)}</div><div className="mt-4 flex justify-end gap-1 border-t border-[color:var(--border-subtle)] pt-3"><GatedButton gate="mcp_tool_group:delete" variant="ghost" aria-label={t("pages.mcpCatalog.groups.deleteAria", { name: group.name })} disabled={remove.isPending && remove.variables === group.id} onClick={() => startDelete(group)}>{remove.isPending && remove.variables === group.id && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}Delete</GatedButton><GatedButton gate="mcp_tool_group:update" variant="outline" aria-label={t("pages.mcpCatalog.groups.configureAria", { name: group.name })} onClick={() => setEditing(group)}>Configure</GatedButton></div></article>)}</div>}
    <ConfirmDialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)} title={t("pages.tool-groups.confirm.title", { name: deleteTarget?.name })} description={t("pages.tool-groups.confirm.body")} confirmLabel={t("common.delete")} pending={remove.isPending} error={remove.error} onConfirm={() => deleteTarget && remove.mutate(deleteTarget.id, { onSuccess: () => setDeleteTarget(null) })} />
    {editing !== undefined && <ToolGroupDialog initial={editing} servers={servers.data ?? []} pending={save.isPending} error={save.error} onClose={() => setEditing(undefined)} onSave={(input) => save.mutate({ initial: editing, input })} />}
  </PageBody>;
}

function ToolGroupDialog({ initial, servers, pending, error, onClose, onSave }: { initial: McpToolGroupRow | null; servers: McpServerRow[]; pending: boolean; error: Error | null; onClose: () => void; onSave: (input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at">) => void }) {
  const [name, setName] = React.useState(initial?.name ?? ""); const [description, setDescription] = React.useState(initial?.description ?? ""); const [enabled, setEnabled] = React.useState(initial?.enabled ?? true); const [selected, setSelected] = React.useState<McpToolRef[]>(initial?.tools ?? []);
  const toggle = (item: McpToolRef) => setSelected((current) => current.some((tool) => tool.server_id === item.server_id && tool.tool === item.tool) ? current.filter((tool) => tool.server_id !== item.server_id || tool.tool !== item.tool) : [...current, item]);
  return <Dialog open onClose={onClose}><DialogHeader><DialogTitle>{initial ? "Configure tool group" : "Create tool group"}</DialogTitle><DialogDescription>Select exact tools. A group is a policy manifest, not a credential.</DialogDescription></DialogHeader><div className="grid gap-4 py-4"><div className="grid gap-3 sm:grid-cols-2"><Field label="Name" htmlFor="group-name"><Input id="group-name" value={name} onChange={(event) => setName(event.target.value)} /></Field><label className="mt-auto flex min-h-9 items-center justify-between rounded-md border border-[color:var(--border-default)] px-3 text-sm"><span id="mcp-group-enabled-label">Enabled</span><Switch checked={enabled} aria-labelledby="mcp-group-enabled-label" onCheckedChange={setEnabled} /></label></div><Field label="Description" htmlFor="group-description"><Textarea id="group-description" rows={2} value={description} onChange={(event) => setDescription(event.target.value)} /></Field><fieldset><legend className="text-sm font-medium">Tools</legend><div className="mt-2 max-h-64 space-y-3 overflow-y-auto rounded-[10px] border border-[color:var(--border-default)] p-3">{servers.map((server) => <div key={server.id}><p className="mb-2 font-mono text-xs text-muted-foreground">{server.name}</p><div className="flex flex-wrap gap-2">{server.tools.map((tool) => { const active = selected.some((item) => item.server_id === server.id && item.tool === tool); return <Button key={tool} type="button" variant={active ? "default" : "outline"} aria-pressed={active} onClick={() => toggle({ server_id: server.id, tool })}>{tool}</Button>; })}</div></div>)}</div></fieldset>{error && <p role="alert" className="text-sm text-[color:var(--danger-text)]">{error.message}</p>}</div><DialogFooter><Button variant="ghost" onClick={onClose}>Cancel</Button><Button disabled={!name.trim() || !selected.length || pending} onClick={() => onSave({ name, slug: initial?.slug ?? slugify(name), description, enabled, tools: selected })}>{pending ? "Saving…" : "Save group"}</Button></DialogFooter></Dialog>;
}

export function McpSettings() {
  const { t } = useTranslation();
  const { orgId } = useScope(); const client = useQueryClient();
  const query = useQuery({ queryKey: ["mcp-settings", orgId], queryFn: () => fetchMcpSettings(orgId as string), enabled: !!orgId, retry: false });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "mcp-settings");
  if (!orgId) return <PageBody><EmptyState uxTarget="mcp-settings-no-org" icon={<Settings2 />} title="Choose an organization" description="MCP defaults are organization scoped." /></PageBody>;
  if (query.isLoading) return <PageBody><CardGridSkeleton cards={2} height={190} min={300} /></PageBody>;
  if (query.error) return <PageBody><LoadError error={query.error} resource={t("errors.resources.mcpSettings")} onRetry={() => void query.refetch()} /></PageBody>;
  return <McpSettingsForm initial={query.data as McpGatewaySettingsRow} onSaved={() => void client.invalidateQueries({ queryKey: ["mcp-settings", orgId] })} />;
}

function McpSettingsForm({ initial, onSaved }: { initial: McpGatewaySettingsRow; onSaved: () => void }) {
  const { t } = useTranslation();
  const { orgId } = useScope(); const [form, setForm] = React.useState(initial); const set = (patch: Partial<McpGatewaySettingsRow>) => setForm((current) => ({ ...current, ...patch }));
  const save = useMutation({ mutationFn: () => updateMcpSettings(orgId as string, form), onSuccess: (next) => { setForm(next); onSaved(); } });
  return <PageBody><PageLead eyebrow="organization defaults">Defaults apply when registering new MCP servers. Runtime authorization still comes from server scopes and each user&apos;s OAuth session.</PageLead><div className="grid gap-4 lg:grid-cols-2"><section className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-5"><div className="flex items-center gap-2"><Settings2 className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden /><h2 className="font-semibold">Transport defaults</h2></div><div className="mt-5 grid gap-4"><Field label={t("pages.mcpSettings.defaultTransport")} htmlFor="default-transport"><Select id="default-transport" value={form.default_transport} onChange={(event) => set({ default_transport: event.target.value })}>{TRANSPORTS.map((transport) => <option key={transport}>{transport}</option>)}</Select></Field><Field label="Default failure policy" htmlFor="failure-mode" hint="Stored as the intended policy for clients that consume MCP settings."><Select id="failure-mode" value={form.default_failure_mode} onChange={(event) => set({ default_failure_mode: event.target.value as McpGatewaySettingsRow["default_failure_mode"] })}><option value="fail_closed">Fail closed</option><option value="fail_open">Fail open</option></Select></Field><label className="flex items-start justify-between gap-4 rounded-[10px] border border-[color:var(--border-subtle)] p-3"><span><span id="mcp-unlisted-tools-label" className="block text-sm font-medium">Allow unlisted tools</span><span className="mt-1 block text-xs leading-relaxed text-muted-foreground">Permit clients to request tools not declared in the registry manifest.</span></span><Switch checked={form.allow_unlisted_tools} aria-labelledby="mcp-unlisted-tools-label" onCheckedChange={(value) => set({ allow_unlisted_tools: value })} /></label></div></section><section className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-5"><div className="flex items-center gap-2"><ShieldCheck className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden /><h2 className="font-semibold">Request controls</h2></div><div className="mt-5 grid gap-4 sm:grid-cols-2"><Field label="Connect timeout (ms)" htmlFor="connect-timeout"><Input id="connect-timeout" type="number" min={100} max={60000} value={form.connect_timeout_ms} onChange={(event) => set({ connect_timeout_ms: Number(event.target.value) })} /></Field><Field label="Request timeout (ms)" htmlFor="request-timeout"><Input id="request-timeout" type="number" min={1000} max={300000} value={form.request_timeout_ms} onChange={(event) => set({ request_timeout_ms: Number(event.target.value) })} /></Field><Field label="Maximum retries" htmlFor="max-retries"><Input id="max-retries" type="number" min={0} max={5} value={form.max_retries} onChange={(event) => set({ max_retries: Number(event.target.value) })} /></Field></div><p className="mt-5 flex items-start gap-2 rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-3 text-xs leading-relaxed text-muted-foreground"><Wrench className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />These values are persisted for MCP-aware clients. The current HTTP proxy continues to use deployment-level transport timeouts.</p></section></div>{save.error && <p role="alert" className="text-sm text-[color:var(--danger-text)]">{save.error.message}</p>}<div className="flex justify-end"><Button disabled={save.isPending} onClick={() => save.mutate()}>{save.isPending ? "Saving…" : "Save MCP settings"}</Button></div></PageBody>;
}
