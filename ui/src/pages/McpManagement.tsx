import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import {
  Boxes,
  KeyRound,
  Link2,
  Loader2,
  Plus,
  Puzzle,
  Radar,
  Server,
  Settings2,
  ShieldAlert,
  ShieldCheck,
  Timer,
  Wrench,
} from "lucide-react";
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
import { Combobox } from "@/components/ui/combobox";
import {
  Dialog as BaseDialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
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
  MCP_AUTH_KINDS,
  MCP_OAUTH_DISCOVERY_MODES,
  setMcpOAuthClient,
  setMcpServerAuth,
  startMcpOAuth,
  updateMcpServer,
  updateMcpSettings,
  updateMcpToolGroup,
  type McpGatewaySettingsRow,
  type McpAuthKind,
  type McpLibraryItem,
  type McpOAuthClientInput,
  type McpOAuthDiscovery,
  type McpServerAuthInput,
  type McpServerInput,
  type McpServerRow,
  type McpToolGroupRow,
  type McpToolRef,
  type McpTransportOverridesPatch,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import {
  authDraft,
  authDraftValid,
  authInput,
  carriesCredential,
  dropsCredential,
  headerNameProblem,
  isKekMissing,
  OVERRIDE_BOUNDS,
  OVERRIDE_KEYS,
  overrideDraft,
  overridesPatch,
  overridesValid,
  parseOverride,
  type AuthDraft,
  type OverrideDraft,
  type OverrideKey,
} from "@/lib/mcp-server-auth";
import {
  oauthChanged,
  oauthConnectable,
  oauthDraft,
  oauthEndpoint,
  oauthProblem,
  oauthResetsDiscovery,
  oauthTouched,
  oauthValid,
  toOAuthInput,
  urlResetsDiscovery,
  type OAuthDraft,
} from "@/lib/mcp-oauth-client";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const TRANSPORTS = ["streamable_http", "sse", "websocket"];
const slugify = (value: string) =>
  value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 63);
const lines = (value: string) => [
  ...new Set(
    value
      .split(/[,\n]/)
      .map((item) => item.trim())
      .filter(Boolean),
  ),
];

function Dialog({
  onClose,
  children,
}: {
  open?: boolean;
  onClose: () => void;
  children: React.ReactNode;
}) {
  return (
    <BaseDialog open onOpenChange={(open) => !open && onClose()}>
      {children}
    </BaseDialog>
  );
}

function PageLead({
  eyebrow,
  children,
  action,
}: {
  eyebrow: string;
  children: React.ReactNode;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-4 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end">
      <div className="min-w-0 flex-1">
        <p className="font-mono text-[10px] uppercase tracking-[0.24em] text-[color:var(--red-folk-text)]">
          {eyebrow}
        </p>
        <div className="mt-2 text-sm leading-relaxed text-muted-foreground">{children}</div>
      </div>
      {action}
    </div>
  );
}

function ToolBadges({ tools }: { tools: string[] }) {
  const { t } = useTranslation();
  if (!tools.length)
    return (
      <span className="text-xs text-[color:var(--text-subtle)]">
        {t("pages.mcpCatalog.noTools")}
      </span>
    );
  return (
    <div className="flex flex-wrap gap-1.5">
      {tools.map((tool) => (
        <Badge key={tool} tone="outline">
          {tool}
        </Badge>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// the OAuth client rolter presents to one server's authorization server (#707).
// registering it is what makes the Connect action possible. since #1347 only
// the client id is required: discovery resolves the endpoints, and the typed
// pair is either a fallback (`auto`) or the whole answer (`manual`, #1415)

function DiscoveryPicker({
  value,
  onChange,
}: {
  value: McpOAuthDiscovery;
  onChange: (mode: McpOAuthDiscovery) => void;
}) {
  const { t } = useTranslation();
  return (
    <fieldset>
      <legend className="text-sm font-medium">{t("pages.mcpCatalog.oauth.discovery.label")}</legend>
      <div className="mt-2 grid gap-2 sm:grid-cols-2">
        {MCP_OAUTH_DISCOVERY_MODES.map((mode) => {
          const selected = value === mode;
          return (
            <label
              key={mode}
              className={`flex cursor-pointer items-start gap-2.5 rounded-[10px] border p-3 transition-colors focus-within:ring-1 focus-within:ring-ring ${selected ? "border-[color:var(--red-folk)] bg-card" : "border-[color:var(--border-default)] hover:bg-card/60"}`}
            >
              <input
                type="radio"
                name="mcp-oauth-discovery"
                value={mode}
                checked={selected}
                onChange={() => onChange(mode)}
                className="mt-0.5 accent-[color:var(--red-folk)]"
                aria-describedby={`mcp-oauth-discovery-${mode}-hint`}
              />
              <span className="min-w-0">
                <span className="block text-sm font-medium">
                  {t(`pages.mcpCatalog.oauth.discovery.${mode}.label`)}
                </span>
                <span
                  id={`mcp-oauth-discovery-${mode}-hint`}
                  className="mt-0.5 block text-xs leading-relaxed text-muted-foreground"
                >
                  {t(`pages.mcpCatalog.oauth.discovery.${mode}.hint`)}
                </span>
              </span>
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}

// read-only: what the last successful discovery resolved, so an operator can
// see whether it worked instead of guessing. the control plane only probes on
// an interactive Connect, so a fresh server honestly has nothing to show yet
function DiscoveredEndpoints({ server, resets }: { server: McpServerRow | null; resets: boolean }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const found = server?.oauth_discovered_at ? server : null;
  const rows = found
    ? ([
        [t("pages.mcpCatalog.oauth.discovered.issuer"), found.oauth_discovered_issuer],
        [t("pages.mcpCatalog.oauth.authorizeUrl"), found.oauth_discovered_authorize_url],
        [t("pages.mcpCatalog.oauth.tokenUrl"), found.oauth_discovered_token_url],
      ] as const)
    : [];
  return (
    <div
      role="group"
      aria-labelledby="mcp-oauth-discovered-title"
      className="rounded-[10px] border border-dashed border-[color:var(--border-default)] bg-card p-3"
    >
      <div className="flex flex-wrap items-center gap-2">
        <Radar className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden />
        <h4
          id="mcp-oauth-discovered-title"
          className="text-xs font-semibold uppercase tracking-[0.12em]"
        >
          {t("pages.mcpCatalog.oauth.discovered.title")}
        </h4>
        {found?.oauth_discovered_at && (
          <span className="ml-auto font-mono text-[11px] text-muted-foreground">
            {t("pages.mcpCatalog.oauth.discovered.at", {
              when: fmt.dateTime(found.oauth_discovered_at),
            })}
          </span>
        )}
      </div>
      {found ? (
        <>
          <dl className="mt-3 grid gap-2 text-xs">
            {rows
              .filter(([, value]) => value)
              .map(([label, value]) => (
                <div key={label} className="grid gap-0.5 sm:grid-cols-4 sm:gap-3">
                  <dt className="text-muted-foreground">{label}</dt>
                  <dd className="min-w-0 break-all font-mono sm:col-span-3">{value}</dd>
                </div>
              ))}
          </dl>
          <div className="mt-3">
            <Badge
              tone={found.oauth_discovered_iss_supported ? "success" : "neutral"}
              dot={found.oauth_discovered_iss_supported}
            >
              {found.oauth_discovered_iss_supported
                ? t("pages.mcpCatalog.oauth.discovered.issSupported")
                : t("pages.mcpCatalog.oauth.discovered.issUnsupported")}
            </Badge>
          </div>
        </>
      ) : (
        <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
          {t("pages.mcpCatalog.oauth.discovered.none")}
        </p>
      )}
      {resets && (
        <p className="mt-3 text-xs leading-relaxed text-[color:var(--status-warning-text)]">
          {t("pages.mcpCatalog.oauth.discovered.resets")}
        </p>
      )}
    </div>
  );
}

const endpointError = (t: TFunction, value: string) =>
  value.trim() && !oauthEndpoint(value.trim())
    ? t("pages.mcpCatalog.oauth.endpointError")
    : undefined;

function OAuthClientSection({
  server,
  url,
  draft,
  onChange,
}: {
  server: McpServerRow | null;
  url: string;
  draft: OAuthDraft;
  onChange: (patch: Partial<OAuthDraft>) => void;
}) {
  const { t } = useTranslation();
  // read only for the redirect uri, which is deployment-derived and so cannot
  // be worked out from the browser's own origin. best-effort: the section still
  // works off the server row when the read is refused or the server is new
  const client = useQuery({
    queryKey: ["mcp-oauth-client", server?.id],
    queryFn: () => fetchMcpOAuthClient(server?.id as string),
    enabled: !!server,
    retry: false,
  });
  const stored = server?.has_client_secret ?? false;
  const manual = draft.discovery === "manual";
  const problem = oauthTouched(draft, server) ? oauthProblem(draft) : null;
  return (
    <section className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-4">
      <div className="flex flex-wrap items-center gap-2">
        <KeyRound className="h-4 w-4 text-[color:var(--red-folk)]" aria-hidden />
        <h3 className="text-sm font-semibold">{t("pages.mcpCatalog.oauth.title")}</h3>
        <Badge tone={stored && !draft.clearSecret ? "success" : "neutral"}>
          {stored && !draft.clearSecret
            ? t("pages.mcpCatalog.oauth.secretSet")
            : t("pages.mcpCatalog.oauth.secretMissing")}
        </Badge>
      </div>
      <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
        {t("pages.mcpCatalog.oauth.lead")}
      </p>
      <div className="mt-4 grid gap-3">
        <DiscoveryPicker
          value={draft.discovery}
          onChange={(discovery) => onChange({ discovery })}
        />
        {!manual && (
          <DiscoveredEndpoints server={server} resets={oauthResetsDiscovery(draft, server, url)} />
        )}
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pages.mcpCatalog.oauth.clientId")} htmlFor="mcp-client-id">
            <Input
              id="mcp-client-id"
              value={draft.clientId}
              onChange={(event) => onChange({ clientId: event.target.value })}
            />
          </Field>
          <Field
            label={t("pages.mcpCatalog.oauth.clientSecret")}
            htmlFor="mcp-client-secret"
            hint={
              draft.clearSecret
                ? t("pages.mcpCatalog.oauth.clearingSecret")
                : t("pages.mcpCatalog.oauth.secretHint")
            }
          >
            <Input
              id="mcp-client-secret"
              type="password"
              autoComplete="new-password"
              disabled={draft.clearSecret}
              placeholder={stored ? t("pages.mcpCatalog.oauth.secretPlaceholder") : undefined}
              value={draft.secret}
              onChange={(event) => onChange({ secret: event.target.value })}
            />
          </Field>
        </div>
        {stored && (
          <div>
            <Button
              type="button"
              variant={draft.clearSecret ? "default" : "outline"}
              aria-pressed={draft.clearSecret}
              onClick={() => onChange({ clearSecret: !draft.clearSecret, secret: "" })}
            >
              {t("pages.mcpCatalog.oauth.clearSecret")}
            </Button>
          </div>
        )}
        <div
          role="group"
          aria-labelledby="mcp-oauth-endpoints-title"
          aria-describedby="mcp-oauth-endpoints-hint"
          className="grid gap-3 border-t border-[color:var(--border-subtle)] pt-3"
        >
          <div>
            <h4 id="mcp-oauth-endpoints-title" className="text-sm font-medium">
              {manual
                ? t("pages.mcpCatalog.oauth.endpoints.manualTitle")
                : t("pages.mcpCatalog.oauth.endpoints.fallbackTitle")}
            </h4>
            <p
              id="mcp-oauth-endpoints-hint"
              className="mt-0.5 text-xs leading-relaxed text-muted-foreground"
            >
              {manual
                ? t("pages.mcpCatalog.oauth.endpoints.manualHint")
                : t("pages.mcpCatalog.oauth.endpoints.fallbackHint")}
            </p>
          </div>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field
              label={t("pages.mcpCatalog.oauth.authorizeUrl")}
              htmlFor="mcp-authorize-url"
              error={endpointError(t, draft.authorizeUrl)}
            >
              <Input
                id="mcp-authorize-url"
                className="font-mono"
                spellCheck={false}
                required={manual}
                value={draft.authorizeUrl}
                onChange={(event) => onChange({ authorizeUrl: event.target.value })}
              />
            </Field>
            <Field
              label={t("pages.mcpCatalog.oauth.tokenUrl")}
              htmlFor="mcp-token-url"
              error={endpointError(t, draft.tokenUrl)}
            >
              <Input
                id="mcp-token-url"
                className="font-mono"
                spellCheck={false}
                required={manual}
                value={draft.tokenUrl}
                onChange={(event) => onChange({ tokenUrl: event.target.value })}
              />
            </Field>
          </div>
        </div>
        <Field
          label={t("pages.mcpCatalog.oauth.issuer")}
          htmlFor="mcp-oauth-issuer"
          hint={endpointError(t, draft.issuer) ? undefined : t("pages.mcpCatalog.oauth.issuerHint")}
          error={endpointError(t, draft.issuer)}
        >
          <Input
            id="mcp-oauth-issuer"
            className="font-mono"
            spellCheck={false}
            placeholder={t("pages.mcpCatalog.oauth.issuerPlaceholder")}
            value={draft.issuer}
            onChange={(event) => onChange({ issuer: event.target.value })}
          />
        </Field>
        <Field
          label={t("pages.mcpCatalog.oauth.scopes")}
          htmlFor="mcp-default-scopes"
          hint={t("pages.mcpCatalog.oauth.scopesHint")}
        >
          <Input
            id="mcp-default-scopes"
            value={draft.scopes}
            onChange={(event) => onChange({ scopes: event.target.value })}
          />
        </Field>
        {client.data && (
          <Field
            label={t("pages.mcpCatalog.oauth.redirectUri")}
            htmlFor="mcp-redirect-uri"
            hint={t("pages.mcpCatalog.oauth.redirectHint")}
          >
            <Input id="mcp-redirect-uri" readOnly value={client.data.redirect_uri} />
          </Field>
        )}
        {problem && problem !== "endpoint" && (
          <p role="alert" className="text-xs text-[color:var(--danger-text)]">
            {t(`pages.mcpCatalog.oauth.problems.${problem}`)}
          </p>
        )}
      </div>
    </section>
  );
}

// starts consent and hands the browser the url the control plane minted. the
// dashboard never navigates itself there: the consent screen belongs to a third
// party, and the operator should come back to a page that kept its place
function ConnectButton({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const toast = useToast();
  const ready = oauthConnectable(server);
  const connect = useMutation({
    mutationFn: () => startMcpOAuth(server.id),
    onSuccess: (started) => {
      const opened = window.open(started.authorization_url, "_blank", "noopener,noreferrer");
      // a blocked pop-up is silent otherwise: the request succeeded, a login
      // state row exists upstream, and nothing at all appeared on screen
      if (!opened)
        return void toast.push({
          tone: "error",
          title: t("pages.mcpCatalog.connect.blocked"),
          detail: t("pages.mcpCatalog.connect.blockedDetail"),
        });
      toast.push({
        tone: "info",
        title: t("pages.mcpCatalog.connect.started", { name: server.name }),
        detail: t("pages.mcpCatalog.connect.startedDetail"),
      });
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpCatalog.connect.failed", { name: server.name }),
        detail: errorDetail(error),
      }),
  });
  return (
    <Button
      variant="outline"
      disabled={!ready || connect.isPending}
      title={ready ? undefined : t("pages.mcpCatalog.connect.unconfigured")}
      aria-label={t("pages.mcpCatalog.connect.ready", { name: server.name })}
      onClick={() => connect.mutate()}
    >
      {connect.isPending ? (
        <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden />
      ) : (
        <Link2 className="mr-2 h-4 w-4" aria-hidden />
      )}
      {t("pages.mcpCatalog.connect.action")}
    </Button>
  );
}

function ConfirmDelete({
  server,
  pending,
  error,
  onClose,
  onConfirm,
}: {
  server: McpServerRow;
  pending: boolean;
  error: Error | null;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open onClose={onClose}>
      <DialogHeader>
        <DialogTitle>
          {t("pages.mcpCatalog.confirmDelete.title", { name: server.name })}
        </DialogTitle>
        <DialogDescription>{t("pages.mcpCatalog.confirmDelete.body")}</DialogDescription>
      </DialogHeader>
      {error && (
        <p role="alert" className="text-sm text-[color:var(--danger-text)]">
          {error.message}
        </p>
      )}
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button variant="destructive" disabled={pending} onClick={onConfirm}>
          {pending
            ? t("pages.mcpCatalog.confirmDelete.pending")
            : t("pages.mcpCatalog.confirmDelete.confirm")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

export function McpCatalog() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "mcp-servers");
  const [editing, setEditing] = React.useState<McpServerRow | null | undefined>(undefined);
  const [deleting, setDeleting] = React.useState<McpServerRow | null>(null);
  // the OAuth client lives behind its own endpoint, keyed by server id, so it
  // is written after the row — which is also how a client can be registered on
  // a server in the same breath as creating it
  // the same holds for authentication (#1447): the credential has a route of
  // its own so the general PATCH never carries a secret. a create takes no
  // override fields, so a new server that asks for any gets them in a PATCH
  // straight after. once the row exists the dialog is re-pointed at it, so a
  // later step that fails — a missing ROLTER_KEK, most likely — is retried as
  // an edit instead of a second create colliding on the slug
  const save = useMutation({
    mutationFn: async ({ initial, draft }: { initial: McpServerRow | null; draft: ServerSave }) => {
      let server = initial
        ? await updateMcpServer(initial.id, { ...draft.input, ...draft.overrides })
        : await createMcpServer(orgId as string, draft.input);
      if (!initial) {
        setEditing(server);
        if (Object.keys(draft.overrides).length)
          server = await updateMcpServer(server.id, { ...draft.input, ...draft.overrides });
      }
      if (draft.auth) server = await setMcpServerAuth(server.id, draft.auth);
      if (draft.oauth) await setMcpOAuthClient(server.id, draft.oauth);
      return server;
    },
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] });
      void client.invalidateQueries({ queryKey: ["mcp-oauth-client"] });
    },
    onSuccess: () => setEditing(undefined),
  });
  const remove = useMutation({
    mutationFn: deleteMcpServer,
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] });
      setDeleting(null);
    },
  });
  const toggle = useMutation({
    mutationFn: (server: McpServerRow) =>
      updateMcpServer(server.id, { ...serverInput(server), enabled: !server.enabled }),
    onSuccess: () => void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }),
  });
  if (!orgId)
    return (
      <PageBody>
        <EmptyState
          uxTarget="mcp-no-org"
          icon={<Server />}
          title={t("pages.mcpCatalog.noOrgTitle")}
          description={t("pages.mcpCatalog.noOrgBody")}
        />
      </PageBody>
    );
  // only enabled servers reach the gateway snapshot, so the tool tally counts theirs
  const live = query.data?.filter((server) => server.enabled) ?? [];
  const liveTools = live.reduce((sum, server) => sum + server.tools.length, 0);
  return (
    <PageBody>
      <PageLead
        eyebrow={t("pages.mcpCatalog.eyebrow")}
        action={
          <GatedButton gate="mcp_server:create" onClick={() => setEditing(null)}>
            <Plus className="h-4 w-4" aria-hidden />
            {t("pages.mcpCatalog.registerServer")}
          </GatedButton>
        }
      >
        {query.data ? (
          <>
            <span>{t("pages.mcpCatalog.enabledServers", { count: live.length })}</span>
            {" · "}
            <span>{t("pages.mcpCatalog.declaredTools", { count: liveTools })}</span>
          </>
        ) : (
          t("pages.mcpCatalog.registryHint")
        )}
      </PageLead>
      {query.isLoading ? (
        <CardGridSkeleton cards={3} height={190} min={300} />
      ) : query.error ? (
        <LoadError
          error={query.error}
          resource={t("errors.resources.mcpServers")}
          onRetry={() => void query.refetch()}
        />
      ) : !query.data?.length ? (
        <EmptyState
          uxTarget="mcp-servers"
          icon={<Server />}
          title={t("pages.mcpCatalog.emptyTitle")}
          description={t("pages.mcpCatalog.emptyBody")}
          actions={
            <GatedButton gate="mcp_server:create" onClick={() => setEditing(null)}>
              {t("pages.mcpCatalog.registerServer")}
            </GatedButton>
          }
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {query.data.map((server) => (
            <article
              key={server.id}
              className={`min-w-0 rounded-[10px] border border-[color:var(--border-default)] p-4 ${server.enabled ? "bg-card" : "bg-[color:var(--surface-subtle)]/60"}`}
            >
              <div className="flex items-start gap-3">
                <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)]">
                  <Server className="h-4 w-4" aria-hidden />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="truncate font-mono text-sm font-semibold">{server.name}</h2>
                    <Badge tone={server.source === "library" ? "accent" : "neutral"}>
                      {server.source}
                    </Badge>
                  </div>
                  <p className="mt-1 truncate font-mono text-xs text-muted-foreground">
                    {server.url}
                  </p>
                </div>
                <GatedSwitch
                  gate="mcp_server:update"
                  checked={server.enabled}
                  aria-label={t("pages.mcpCatalog.servers.toggleAria", { name: server.name })}
                  onCheckedChange={() => toggle.mutate(server)}
                />
              </div>
              <p className="mt-3 min-h-10 text-xs leading-relaxed text-muted-foreground">
                {server.description || t("pages.mcpCatalog.noDescription")}
              </p>
              <div className="mt-3">
                <ToolBadges tools={server.tools} />
              </div>
              <div className="mt-4 flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <Badge tone="info">{server.transport.replace("_", " ")}</Badge>
                <AuthBadge server={server} />
                <span className="ml-auto flex flex-wrap justify-end gap-1">
                  <ConnectButton server={server} />
                  <GatedButton
                    gate="mcp_server:delete"
                    variant="ghost"
                    aria-label={t("pages.mcpCatalog.servers.deleteAria", { name: server.name })}
                    onClick={() => setDeleting(server)}
                  >
                    {t("common.delete")}
                  </GatedButton>
                  <GatedButton
                    gate="mcp_server:update"
                    variant="outline"
                    aria-label={t("pages.mcpCatalog.servers.configureAria", { name: server.name })}
                    onClick={() => setEditing(server)}
                  >
                    {t("pages.mcpCatalog.configure")}
                  </GatedButton>
                </span>
              </div>
            </article>
          ))}
        </div>
      )}
      {editing !== undefined && (
        <ServerDialog
          initial={editing}
          pending={save.isPending}
          error={save.error}
          onClose={() => {
            save.reset();
            setEditing(undefined);
          }}
          onSave={(draft) => save.mutate({ initial: editing, draft })}
        />
      )}
      {deleting && (
        <ConfirmDelete
          server={deleting}
          pending={remove.isPending}
          error={remove.error}
          onClose={() => setDeleting(null)}
          onConfirm={() => remove.mutate(deleting.id)}
        />
      )}
    </PageBody>
  );
}

// the PATCH-able part of a row. picked field by field rather than spread, so a
// save never echoes the overrides back — sending one unchanged would still be
// sending it, and the absent/null distinction is the whole contract (#1447)
const serverInput = (server: McpServerRow): McpServerInput => ({
  name: server.name,
  slug: server.slug,
  url: server.url,
  transport: server.transport,
  description: server.description,
  enabled: server.enabled,
  tools: server.tools,
  source: server.source,
  required_scopes: server.required_scopes,
});

interface ServerSave {
  input: McpServerInput;
  overrides: McpTransportOverridesPatch;
  auth: McpServerAuthInput | null;
  oauth: McpOAuthClientInput | null;
}

// the card says how a server authenticates, so a bearer-armed server is not
// indistinguishable from an open one until someone opens Configure
function AuthBadge({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const armed = carriesCredential(server.auth_kind) && server.has_credential;
  return (
    <Badge
      tone={
        server.auth_kind === "none"
          ? "neutral"
          : armed || server.auth_kind === "oauth"
            ? "success"
            : "warning"
      }
    >
      {t(`pages.mcpCatalog.auth.kinds.${server.auth_kind}.badge`)}
    </Badge>
  );
}

// the refusal a deployment without ROLTER_KEK gives. it is a deployment
// problem rather than anything in the form, so it says what to change and
// where; the control plane's own message stays underneath it
function SaveError({ error }: { error: Error }) {
  const { t } = useTranslation();
  if (!isKekMissing(error))
    return (
      <p role="alert" className="text-sm text-[color:var(--danger-text)]">
        {error.message}
      </p>
    );
  return (
    <div
      role="alert"
      className="flex items-start gap-2.5 rounded-[10px] border border-[color:var(--status-danger-text)]/40 bg-[color:var(--surface-subtle)] p-3"
    >
      <ShieldAlert
        className="mt-0.5 h-4 w-4 shrink-0 text-[color:var(--status-danger-text)]"
        aria-hidden
      />
      <div className="min-w-0 text-xs leading-relaxed">
        <p className="font-semibold text-foreground">{t("pages.mcpCatalog.auth.kekTitle")}</p>
        <p className="mt-1 text-muted-foreground">{t("pages.mcpCatalog.auth.kekBody")}</p>
        <p className="mt-1 font-mono text-[color:var(--text-subtle)]">{error.message}</p>
      </div>
    </div>
  );
}

// one card per kind, each saying what Rolter will actually send. `none` is
// spelled out as unauthenticated so it cannot be read as "not configured yet"
function AuthKindPicker({
  value,
  onChange,
}: {
  value: McpAuthKind;
  onChange: (kind: McpAuthKind) => void;
}) {
  const { t } = useTranslation();
  return (
    <fieldset>
      <legend className="text-sm font-medium">{t("pages.mcpCatalog.auth.kindLabel")}</legend>
      <div className="mt-2 grid gap-2 sm:grid-cols-2">
        {MCP_AUTH_KINDS.map((kind) => {
          const selected = value === kind;
          return (
            <label
              key={kind}
              className={`flex cursor-pointer items-start gap-2.5 rounded-[10px] border p-3 transition-colors focus-within:ring-1 focus-within:ring-ring ${selected ? "border-[color:var(--red-folk)] bg-[color:var(--surface-subtle)]" : "border-[color:var(--border-default)] hover:bg-[color:var(--surface-subtle)]/60"}`}
            >
              <input
                type="radio"
                name="mcp-auth-kind"
                value={kind}
                checked={selected}
                onChange={() => onChange(kind)}
                className="mt-0.5 accent-[color:var(--red-folk)]"
                aria-describedby={`mcp-auth-kind-${kind}-hint`}
              />
              <span className="min-w-0">
                <span className="block text-sm font-medium">
                  {t(`pages.mcpCatalog.auth.kinds.${kind}.label`)}
                </span>
                <span
                  id={`mcp-auth-kind-${kind}-hint`}
                  className="mt-0.5 block text-xs leading-relaxed text-muted-foreground"
                >
                  {t(`pages.mcpCatalog.auth.kinds.${kind}.hint`)}
                </span>
              </span>
            </label>
          );
        })}
      </div>
    </fieldset>
  );
}

function headerNameError(t: TFunction, name: string): string | undefined {
  const problem = headerNameProblem(name.trim());
  if (!problem || (problem === "required" && !name)) return undefined;
  return problem === "reserved"
    ? t("pages.mcpCatalog.auth.headerReserved", { name: name.trim() })
    : t("pages.mcpCatalog.auth.headerShape");
}

// the static-credential branch: bearer and header. the credential input is
// write-only — never pre-filled, and blank means "keep the stored one"
function CredentialSection({
  server,
  draft,
  onChange,
  onClear,
}: {
  server: McpServerRow | null;
  draft: AuthDraft;
  onChange: (patch: Partial<AuthDraft>) => void;
  onClear: () => void;
}) {
  const { t } = useTranslation();
  const stored = !!server?.has_credential;
  const kept = stored && !draft.credential;
  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={stored ? "success" : "neutral"} dot={stored}>
          {stored
            ? t("pages.mcpCatalog.auth.credentialStored")
            : t("pages.mcpCatalog.auth.credentialMissing")}
        </Badge>
        {stored && (
          <GatedButton
            gate="mcp_server:update"
            type="button"
            variant="outline"
            size="sm"
            className="ml-auto"
            onClick={onClear}
          >
            {t("pages.mcpCatalog.auth.clearCredential")}
          </GatedButton>
        )}
      </div>
      {draft.kind === "header" && (
        <Field
          label={t("pages.mcpCatalog.auth.headerName")}
          htmlFor="mcp-auth-header"
          hint={t("pages.mcpCatalog.auth.headerHint")}
          error={headerNameError(t, draft.headerName)}
        >
          <Input
            id="mcp-auth-header"
            className="font-mono"
            autoComplete="off"
            spellCheck={false}
            placeholder={t("pages.mcpCatalog.auth.headerPlaceholder")}
            value={draft.headerName}
            onChange={(event) => onChange({ headerName: event.target.value })}
          />
        </Field>
      )}
      <Field
        label={
          draft.kind === "header"
            ? t("pages.mcpCatalog.auth.apiKey")
            : t("pages.mcpCatalog.auth.bearerToken")
        }
        htmlFor="mcp-auth-credential"
        hint={
          kept
            ? t("pages.mcpCatalog.auth.credentialKeepHint")
            : t("pages.mcpCatalog.auth.credentialHint")
        }
      >
        <Input
          id="mcp-auth-credential"
          type="password"
          autoComplete="new-password"
          placeholder={stored ? t("pages.mcpCatalog.auth.credentialPlaceholder") : undefined}
          value={draft.credential}
          onChange={(event) => onChange({ credential: event.target.value })}
        />
      </Field>
    </div>
  );
}

// blank inherits, and the placeholder says so rather than showing a number:
// the org-wide MCP settings do not reach the proxy yet (#1404), so quoting them
// here would describe a default that is not the one in effect
function OverridesSection({
  draft,
  onChange,
}: {
  draft: OverrideDraft;
  onChange: (patch: Partial<OverrideDraft>) => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const labels: Record<OverrideKey, string> = {
    connect_timeout_ms: t("pages.mcpCatalog.overrides.connectTimeout"),
    request_timeout_ms: t("pages.mcpCatalog.overrides.requestTimeout"),
    max_retries: t("pages.mcpCatalog.overrides.maxRetries"),
  };
  return (
    <section className="rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div className="flex items-center gap-2">
        <Timer className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden />
        <h3 className="text-sm font-semibold">{t("pages.mcpCatalog.overrides.title")}</h3>
      </div>
      <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
        {t("pages.mcpCatalog.overrides.lead")}
      </p>
      <div className="mt-4 grid gap-3 sm:grid-cols-3">
        {OVERRIDE_KEYS.map((key) => {
          const { min, max } = OVERRIDE_BOUNDS[key];
          const range = { min: fmt.number(min), max: fmt.number(max) };
          const bad = parseOverride(key, draft[key]) === undefined;
          return (
            <Field
              key={key}
              label={labels[key]}
              htmlFor={`mcp-override-${key}`}
              hint={bad ? undefined : t("pages.mcpCatalog.overrides.range", range)}
              error={bad ? t("pages.mcpCatalog.overrides.outOfRange", range) : undefined}
            >
              <Input
                id={`mcp-override-${key}`}
                inputMode="numeric"
                className="font-mono"
                placeholder={t("pages.mcpCatalog.overrides.inherit")}
                value={draft[key]}
                onChange={(event) => onChange({ [key]: event.target.value })}
              />
            </Field>
          );
        })}
      </div>
    </section>
  );
}

function ServerDialog({
  initial,
  pending,
  error,
  onClose,
  onSave,
}: {
  initial: McpServerRow | null;
  pending: boolean;
  error: Error | null;
  onClose: () => void;
  onSave: (draft: ServerSave) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [form, setForm] = React.useState<McpServerInput>(() =>
    initial
      ? serverInput(initial)
      : {
          name: "",
          slug: "",
          url: "",
          transport: "streamable_http",
          description: "",
          enabled: true,
          tools: [],
          source: "custom",
          required_scopes: [],
        },
  );
  const [tools, setTools] = React.useState(form.tools.join("\n"));
  const [scopes, setScopes] = React.useState(form.required_scopes.join("\n"));
  const [oauth, setOAuth] = React.useState<OAuthDraft>(() => oauthDraft(initial));
  // the row the auth diff is taken against. it moves when the credential is
  // cleared from inside the dialog, so a later save compares with what is
  // stored now rather than with what was stored when the dialog opened
  const [row, setRow] = React.useState<McpServerRow | null>(initial);
  React.useEffect(() => setRow((current) => current ?? initial), [initial]);
  const [auth, setAuth] = React.useState<AuthDraft>(() => authDraft(initial));
  const [overrides, setOverrides] = React.useState<OverrideDraft>(() => overrideDraft(initial));
  const [confirming, setConfirming] = React.useState<"save" | "clear" | null>(null);

  // clearing is its own immediate action rather than a draft flag: the schema
  // lets a bearer or header server exist only with a credential, so "clear" is
  // a move to `none` — the server is unauthenticated from that moment on
  const clear = useMutation({
    mutationFn: (id: string) => setMcpServerAuth(id, { auth_kind: "none" }),
    onSuccess: (next) => {
      setRow(next);
      setAuth(authDraft(next));
      setConfirming(null);
      void client.invalidateQueries({ queryKey: ["mcp-servers", next.org_id] });
    },
  });

  const clientValid = auth.kind !== "oauth" || oauthValid(oauth, initial);
  const valid =
    form.name.trim() &&
    (initial || slugify(form.slug || form.name)) &&
    /^https?:\/\//.test(form.url) &&
    clientValid &&
    authDraftValid(auth, row) &&
    overridesValid(overrides);
  const draft = (): ServerSave => ({
    input: {
      ...form,
      slug: initial?.slug ?? slugify(form.slug || form.name),
      tools: lines(tools),
      required_scopes: lines(scopes),
    },
    overrides: overridesPatch(overrides, initial),
    auth: authInput(auth, row),
    oauth:
      auth.kind === "oauth" && oauthTouched(oauth, initial) && oauthChanged(oauth, initial)
        ? toOAuthInput(oauth)
        : null,
  });
  const submit = () => (dropsCredential(auth, row) ? setConfirming("save") : onSave(draft()));
  const name = form.name.trim() || initial?.name || "";
  // said beside the url itself, whatever the auth kind: the store drops the
  // discovery cache on any url move (#1416), and the panel that also warns is
  // only on screen for an oauth server under auto discovery
  const urlResets = urlResetsDiscovery(form.url, initial);

  return (
    <>
      <BaseDialog open={!confirming} onOpenChange={(open) => !open && onClose()}>
        <DialogHeader>
          <DialogTitle>
            {initial
              ? t("pages.mcpCatalog.dialog.titleEdit")
              : t("pages.mcpCatalog.dialog.titleAdd")}
          </DialogTitle>
          <DialogDescription>{t("pages.mcpCatalog.dialog.lead")}</DialogDescription>
        </DialogHeader>
        <div className="grid max-h-[70vh] gap-4 overflow-y-auto py-4 pr-1">
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t("pages.mcpCatalog.fields.name")} htmlFor="mcp-name">
              <Input
                id="mcp-name"
                value={form.name}
                onChange={(event) => setForm({ ...form, name: event.target.value })}
              />
            </Field>
            <Field
              label={t("pages.mcpCatalog.fields.slug")}
              htmlFor="mcp-slug"
              hint={t("pages.mcpCatalog.fields.slugHint")}
            >
              <Input
                id="mcp-slug"
                disabled={!!initial}
                value={initial?.slug ?? form.slug}
                onChange={(event) => setForm({ ...form, slug: event.target.value })}
              />
            </Field>
          </div>
          <Field label={t("pages.mcpCatalog.fields.url")} htmlFor="mcp-url">
            <Input
              id="mcp-url"
              aria-describedby={urlResets ? "mcp-url-resets-discovery" : undefined}
              value={form.url}
              onChange={(event) => setForm({ ...form, url: event.target.value })}
            />
            {urlResets && (
              <p
                id="mcp-url-resets-discovery"
                className="text-xs leading-relaxed text-[color:var(--status-warning-text)]"
              >
                {t("pages.mcpCatalog.fields.urlResetsDiscovery")}
              </p>
            )}
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label={t("pages.mcpCatalog.fields.transport")} htmlFor="mcp-transport">
              <Combobox
                id="mcp-transport"
                value={form.transport}
                onChange={(transport) => setForm({ ...form, transport })}
                options={TRANSPORTS.map((transport) => ({ value: transport, label: transport }))}
              />
            </Field>
            <div className="flex items-end">
              <label className="flex min-h-9 w-full items-center justify-between rounded-md border border-[color:var(--border-default)] px-3 text-sm">
                <span id="mcp-server-enabled-label">
                  {t("pages.mcpCatalog.fields.enabledInGateway")}
                </span>
                <Switch
                  checked={form.enabled}
                  aria-labelledby="mcp-server-enabled-label"
                  onCheckedChange={(enabled) => setForm({ ...form, enabled })}
                />
              </label>
            </div>
          </div>
          <Field label={t("pages.mcpCatalog.fields.description")} htmlFor="mcp-description">
            <Textarea
              id="mcp-description"
              rows={2}
              value={form.description}
              onChange={(event) => setForm({ ...form, description: event.target.value })}
            />
          </Field>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field
              label={t("pages.mcpCatalog.fields.tools")}
              htmlFor="mcp-tools"
              hint={t("pages.mcpCatalog.fields.toolsHint")}
            >
              <Textarea
                id="mcp-tools"
                rows={5}
                value={tools}
                onChange={(event) => setTools(event.target.value)}
              />
            </Field>
            <Field
              label={t("pages.mcpCatalog.fields.scopes")}
              htmlFor="mcp-scopes"
              hint={t("pages.mcpCatalog.fields.scopesHint")}
            >
              <Textarea
                id="mcp-scopes"
                rows={5}
                value={scopes}
                onChange={(event) => setScopes(event.target.value)}
              />
            </Field>
          </div>
          <section className="grid gap-4 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
            <div>
              <div className="flex items-center gap-2">
                <KeyRound className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden />
                <h3 className="text-sm font-semibold">{t("pages.mcpCatalog.auth.title")}</h3>
              </div>
              <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
                {t("pages.mcpCatalog.auth.lead")}
              </p>
            </div>
            <AuthKindPicker
              value={auth.kind}
              onChange={(kind) => setAuth((current) => ({ ...current, kind }))}
            />
            {carriesCredential(auth.kind) && (
              <CredentialSection
                server={row}
                draft={auth}
                onChange={(patch) => setAuth((current) => ({ ...current, ...patch }))}
                onClear={() => {
                  clear.reset();
                  setConfirming("clear");
                }}
              />
            )}
            {auth.kind === "oauth" && (
              <OAuthClientSection
                server={initial}
                url={form.url}
                draft={oauth}
                onChange={(patch) => setOAuth((current) => ({ ...current, ...patch }))}
              />
            )}
            {dropsCredential(auth, row) && (
              <p className="text-xs leading-relaxed text-[color:var(--status-warning-text)]">
                {t("pages.mcpCatalog.auth.dropsOnSave")}
              </p>
            )}
          </section>
          <OverridesSection
            draft={overrides}
            onChange={(patch) => setOverrides((current) => ({ ...current, ...patch }))}
          />
          {error && <SaveError error={error} />}
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button disabled={!valid || pending} onClick={submit}>
            {pending
              ? t("common.saving")
              : initial
                ? t("pages.mcpCatalog.dialog.saveServer")
                : t("pages.mcpCatalog.registerServer")}
          </Button>
        </DialogFooter>
      </BaseDialog>
      <ConfirmDialog
        open={confirming !== null}
        onOpenChange={(open) => !open && setConfirming(null)}
        title={t("pages.mcpCatalog.confirm.clearCredentialTitle", { name })}
        description={
          confirming === "save"
            ? t("pages.mcpCatalog.confirm.switchKindBody", {
                kind: t(`pages.mcpCatalog.auth.kinds.${auth.kind}.label`),
              })
            : t("pages.mcpCatalog.confirm.clearCredentialBody")
        }
        confirmLabel={t("pages.mcpCatalog.confirm.clearCredentialConfirm")}
        pending={confirming === "save" ? pending : clear.isPending}
        error={confirming === "save" ? error : clear.error}
        onConfirm={() => {
          if (confirming === "clear" && row) clear.mutate(row.id);
          else if (confirming === "save") onSave(draft());
        }}
      />
    </>
  );
}

export function McpLibrary() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["mcp-library", orgId],
    queryFn: () => fetchMcpLibrary(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "mcp-library");
  const install = useMutation({
    mutationFn: (item: McpLibraryItem) =>
      createMcpServer(orgId as string, { ...item, enabled: true, source: "library" }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["mcp-library", orgId] });
      void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] });
    },
  });
  return (
    <PageBody>
      <PageLead eyebrow={t("pages.mcpLibrary.eyebrow")}>{t("pages.mcpLibrary.lead")}</PageLead>
      {/* every curated definition in mcp_oauth.rs carries its tool manifest as
        of #1252, so this row normally has something in it: it is what an
        installed server arrives with, what the tool tally counts, and what a
        tool group can pick from. the guard stays for a control plane older
        than that change, which still answers with empty lists — saying "No
        tools declared" about those stated something false about the catalog
        rather than about the server (#1194) */}
      {query.isLoading ? (
        <CardGridSkeleton cards={4} height={190} min={300} />
      ) : query.error ? (
        <LoadError
          error={query.error}
          resource={t("errors.resources.mcpLibrary")}
          onRetry={() => void query.refetch()}
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2">
          {query.data?.map((item) => (
            <article
              key={item.slug}
              className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-5"
            >
              <div className="flex items-start gap-3">
                <span className="flex h-10 w-10 items-center justify-center rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)]">
                  <Boxes className="h-5 w-5" aria-hidden />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <h2 className="font-semibold">{item.name}</h2>
                    <Badge tone="info">{item.transport.replace("_", " ")}</Badge>
                  </div>
                  <p className="mt-1 text-sm text-muted-foreground">{item.description}</p>
                </div>
              </div>
              {item.tools.length > 0 && (
                <div className="mt-4">
                  <ToolBadges tools={item.tools} />
                </div>
              )}
              <div className="mt-5 flex items-center border-t border-[color:var(--border-subtle)] pt-4">
                <span className="text-xs text-muted-foreground">
                  {item.required_scopes.length
                    ? t("pages.mcpLibrary.scopesCount", { count: item.required_scopes.length })
                    : t("pages.mcpLibrary.noScopes")}
                </span>
                <Button
                  className="ml-auto"
                  variant={item.installed ? "outline" : "default"}
                  disabled={item.installed || install.isPending}
                  onClick={() => install.mutate(item)}
                >
                  {install.isPending && install.variables?.slug === item.slug && (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  )}
                  {item.installed ? t("pages.mcpLibrary.installed") : t("pages.mcpLibrary.install")}
                </Button>
              </div>
            </article>
          ))}
        </div>
      )}
    </PageBody>
  );
}

export function ToolGroups() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const groups = useQuery({
    queryKey: ["mcp-tool-groups", orgId],
    queryFn: () => fetchMcpToolGroups(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!groups.isLoading);
  useErrorState(!!groups.error, "tool-groups");
  const servers = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const [editing, setEditing] = React.useState<McpToolGroupRow | null | undefined>(undefined);
  const save = useMutation({
    mutationFn: ({
      initial,
      input,
    }: {
      initial: McpToolGroupRow | null;
      input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at">;
    }) =>
      initial ? updateMcpToolGroup(initial.id, input) : createMcpToolGroup(orgId as string, input),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["mcp-tool-groups", orgId] });
      setEditing(undefined);
    },
  });
  const remove = useMutation({
    mutationFn: deleteMcpToolGroup,
    onSuccess: () => void client.invalidateQueries({ queryKey: ["mcp-tool-groups", orgId] }),
  });
  const serverName = (id: string) =>
    servers.data?.find((server) => server.id === id)?.name ?? id.slice(0, 8);
  // was a bare window.confirm that could not name the group it was about (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<McpToolGroupRow | null>(null);
  const startDelete = (group: McpToolGroupRow) => {
    remove.reset();
    setDeleteTarget(group);
  };
  return (
    <PageBody>
      <PageLead
        eyebrow={t("pages.tool-groups.eyebrow")}
        action={
          <Button disabled={!servers.data?.length} onClick={() => setEditing(null)}>
            <Plus className="h-4 w-4" aria-hidden />
            {t("pages.tool-groups.create")}
          </Button>
        }
      >
        {t("pages.tool-groups.lead")}
      </PageLead>
      {groups.isLoading || servers.isLoading ? (
        <CardGridSkeleton cards={3} height={190} min={300} />
      ) : groups.error ? (
        <LoadError
          error={groups.error}
          resource={t("errors.resources.toolGroups")}
          onRetry={() => void groups.refetch()}
        />
      ) : !groups.data?.length ? (
        <EmptyState
          uxTarget="tool-groups"
          icon={<Puzzle />}
          title={t("pages.tool-groups.emptyTitle")}
          description={
            servers.data?.length
              ? t("pages.tool-groups.emptyBody")
              : t("pages.tool-groups.emptyNoServers")
          }
          actions={
            servers.data?.length ? (
              <Button onClick={() => setEditing(null)}>{t("pages.tool-groups.create")}</Button>
            ) : undefined
          }
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {groups.data.map((group) => (
            <article
              key={group.id}
              className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2">
                <Puzzle className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden />
                <h2 className="font-semibold">{group.name}</h2>
                <Badge className="ml-auto" tone={group.enabled ? "success" : "neutral"} dot>
                  {group.enabled ? t("pages.tool-groups.enabled") : t("pages.tool-groups.paused")}
                </Badge>
              </div>
              <p className="mt-2 min-h-10 text-xs leading-relaxed text-muted-foreground">
                {group.description || t("pages.mcpCatalog.noDescription")}
              </p>
              <div className="mt-3 flex flex-wrap gap-1.5">
                {group.tools.map((item) => (
                  <Badge key={`${item.server_id}/${item.tool}`} tone="outline">
                    {serverName(item.server_id)}/{item.tool}
                  </Badge>
                ))}
              </div>
              <div className="mt-4 flex justify-end gap-1 border-t border-[color:var(--border-subtle)] pt-3">
                <GatedButton
                  gate="mcp_tool_group:delete"
                  variant="ghost"
                  aria-label={t("pages.mcpCatalog.groups.deleteAria", { name: group.name })}
                  disabled={remove.isPending && remove.variables === group.id}
                  onClick={() => startDelete(group)}
                >
                  {remove.isPending && remove.variables === group.id && (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  )}
                  {t("common.delete")}
                </GatedButton>
                <GatedButton
                  gate="mcp_tool_group:update"
                  variant="outline"
                  aria-label={t("pages.mcpCatalog.groups.configureAria", { name: group.name })}
                  onClick={() => setEditing(group)}
                >
                  {t("pages.mcpCatalog.configure")}
                </GatedButton>
              </div>
            </article>
          ))}
        </div>
      )}
      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.tool-groups.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.tool-groups.confirm.body")}
        confirmLabel={t("common.delete")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() =>
          deleteTarget && remove.mutate(deleteTarget.id, { onSuccess: () => setDeleteTarget(null) })
        }
      />
      {editing !== undefined && (
        <ToolGroupDialog
          initial={editing}
          servers={servers.data ?? []}
          pending={save.isPending}
          error={save.error}
          onClose={() => setEditing(undefined)}
          onSave={(input) => save.mutate({ initial: editing, input })}
        />
      )}
    </PageBody>
  );
}

function ToolGroupDialog({
  initial,
  servers,
  pending,
  error,
  onClose,
  onSave,
}: {
  initial: McpToolGroupRow | null;
  servers: McpServerRow[];
  pending: boolean;
  error: Error | null;
  onClose: () => void;
  onSave: (input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at">) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = React.useState(initial?.name ?? "");
  const [description, setDescription] = React.useState(initial?.description ?? "");
  const [enabled, setEnabled] = React.useState(initial?.enabled ?? true);
  const [selected, setSelected] = React.useState<McpToolRef[]>(initial?.tools ?? []);
  const toggle = (item: McpToolRef) =>
    setSelected((current) =>
      current.some((tool) => tool.server_id === item.server_id && tool.tool === item.tool)
        ? current.filter((tool) => tool.server_id !== item.server_id || tool.tool !== item.tool)
        : [...current, item],
    );
  return (
    <Dialog open onClose={onClose}>
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.tool-groups.dialog.titleEdit")
            : t("pages.tool-groups.dialog.titleAdd")}
        </DialogTitle>
        <DialogDescription>{t("pages.tool-groups.dialog.lead")}</DialogDescription>
      </DialogHeader>
      <div className="grid gap-4 py-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pages.mcpCatalog.fields.name")} htmlFor="group-name">
            <Input id="group-name" value={name} onChange={(event) => setName(event.target.value)} />
          </Field>
          <label className="mt-auto flex min-h-9 items-center justify-between rounded-md border border-[color:var(--border-default)] px-3 text-sm">
            <span id="mcp-group-enabled-label">{t("pages.tool-groups.enabledLabel")}</span>
            <Switch
              checked={enabled}
              aria-labelledby="mcp-group-enabled-label"
              onCheckedChange={setEnabled}
            />
          </label>
        </div>
        <Field label={t("pages.mcpCatalog.fields.description")} htmlFor="group-description">
          <Textarea
            id="group-description"
            rows={2}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
        </Field>
        <fieldset>
          <legend className="text-sm font-medium">{t("pages.mcpCatalog.fields.tools")}</legend>
          <div className="mt-2 max-h-64 space-y-3 overflow-y-auto rounded-[10px] border border-[color:var(--border-default)] p-3">
            {servers.map((server) => (
              <div key={server.id}>
                <p className="mb-2 font-mono text-xs text-muted-foreground">{server.name}</p>
                <div className="flex flex-wrap gap-2">
                  {server.tools.map((tool) => {
                    const active = selected.some(
                      (item) => item.server_id === server.id && item.tool === tool,
                    );
                    return (
                      <Button
                        key={tool}
                        type="button"
                        variant={active ? "default" : "outline"}
                        aria-pressed={active}
                        onClick={() => toggle({ server_id: server.id, tool })}
                      >
                        {tool}
                      </Button>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        </fieldset>
        {error && (
          <p role="alert" className="text-sm text-[color:var(--danger-text)]">
            {error.message}
          </p>
        )}
      </div>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          disabled={!name.trim() || !selected.length || pending}
          onClick={() =>
            onSave({
              name,
              slug: initial?.slug ?? slugify(name),
              description,
              enabled,
              tools: selected,
            })
          }
        >
          {pending ? t("common.saving") : t("pages.tool-groups.dialog.save")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

export function McpSettings() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const query = useQuery({
    queryKey: ["mcp-settings", orgId],
    queryFn: () => fetchMcpSettings(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  useErrorState(!!query.error, "mcp-settings");
  if (!orgId)
    return (
      <PageBody>
        <EmptyState
          uxTarget="mcp-settings-no-org"
          icon={<Settings2 />}
          title={t("pages.mcpSettings.noOrgTitle")}
          description={t("pages.mcpSettings.noOrgBody")}
        />
      </PageBody>
    );
  if (query.isLoading)
    return (
      <PageBody>
        <CardGridSkeleton cards={2} height={190} min={300} />
      </PageBody>
    );
  if (query.error)
    return (
      <PageBody>
        <LoadError
          error={query.error}
          resource={t("errors.resources.mcpSettings")}
          onRetry={() => void query.refetch()}
        />
      </PageBody>
    );
  return (
    <McpSettingsForm
      initial={query.data as McpGatewaySettingsRow}
      onSaved={() => void client.invalidateQueries({ queryKey: ["mcp-settings", orgId] })}
    />
  );
}

function McpSettingsForm({
  initial,
  onSaved,
}: {
  initial: McpGatewaySettingsRow;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const [form, setForm] = React.useState(initial);
  const set = (patch: Partial<McpGatewaySettingsRow>) =>
    setForm((current) => ({ ...current, ...patch }));
  const save = useMutation({
    mutationFn: () => updateMcpSettings(orgId as string, form),
    onSuccess: (next) => {
      setForm(next);
      onSaved();
    },
  });
  return (
    <PageBody>
      <PageLead eyebrow={t("pages.mcpSettings.eyebrow")}>{t("pages.mcpSettings.lead")}</PageLead>
      <div className="grid gap-4 lg:grid-cols-2">
        <section className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-5">
          <div className="flex items-center gap-2">
            <Settings2 className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden />
            <h2 className="font-semibold">{t("pages.mcpSettings.transportDefaults")}</h2>
          </div>
          <div className="mt-5 grid gap-4">
            <Field label={t("pages.mcpSettings.defaultTransport")} htmlFor="default-transport">
              <Combobox
                id="default-transport"
                value={form.default_transport}
                onChange={(default_transport) => set({ default_transport })}
                options={TRANSPORTS.map((transport) => ({ value: transport, label: transport }))}
              />
            </Field>
            <Field
              label={t("pages.mcpSettings.failureMode")}
              htmlFor="failure-mode"
              hint={t("pages.mcpSettings.failureModeHint")}
            >
              <Combobox
                id="failure-mode"
                value={form.default_failure_mode}
                onChange={(picked) =>
                  set({
                    default_failure_mode: picked as McpGatewaySettingsRow["default_failure_mode"],
                  })
                }
                options={[
                  { value: "fail_closed", label: t("pages.mcpSettings.failClosed") },
                  { value: "fail_open", label: t("pages.mcpSettings.failOpen") },
                ]}
              />
            </Field>
            <label className="flex items-start justify-between gap-4 rounded-[10px] border border-[color:var(--border-subtle)] p-3">
              <span>
                <span id="mcp-unlisted-tools-label" className="block text-sm font-medium">
                  {t("pages.mcpSettings.allowUnlisted")}
                </span>
                <span className="mt-1 block text-xs leading-relaxed text-muted-foreground">
                  {t("pages.mcpSettings.allowUnlistedHint")}
                </span>
              </span>
              <Switch
                checked={form.allow_unlisted_tools}
                aria-labelledby="mcp-unlisted-tools-label"
                onCheckedChange={(value) => set({ allow_unlisted_tools: value })}
              />
            </label>
          </div>
        </section>
        <section className="rounded-[10px] border border-[color:var(--border-default)] bg-card p-5">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-4 w-4 text-[color:var(--red-folk-text)]" aria-hidden />
            <h2 className="font-semibold">{t("pages.mcpSettings.requestControls")}</h2>
          </div>
          <div className="mt-5 grid gap-4 sm:grid-cols-2">
            <Field label={t("pages.mcpSettings.connectTimeout")} htmlFor="connect-timeout">
              <Input
                id="connect-timeout"
                type="number"
                min={100}
                max={60000}
                value={form.connect_timeout_ms}
                onChange={(event) => set({ connect_timeout_ms: Number(event.target.value) })}
              />
            </Field>
            <Field label={t("pages.mcpSettings.requestTimeout")} htmlFor="request-timeout">
              <Input
                id="request-timeout"
                type="number"
                min={1000}
                max={300000}
                value={form.request_timeout_ms}
                onChange={(event) => set({ request_timeout_ms: Number(event.target.value) })}
              />
            </Field>
            <Field label={t("pages.mcpSettings.maxRetries")} htmlFor="max-retries">
              <Input
                id="max-retries"
                type="number"
                min={0}
                max={5}
                value={form.max_retries}
                onChange={(event) => set({ max_retries: Number(event.target.value) })}
              />
            </Field>
          </div>
          <p className="mt-5 flex items-start gap-2 rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-3 text-xs leading-relaxed text-muted-foreground">
            <Wrench className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />
            {t("pages.mcpSettings.persistNote")}
          </p>
        </section>
      </div>
      {save.error && (
        <p role="alert" className="text-sm text-[color:var(--danger-text)]">
          {save.error.message}
        </p>
      )}
      <div className="flex justify-end">
        <Button disabled={save.isPending} onClick={() => save.mutate()}>
          {save.isPending ? t("common.saving") : t("pages.mcpSettings.save")}
        </Button>
      </div>
    </PageBody>
  );
}
