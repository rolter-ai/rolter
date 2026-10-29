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
  ScrollText,
  Server,
  Settings2,
  Wrench,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { ConnectionHealth, ConnectionTestResult, ToolsSheet } from "@/components/McpServerProbe";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { ChipGroup } from "@/components/ui/chip-group";
import { Combobox } from "@/components/ui/combobox";
import {
  Dialog as BaseDialog,
  type DialogProps,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { FormSection } from "@/components/ui/form-section";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { SwitchRow } from "@/components/ui/switch-row";
import { Textarea } from "@/components/ui/textarea";
import {
  ApiError,
  createMcpServer,
  createMcpToolGroup,
  deleteMcpServer,
  deleteMcpToolGroup,
  discoverMcpServerTools,
  fetchMcpGrants,
  fetchMcpLibrary,
  fetchMcpOAuthClient,
  fetchMcpServers,
  fetchMcpSessions,
  fetchMcpSettings,
  fetchMcpToolGroupAssignments,
  fetchMcpToolGroups,
  fetchOrgProjects,
  fetchOrgVirtualKeys,
  fetchTeams,
  replaceMcpToolGroupAssignments,
  MCP_AUTH_KINDS,
  MCP_OAUTH_DISCOVERY_MODES,
  setMcpOAuthClient,
  setMcpServerAuth,
  testMcpServerConnection,
  updateMcpServer,
  updateMcpSettings,
  updateMcpToolGroup,
  type McpGatewaySettingsRow,
  type McpAuthKind,
  type McpConnectionTest,
  type McpLibraryItem,
  type McpOAuthClientInput,
  type McpOAuthDiscovery,
  type McpServerAuthInput,
  type McpServerInput,
  type McpServerRow,
  type McpAssignmentSubject,
  type McpToolGroupAssignmentRow,
  type McpToolGroupRow,
  type McpToolRef,
  type McpTransportOverridesPatch,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useOptionalAuth } from "@/lib/auth";
import { useCan } from "@/lib/can";
import { ServerStepUp } from "@/components/McpStepUp";
import { discoveryReasonKey, discoveryRefetchInterval, discoveryView } from "@/lib/mcp-discovery";
import { useFormat } from "@/lib/i18n/format";
import {
  serverConnections,
  useConsentAnnouncements,
  type ServerConnection,
} from "@/lib/mcp-consent";
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
import { slugify } from "@/lib/slug";
import { useMcpConnect } from "@/lib/use-mcp-connect";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

const TRANSPORTS = ["streamable_http", "sse", "websocket"];
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
  ...form
}: {
  open?: boolean;
  onClose: () => void;
  children: React.ReactNode;
} & Pick<DialogProps, "onSubmit" | "canSubmit" | "busy">) {
  return (
    <BaseDialog open onOpenChange={(open) => !open && onClose()} {...form}>
      {children}
    </BaseDialog>
  );
}

function PageLead({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-4 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end">
      <div className="min-w-0 flex-1">
        <div className="text-sm leading-relaxed text-muted-foreground">{children}</div>
      </div>
      {action}
    </div>
  );
}

function ToolBadges({ tools, stale = [] }: { tools: string[]; stale?: string[] }) {
  const { t } = useTranslation();
  if (!tools.length)
    return (
      <span className="text-xs text-[color:var(--text-subtle)]">
        {t("pages.mcpCatalog.noTools")}
      </span>
    );
  // the last discovery no longer returned these, but they are kept so a tool
  // group naming one keeps working. the warning tone is the only visual cue,
  // so the explanation is also text a screen reader gets
  const gone = new Set(stale);
  return (
    <div className="flex flex-wrap gap-1.5">
      {tools.map((tool) =>
        gone.has(tool) ? (
          <Badge key={tool} tone="warning" title={t("pages.mcpCatalog.discovery.staleTitle")}>
            {tool}
            <span className="sr-only"> {t("pages.mcpCatalog.discovery.staleTitle")}</span>
          </Badge>
        ) : (
          <Badge key={tool} tone="outline">
            {tool}
          </Badge>
        ),
      )}
    </div>
  );
}

// what tools/list discovery has said about one server. the three states a
// manifest can be in read differently on purpose: "declares no tools" is an
// answer, "not discovered yet" is the absence of one
function DiscoveryLine({ server }: { server: McpServerRow }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const view = discoveryView(server);
  const reasonKey = discoveryReasonKey(server.tools_discovery_error);
  const reason = reasonKey ? t(reasonKey) : null;
  const last = server.tools_discovered_at
    ? t("pages.mcpCatalog.discovery.lastSuccess", {
        when: fmt.dateTime(server.tools_discovered_at),
      })
    : null;
  const text =
    view === "failed"
      ? t("pages.mcpCatalog.discovery.failed", {
          reason: reason ?? t("pages.mcpCatalog.discovery.reasons.unknown"),
        })
      : view === "pending"
        ? server.tools_discovery_attempted_at === null
          ? t("pages.mcpCatalog.discovery.running")
          : reason
            ? t("pages.mcpCatalog.discovery.pendingWhy", { reason })
            : t("pages.mcpCatalog.discovery.pending")
        : view === "declaresNone"
          ? t("pages.mcpCatalog.discovery.declaresNone", {
              when: fmt.dateTime(server.tools_discovered_at ?? ""),
            })
          : t("pages.mcpCatalog.discovery.discovered", {
              when: fmt.dateTime(server.tools_discovered_at ?? ""),
            });
  const warn = view === "failed";
  return (
    <p
      id={`mcp-discovery-${server.id}`}
      className={`mt-2 text-xs ${warn ? "text-[color:var(--status-warning-text)]" : "text-muted-foreground"}`}
    >
      {text}
      {warn && last ? ` ${last}` : null}
    </p>
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
              className={`flex cursor-pointer items-start gap-2.5 rounded-lg border p-3 transition-colors focus-within:ring-1 focus-within:ring-ring ${selected ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)]" : "border-[color:var(--border-default)] hover:bg-card/60"}`}
            >
              <input
                type="radio"
                name="mcp-oauth-discovery"
                value={mode}
                checked={selected}
                onChange={() => onChange(mode)}
                className="mt-0.5 accent-[color:var(--red-500)]"
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
      className="rounded-lg border border-dashed border-[color:var(--border-default)] bg-card p-3"
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
          <span className="ml-auto font-mono text-2xs text-muted-foreground">
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
    <section className="rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-4">
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

// the request and the pop-up handling are shared with Reconnect on Auth Sessions.
// the reason a disabled Connect is disabled is text on the card, not a tooltip,
// and the button points at it
function ConnectButton({
  server,
  expired,
  describedBy,
}: {
  server: McpServerRow;
  expired: boolean;
  describedBy?: string;
}) {
  const { t } = useTranslation();
  const ready = oauthConnectable(server);
  const connect = useMcpConnect(server);
  return (
    <Button
      variant="outline"
      disabled={!ready || connect.isPending}
      aria-describedby={ready ? undefined : describedBy}
      aria-label={
        expired
          ? t("pages.mcpOAuth.reconnect.action", { server: server.name })
          : t("pages.mcpCatalog.connect.ready", { name: server.name })
      }
      onClick={() => connect.mutate()}
    >
      {connect.isPending ? (
        <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
      ) : (
        <Link2 className="mr-2 h-4 w-4" aria-hidden />
      )}
      {t(expired ? "pages.mcpCatalog.connect.reconnect" : "pages.mcpCatalog.connect.action")}
    </Button>
  );
}

// where the caller stands with an oauth server, in words: a missing client is
// said first, since no session can exist without one. `connection` is
// undefined when the sessions listing is out of reach, and then the card says
// nothing rather than "not connected"
function ConnectionLine({
  server,
  connection,
  known,
  id,
}: {
  server: McpServerRow;
  connection: ServerConnection | undefined;
  known: boolean;
  id: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (!oauthConnectable(server))
    return (
      <p id={id} className="mt-3 text-xs text-[color:var(--status-warning-text)]">
        {t("pages.mcpCatalog.connect.unconfigured")}
      </p>
    );
  if (!known) return null;
  const [text, tone] = !connection
    ? [t("pages.mcpCatalog.connect.none"), "text-muted-foreground"]
    : connection.state === "active"
      ? [
          t("pages.mcpCatalog.connect.activeUntil", { when: fmt.dateTime(connection.until) }),
          "text-[color:var(--status-success-text)]",
        ]
      : [
          t("pages.mcpCatalog.connect.expiredAt", { when: fmt.dateTime(connection.since) }),
          "text-[color:var(--status-warning-text)]",
        ];
  return (
    <p id={id} className={`mt-3 text-xs ${tone}`}>
      {text}
    </p>
  );
}

export function McpCatalog() {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const client = useQueryClient();
  const toast = useToast();
  const query = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled: !!orgId,
    retry: false,
    // the control plane discovers in the background after an install; keep
    // reading until every row has an attempt so the result shows without a reload
    refetchInterval: (q) => discoveryRefetchInterval(q.state.data),
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);

  // which servers the signed-in account is connected to (#2166). both reads
  // are best-effort: a caller who may not list them sees the cards without
  // the mark rather than an error, since the registry is what this screen is
  const auth = useOptionalAuth();
  const grants = useQuery({
    queryKey: ["mcp-grants", orgId],
    queryFn: () => fetchMcpGrants(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const sessions = useQuery({
    queryKey: ["mcp-sessions", orgId],
    queryFn: () => fetchMcpSessions(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const connections = React.useMemo(
    () => serverConnections(grants.data, sessions.data, auth?.user?.id, Date.now()),
    [grants.data, sessions.data, auth?.user?.id],
  );
  // "not connected" is only a claim when both listings answered for a known account
  const connectionKnown = !!auth?.user?.id && grants.isSuccess && sessions.isSuccess;
  // Connect finishes in the tab the authorization server sends the user back
  // to, never in this one. that tab announces the new session, and the cards
  // re-read who is connected rather than waiting for a reload
  useConsentAnnouncements(() => {
    void client.invalidateQueries({ queryKey: ["mcp-grants", orgId] });
    void client.invalidateQueries({ queryKey: ["mcp-sessions", orgId] });
  });
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
    onSuccess: (server, { initial }) => {
      setEditing(undefined);
      toast.push(
        initial
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: server.name }),
            }
          : { tone: "success", title: t("toast.created", { what: server.name }) },
      );
    },
    // the dialog keeps its own inline reason; the toast is what outlives it
    onError: (error, { draft }) =>
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: draft.input.name }),
        detail: errorText(error, t),
      }),
  });
  const remove = useMutation({
    mutationFn: (server: McpServerRow) => deleteMcpServer(server.id),
    onSuccess: (_done, server) => {
      void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] });
      setDeleting(null);
      toast.push({ tone: "success", title: t("toast.deleted", { what: server.name }) });
    },
    onError: (error, server) =>
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: server.name }),
        detail: errorText(error, t),
      }),
  });
  const toggle = useMutation({
    mutationFn: (server: McpServerRow) =>
      updateMcpServer(server.id, { ...serverInput(server), enabled: !server.enabled }),
    onSuccess: (_row, server) => {
      void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] });
      toast.push({
        tone: "success",
        title: t(
          server.enabled ? "pages.mcpCatalog.toggle.disabled" : "pages.mcpCatalog.toggle.enabled",
          { name: server.name },
        ),
      });
    },
    // a switch that bounced back reads as nothing having happened (#1197)
    onError: (error, server) =>
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: server.name }),
        detail: errorText(error, t),
      }),
  });
  // an error code from the server still answers 200 with the row carrying it,
  // so only a failed request lands in onError; the card shows the new state
  const discover = useMutation({
    mutationFn: (server: McpServerRow) => discoverMcpServerTools(server.id),
    onSettled: () => void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }),
    onError: (error, server) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpCatalog.discovery.failedToast", { name: server.name }),
        detail: errorText(error, t),
      }),
  });
  // a connection test answers 200 whatever it found, so only a failed request
  // lands in onError; the result is kept per card until the next test replaces it
  const [tested, setTested] = React.useState<Record<string, McpConnectionTest>>({});
  const testConnection = useMutation({
    mutationFn: (server: McpServerRow) => testMcpServerConnection(server.id),
    onSuccess: (result, server) => setTested((prev) => ({ ...prev, [server.id]: result })),
    onSettled: () => void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] }),
    onError: (error, server) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpCatalog.test.failedToast", { name: server.name }),
        detail: errorText(error, t),
      }),
  });
  const testing = testConnection.isPending ? testConnection.variables?.id : undefined;
  // the tools panel follows the list, so a discovery that lands behind it shows
  const [toolsFor, setToolsFor] = React.useState<string | null>(null);
  const toolsServer = query.data?.find((server) => server.id === toolsFor);
  const discovering = discover.isPending ? discover.variables?.id : undefined;
  // only the card whose request is in flight locks; the others stay live
  const toggling = toggle.isPending ? toggle.variables?.id : undefined;
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
        action={
          <GatedButton
            gate="mcp_server:create"
            control="mcp-server-new"
            onClick={() => setEditing(null)}
          >
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
          target="mcp-servers"
        />
      ) : !query.data?.length ? (
        <EmptyState
          thread
          uxTarget="mcp-servers"
          icon={<Server />}
          title={t("pages.mcpCatalog.emptyTitle")}
          description={t("pages.mcpCatalog.emptyBody")}
          actions={
            <GatedButton
              gate="mcp_server:create"
              control="mcp-server-new-empty"
              onClick={() => setEditing(null)}
            >
              {t("pages.mcpCatalog.registerServer")}
            </GatedButton>
          }
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {query.data.map((server) => (
            <article
              key={server.id}
              className={`min-w-0 rounded-lg border border-[color:var(--border-default)] p-4 ${server.enabled ? "bg-card" : "bg-[color:var(--surface-subtle)]/60"}`}
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
                  control="mcp-server-toggle"
                  checked={server.enabled}
                  disabled={toggling === server.id}
                  aria-label={t("pages.mcpCatalog.servers.toggleAria", { name: server.name })}
                  onCheckedChange={() => toggle.mutate(server)}
                />
              </div>
              <p className="mt-3 min-h-10 text-xs leading-relaxed text-muted-foreground">
                {server.description || t("pages.mcpCatalog.noDescription")}
              </p>
              <div className="mt-3">
                <ToolBadges tools={server.tools} stale={server.tools_stale} />
                {server.tools_stale.length > 0 && (
                  <p className="mt-2 text-xs text-[color:var(--status-warning-text)]">
                    {t("pages.mcpCatalog.discovery.staleHint", {
                      count: server.tools_stale.length,
                    })}
                  </p>
                )}
                <DiscoveryLine server={server} />
              </div>
              <ConnectionHealth server={server} />
              {tested[server.id] && (
                <ConnectionTestResult server={server} result={tested[server.id]} />
              )}
              {server.auth_kind === "oauth" && (
                <ConnectionLine
                  id={`mcp-connection-${server.id}`}
                  server={server}
                  connection={connections.get(server.id)}
                  known={connectionKnown}
                />
              )}
              {server.auth_kind === "oauth" && (
                <ServerStepUp
                  server={server}
                  grants={grants.data}
                  sessions={sessions.data}
                  userId={auth?.user?.id}
                />
              )}
              <div className="mt-4 flex flex-wrap items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <Badge tone="info">{server.transport.replace("_", " ")}</Badge>
                <AuthBadge server={server} />
                <span className="ml-auto flex flex-wrap justify-end gap-1">
                  {server.auth_kind === "oauth" && (
                    <ConnectButton
                      server={server}
                      expired={connections.get(server.id)?.state === "expired"}
                      describedBy={`mcp-connection-${server.id}`}
                    />
                  )}
                  <GatedButton
                    gate="mcp_server:update"
                    control="mcp-server-discover"
                    variant="outline"
                    disabled={discovering === server.id}
                    aria-describedby={`mcp-discovery-${server.id}`}
                    aria-label={t("pages.mcpCatalog.discovery.discoverAria", { name: server.name })}
                    onClick={() => discover.mutate(server)}
                  >
                    {discovering === server.id && (
                      <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
                    )}
                    {discovering === server.id
                      ? t("pages.mcpCatalog.discovery.discovering")
                      : t("pages.mcpCatalog.discovery.discover")}
                  </GatedButton>
                  <GatedButton
                    gate="mcp_server:update"
                    control="mcp-server-test"
                    variant="outline"
                    disabled={testing === server.id}
                    aria-label={t("pages.mcpCatalog.test.aria", { name: server.name })}
                    onClick={() => testConnection.mutate(server)}
                  >
                    {testing === server.id && (
                      <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
                    )}
                    {testing === server.id
                      ? t("pages.mcpCatalog.test.running")
                      : t("pages.mcpCatalog.test.action")}
                  </GatedButton>
                  <Button
                    variant="outline"
                    aria-label={t("pages.mcpCatalog.tools.openAria", { name: server.name })}
                    onClick={() => setToolsFor(server.id)}
                  >
                    <Wrench className="mr-2 h-4 w-4" aria-hidden />
                    {t("pages.mcpCatalog.tools.open")}
                  </Button>
                  <Link
                    to={`/mcp-logs?server=${encodeURIComponent(server.slug)}`}
                    aria-label={t("pages.mcpCatalog.logs.aria", { name: server.name })}
                    className={buttonVariants({ variant: "ghost" })}
                  >
                    <ScrollText className="mr-2 h-4 w-4" aria-hidden />
                    {t("pages.mcpCatalog.logs.link")}
                  </Link>
                  <GatedButton
                    gate="mcp_server:delete"
                    control="mcp-server-delete"
                    variant="ghost"
                    aria-label={t("pages.mcpCatalog.servers.deleteAria", { name: server.name })}
                    onClick={() => setDeleting(server)}
                  >
                    {t("common.delete")}
                  </GatedButton>
                  <GatedButton
                    gate="mcp_server:update"
                    control="mcp-server-configure"
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
        <ServerSheet
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
      {toolsServer && <ToolsSheet server={toolsServer} onClose={() => setToolsFor(null)} />}
      {/* mounted whether or not a server is picked: the dialog reads how the
          delete ended on its own closing edge, which an unmount never has */}
      <ConfirmDialog
        name="mcp-server-delete"
        open={!!deleting}
        onOpenChange={(open) => {
          if (open) return;
          setDeleting(null);
          // a refusal for this server must not greet the next one opened
          remove.reset();
        }}
        title={t("pages.mcpCatalog.confirm.deleteTitle", { name: deleting?.name ?? "" })}
        description={t("pages.mcpCatalog.confirm.deleteBody")}
        confirmLabel={t("pages.mcpCatalog.confirm.deleteConfirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => deleting && remove.mutate(deleting)}
      />
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

type SectionKey = "endpoint" | "tools" | "auth" | "overrides";

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
          : armed || (server.auth_kind === "oauth" && oauthConnectable(server))
            ? "success"
            : "warning"
      }
    >
      {server.auth_kind === "oauth" && !oauthConnectable(server)
        ? t("pages.mcpCatalog.auth.kinds.oauth.badgeNoClient")
        : t(`pages.mcpCatalog.auth.kinds.${server.auth_kind}.badge`)}
    </Badge>
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
              className={`flex cursor-pointer items-start gap-2.5 rounded-lg border p-3 transition-colors focus-within:ring-1 focus-within:ring-ring ${selected ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)]" : "border-[color:var(--border-default)] hover:bg-[color:var(--surface-subtle)]/60"}`}
            >
              <input
                type="radio"
                name="mcp-auth-kind"
                value={kind}
                checked={selected}
                onChange={() => onChange(kind)}
                className="mt-0.5 accent-[color:var(--red-500)]"
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
            control="mcp-server-credential-clear"
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
    <div className="grid gap-3">
      <p className="text-xs leading-relaxed text-muted-foreground">
        {t("pages.mcpCatalog.overrides.lead")}
      </p>
      <div className="grid gap-3 sm:grid-cols-3">
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
    </div>
  );
}

function ServerSheet({
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
  // the sections fold by hand; a section the operator never touched opens when
  // it holds something already (an override) or something wrong (a bad one)
  const [folded, setFolded] = React.useState<Partial<Record<SectionKey, boolean>>>({});
  const overridesSet = Object.values(overrideDraft(initial)).some(Boolean);
  const snapshot = JSON.stringify({ form, tools, scopes, oauth, auth, overrides });
  const baseline = React.useRef(snapshot);
  const dirty = snapshot !== baseline.current;

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

  const kek = !!error && isKekMissing(error);
  const section = (key: SectionKey, fallback: boolean) => ({
    open: folded[key] ?? fallback,
    onToggle: () => setFolded((current) => ({ ...current, [key]: !(current[key] ?? fallback) })),
    className: "grid gap-3.5",
  });

  return (
    <>
      {/* hidden, not unmounted, while a confirmation is up: the drafts live in
          this component, so a cancelled confirmation lands back on the same form */}
      <EditorSheet
        name="mcp-server"
        open
        suspended={confirming !== null}
        onOpenChange={(open) => !open && onClose()}
        title={
          initial ? t("pages.mcpCatalog.dialog.titleEdit") : t("pages.mcpCatalog.dialog.titleAdd")
        }
        subtitle={t("pages.mcpCatalog.dialog.lead")}
        dirty={dirty}
        errorMessage={
          error ? (kek ? t("pages.mcpCatalog.auth.kekTitle") : errorText(error, t)) : undefined
        }
        errorHint={kek ? t("pages.mcpCatalog.auth.kekBody") : undefined}
        errorDetail={kek ? error?.message : undefined}
        saveLabel={
          pending
            ? t("common.saving")
            : initial
              ? t("pages.mcpCatalog.dialog.saveServer")
              : t("pages.mcpCatalog.registerServer")
        }
        canSave={!!valid}
        saving={pending}
        onSubmit={submit}
      >
        <FormSection title={t("pages.mcpCatalog.sections.endpoint")} {...section("endpoint", true)}>
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
              <SwitchRow
                title={t("pages.mcpCatalog.fields.enabledInGateway")}
                checked={form.enabled}
                onChange={(enabled) => setForm({ ...form, enabled })}
              />
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
        </FormSection>
        <FormSection title={t("pages.mcpCatalog.sections.tools")} {...section("tools", true)}>
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
        </FormSection>
        <FormSection title={t("pages.mcpCatalog.auth.title")} {...section("auth", true)}>
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("pages.mcpCatalog.auth.lead")}
          </p>
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
          <Field
            label={t("pages.mcpCatalog.fields.scopes")}
            htmlFor="mcp-scopes"
            hint={t("pages.mcpCatalog.fields.scopesHint")}
          >
            <Textarea
              id="mcp-scopes"
              rows={3}
              value={scopes}
              onChange={(event) => setScopes(event.target.value)}
            />
          </Field>
        </FormSection>
        <FormSection
          title={t("pages.mcpCatalog.overrides.title")}
          {...section("overrides", overridesSet || !overridesValid(overrides))}
        >
          <OverridesSection
            draft={overrides}
            onChange={(patch) => setOverrides((current) => ({ ...current, ...patch }))}
          />
        </FormSection>
      </EditorSheet>
      <ConfirmDialog
        name="mcp-credential-clear"
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
  const toast = useToast();
  const query = useQuery({
    queryKey: ["mcp-library", orgId],
    queryFn: () => fetchMcpLibrary(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!query.isLoading);
  const install = useMutation({
    mutationFn: (item: McpLibraryItem) =>
      createMcpServer(orgId as string, { ...item, enabled: true, source: "library" }),
    onSuccess: (_server, item) => {
      void client.invalidateQueries({ queryKey: ["mcp-library", orgId] });
      void client.invalidateQueries({ queryKey: ["mcp-servers", orgId] });
      // an install registers the definition and nothing else: the next step is
      // Configure on the catalog, so the toast says where
      toast.push({
        tone: "success",
        title: t("pages.mcpLibrary.installedToast", { name: item.name }),
        detail: t("pages.mcpLibrary.nextStep"),
      });
    },
    // a refused install used to leave the button as it was and say nothing
    onError: (error, item) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpLibrary.installFailed", { name: item.name }),
        detail: errorText(error, t),
      }),
  });
  if (!orgId)
    return (
      <PageBody>
        <EmptyState
          uxTarget="mcp-library-no-org"
          icon={<Boxes />}
          title={t("pages.mcpLibrary.noOrgTitle")}
          description={t("pages.mcpLibrary.noOrgBody")}
        />
      </PageBody>
    );
  return (
    <PageBody>
      <PageLead>{t("pages.mcpLibrary.lead")}</PageLead>
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
          target="mcp-library"
        />
      ) : !query.data?.length ? (
        // the curated list ships with the control plane, so an empty answer is
        // a build that carries none; there is nothing to create from here
        <EmptyState
          uxTarget="mcp-library"
          icon={<Boxes />}
          title={t("pages.mcpLibrary.emptyTitle")}
          description={t("pages.mcpLibrary.emptyBody")}
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2">
          {query.data.map((item) => (
            <article
              key={item.slug}
              className="rounded-lg border border-[color:var(--border-default)] bg-card p-5"
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
              <div className="mt-5 flex flex-wrap items-center gap-x-3 gap-y-2 border-t border-[color:var(--border-subtle)] pt-4">
                <span className="text-xs text-muted-foreground">
                  {item.required_scopes.length
                    ? t("pages.mcpLibrary.scopesCount", { count: item.required_scopes.length })
                    : t("pages.mcpLibrary.noScopes")}
                </span>
                {item.installed ? (
                  // a status, not a control: a disabled button read 2.7:1
                  <span className="ml-auto flex flex-wrap items-center justify-end gap-x-3 gap-y-1">
                    <Badge tone="success" dot>
                      {t("pages.mcpLibrary.installed")}
                    </Badge>
                    <Link
                      to="/mcp-catalog"
                      className="text-xs text-[color:var(--red-folk-text)] underline-offset-2 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    >
                      {t("pages.mcpLibrary.configureLink")}
                    </Link>
                  </span>
                ) : (
                  <GatedButton
                    gate="mcp_server:create"
                    control="mcp-library-install"
                    className="ml-auto"
                    aria-label={t("pages.mcpLibrary.installAria", { name: item.name })}
                    disabled={install.isPending}
                    onClick={() => install.mutate(item)}
                  >
                    {install.isPending && install.variables?.slug === item.slug && (
                      <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
                    )}
                    {t("pages.mcpLibrary.install")}
                  </GatedButton>
                )}
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
  const toast = useToast();
  const groups = useQuery({
    queryKey: ["mcp-tool-groups", orgId],
    queryFn: () => fetchMcpToolGroups(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider
  useScreenReady(!groups.isLoading);
  const servers = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  // who each group is assigned to (#3295). a failed read leaves the dialog
  // without an assignment editor rather than offering one that would replace
  // the stored set with an empty one
  const assignments = useQuery({
    queryKey: ["mcp-tool-group-assignments", orgId],
    queryFn: () => fetchMcpToolGroupAssignments(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const [editing, setEditing] = React.useState<McpToolGroupRow | null | undefined>(undefined);
  const save = useMutation({
    mutationFn: async ({
      initial,
      input,
      subjects,
    }: {
      initial: McpToolGroupRow | null;
      input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at">;
      subjects: McpAssignmentSubject[] | null;
    }) => {
      // a new group is created with its assignments in one request, so it is
      // never visible to a gateway as applying to the whole organization
      if (!initial) {
        return createMcpToolGroup(orgId as string, { ...input, assignments: subjects ?? [] });
      }
      const group = await updateMcpToolGroup(initial.id, input);
      if (subjects) await replaceMcpToolGroupAssignments(group.id, subjects);
      return group;
    },
    onSuccess: (group, { initial }) => {
      void client.invalidateQueries({ queryKey: ["mcp-tool-groups", orgId] });
      void client.invalidateQueries({ queryKey: ["mcp-tool-group-assignments", orgId] });
      setEditing(undefined);
      toast.push(
        initial
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: group.name }),
            }
          : { tone: "success", title: t("toast.created", { what: group.name }) },
      );
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
  if (!orgId)
    return (
      <PageBody>
        <EmptyState
          uxTarget="tool-groups-no-org"
          icon={<Puzzle />}
          title={t("pages.tool-groups.noOrgTitle")}
          description={t("pages.tool-groups.noOrgBody")}
        />
      </PageBody>
    );
  // a failed server read is not "no servers": the picker has nothing to offer
  // either way, but only one of them is fixed by registering a server
  const serversFailed = !!servers.error;
  const canPick = !!servers.data?.length;
  return (
    <PageBody>
      <PageLead
        action={
          <GatedButton
            gate="mcp_tool_group:create"
            control="tool-group-new"
            disabled={!canPick}
            onClick={() => setEditing(null)}
          >
            <Plus className="h-4 w-4" aria-hidden />
            {t("pages.tool-groups.create")}
          </GatedButton>
        }
      >
        {t("pages.tool-groups.lead")}
      </PageLead>
      {serversFailed && !servers.isLoading && (
        // load-error-allow: the server list only names the picker's rows and each card's tools
        <LoadError
          error={servers.error}
          resource={t("errors.resources.mcpServers")}
          onRetry={() => void servers.refetch()}
          target="tool-groups-servers"
        />
      )}
      {groups.isLoading || servers.isLoading ? (
        <CardGridSkeleton cards={3} height={190} min={300} />
      ) : groups.error ? (
        <LoadError
          error={groups.error}
          resource={t("errors.resources.toolGroups")}
          onRetry={() => void groups.refetch()}
          target="tool-groups"
        />
      ) : !groups.data?.length ? (
        <EmptyState
          thread
          uxTarget="tool-groups"
          icon={<Puzzle />}
          title={t("pages.tool-groups.emptyTitle")}
          description={
            serversFailed
              ? t("pages.tool-groups.emptyServersFailed")
              : canPick
                ? t("pages.tool-groups.emptyBody")
                : t("pages.tool-groups.emptyNoServers")
          }
          actions={
            canPick ? (
              <GatedButton
                gate="mcp_tool_group:create"
                control="tool-group-new-empty"
                onClick={() => setEditing(null)}
              >
                {t("pages.tool-groups.create")}
              </GatedButton>
            ) : undefined
          }
        />
      ) : (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {groups.data.map((group) => (
            <article
              key={group.id}
              className="rounded-lg border border-[color:var(--border-default)] bg-card p-4"
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
              {assignments.data && (
                <p className="mt-3 text-xs text-muted-foreground">
                  {assignments.data.some((row) => row.group_id === group.id)
                    ? t("pages.tool-groups.assign.narrowed")
                    : t("pages.tool-groups.assign.everyone")}
                </p>
              )}
              <div className="mt-3 flex flex-wrap gap-1.5">
                {group.tools.map((item) => (
                  <Badge key={`${item.server_id}/${item.tool}`} tone="outline">
                    {serverName(item.server_id)}/{item.tool}
                  </Badge>
                ))}
              </div>
              <div className="mt-4 flex flex-wrap justify-end gap-1 border-t border-[color:var(--border-subtle)] pt-3">
                <GatedButton
                  gate="mcp_tool_group:delete"
                  control="tool-group-delete"
                  variant="ghost"
                  aria-label={t("pages.mcpCatalog.groups.deleteAria", { name: group.name })}
                  onClick={() => startDelete(group)}
                >
                  {t("common.delete")}
                </GatedButton>
                <GatedButton
                  gate="mcp_tool_group:update"
                  control="tool-group-configure"
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
        name="tool-group-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.tool-groups.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.tool-groups.confirm.body")}
        confirmLabel={t("common.delete")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() =>
          deleteTarget &&
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              toast.push({
                tone: "success",
                title: t("toast.deleted", { what: deleteTarget.name }),
              });
              setDeleteTarget(null);
            },
          })
        }
      />
      {editing !== undefined && (
        <ToolGroupDialog
          initial={editing}
          servers={servers.data ?? []}
          assignments={assignments.data}
          assignmentsStatus={assignments.status}
          pending={save.isPending}
          error={save.error}
          onClose={() => setEditing(undefined)}
          onSave={(input, subjects) => save.mutate({ initial: editing, input, subjects })}
        />
      )}
    </PageBody>
  );
}

function ToolGroupDialog({
  initial,
  servers,
  assignments,
  assignmentsStatus,
  pending,
  error,
  onClose,
  onSave,
}: {
  initial: McpToolGroupRow | null;
  servers: McpServerRow[];
  /** every assignment of the org, and how reading them is going */
  assignments: McpToolGroupAssignmentRow[] | undefined;
  assignmentsStatus: "pending" | "error" | "success";
  pending: boolean;
  error: Error | null;
  onClose: () => void;
  onSave: (
    input: Omit<McpToolGroupRow, "id" | "org_id" | "created_at" | "updated_at">,
    /** the whole assignment set to store, or null to leave it alone */
    subjects: McpAssignmentSubject[] | null,
  ) => void;
}) {
  const { t } = useTranslation();
  // what the assignment editor last reported: the whole set to store, or null
  // while it is unchanged (or was never readable), which leaves the stored set
  // alone
  const [subjects, setSubjects] = React.useState<McpAssignmentSubject[] | null>(null);
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
    <Dialog
      open
      onClose={onClose}
      onSubmit={() =>
        onSave(
          {
            name,
            slug: initial?.slug ?? slugify(name),
            description,
            enabled,
            tools: selected,
          },
          subjects,
        )
      }
      canSubmit={!!name.trim() && !!selected.length}
      busy={pending}
    >
      <DialogHeader>
        <DialogTitle>
          {initial
            ? t("pages.tool-groups.dialog.titleEdit")
            : t("pages.tool-groups.dialog.titleAdd")}
        </DialogTitle>
        <DialogDescription>{t("pages.tool-groups.dialog.lead")}</DialogDescription>
      </DialogHeader>
      <DialogBody className="grid gap-4 py-4">
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pages.mcpCatalog.fields.name")} htmlFor="group-name" required>
            <Input id="group-name" value={name} onChange={(event) => setName(event.target.value)} />
          </Field>
          <SwitchRow
            title={t("pages.tool-groups.enabledLabel")}
            checked={enabled}
            onChange={setEnabled}
          />
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
          <div className="mt-2 max-h-64 space-y-3 overflow-y-auto rounded-lg border border-[color:var(--border-default)] p-3">
            {servers.map((server) => (
              <ChipGroup
                key={server.id}
                label={server.name}
                options={server.tools.map((tool) => ({ id: tool, name: tool }))}
                selected={selected
                  .filter((item) => item.server_id === server.id)
                  .map((i) => i.tool)}
                onToggle={(tool) => toggle({ server_id: server.id, tool })}
              />
            ))}
          </div>
        </fieldset>
        <fieldset>
          <legend className="text-sm font-medium">{t("pages.tool-groups.assign.title")}</legend>
          <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
            {t("pages.tool-groups.assign.hint")}
          </p>
          {!initial || (assignmentsStatus === "success" && assignments) ? (
            <AssignmentEditor
              rows={(assignments ?? []).filter((row) => row.group_id === initial?.id)}
              onChange={setSubjects}
            />
          ) : (
            <p className="mt-2 text-xs text-muted-foreground">
              {assignmentsStatus === "pending"
                ? t("pages.tool-groups.assign.loading")
                : t("pages.tool-groups.assign.unavailable")}
            </p>
          )}
        </fieldset>
        {error && (
          <p role="alert" className="text-sm text-[color:var(--danger-text)]">
            {errorText(error, t)}
          </p>
        )}
      </DialogBody>
      <DialogFooter>
        <Button variant="ghost" onClick={onClose}>
          {t("common.cancel")}
        </Button>
        <Button
          type="submit"
          disabled={pending}
          aria-disabled={!name.trim() || !selected.length || undefined}
          className="aria-disabled:opacity-50"
        >
          {pending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />}
          {pending ? t("common.saving") : t("pages.tool-groups.dialog.save")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

/**
 * The team and project chips of a tool group's assignments. Mounted only once
 * the stored assignments are known, so its state starts from them and never
 * from an empty set a slow read has not filled in yet (an empty set would widen
 * a narrowed group to the whole organization when saved). It reports the whole
 * set to store, or null while nothing changed.
 */
function AssignmentEditor({
  rows,
  onChange,
}: {
  rows: McpToolGroupAssignmentRow[];
  onChange: (subjects: McpAssignmentSubject[] | null) => void;
}) {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const ofKind = (kind: McpToolGroupAssignmentRow["subject_kind"]) =>
    rows.filter((row) => row.subject_kind === kind).map((row) => row.subject_id);
  const [stored] = React.useState(() => ({
    teams: ofKind("team"),
    projects: ofKind("project"),
    keys: ofKind("virtual_key"),
  }));
  const [teamIds, setTeamIds] = React.useState(stored.teams);
  const [projectIds, setProjectIds] = React.useState(stored.projects);
  const [keyIds, setKeyIds] = React.useState(stored.keys);
  const canAssign = useCan()("mcp_tool_group_assignment", "update") !== false;
  const teams = useQuery({
    queryKey: ["teams", orgId],
    queryFn: () => fetchTeams(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const projects = useQuery({
    queryKey: ["org-projects", orgId],
    queryFn: () => fetchOrgProjects(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  // one read for the organization's keys, each naming its project (#3467); the
  // picker groups them under the project names it already has
  const keys = useQuery({
    // under the `virtual-keys` prefix, so the Keys page invalidating a
    // project's keys refreshes this list too
    queryKey: ["virtual-keys", "org", orgId],
    queryFn: () => fetchOrgVirtualKeys(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const projectList = projects.data ?? [];
  const keysKnown = !!projects.data && !!keys.data;
  const keysReading = projects.isPending || keys.isPending;
  const keysRefused = keys.error instanceof ApiError && keys.error.status === 403;
  const keysUnread = projects.isError || (keys.isError && !keysRefused);
  const keyGroups = projectList
    .map((project) => ({
      project,
      keys: (keys.data ?? []).filter((key) => key.project_id === project.id),
    }))
    .filter((group) => group.keys.length > 0);
  const flip = (ids: string[], id: string) =>
    ids.includes(id) ? ids.filter((current) => current !== id) : [...ids, id];
  // an id that is no longer among the options (a deleted team) is dropped from
  // what is saved, once the options have been read
  const live = (ids: string[], options: { id: string }[] | undefined) =>
    options ? ids.filter((id) => options.some((option) => option.id === id)) : ids;
  const savedTeams = live(teamIds, teams.data);
  const savedProjects = live(projectIds, projects.data);
  // a key that is gone (a deleted one) is dropped only once the keys have been
  // read; until then, or when the read failed or was refused, the stored ones
  // stay, as they would if the picker did not exist
  const savedKeys = keysKnown ? live(keyIds, keys.data) : keyIds;
  const same = (a: string[], b: string[]) =>
    a.length === b.length && a.every((id) => b.includes(id));
  const changed =
    !same(savedTeams, stored.teams) ||
    !same(savedProjects, stored.projects) ||
    !same(savedKeys, stored.keys);
  const report = JSON.stringify([changed, savedTeams, savedProjects, savedKeys]);
  React.useEffect(() => {
    onChange(
      changed
        ? [
            ...savedTeams.map((id) => ({ kind: "team" as const, id })),
            ...savedProjects.map((id) => ({ kind: "project" as const, id })),
            ...savedKeys.map((id) => ({ kind: "virtual_key" as const, id })),
          ]
        : null,
    );
    // `report` stands for exactly the values read above
  }, [report]);
  return (
    <div className="mt-2 space-y-3">
      <ChipGroup
        label={t("pages.tool-groups.assign.teams")}
        options={(teams.data ?? []).map((team) => ({ id: team.id, name: team.name }))}
        selected={teamIds}
        onToggle={(id) => setTeamIds((current) => flip(current, id))}
        disabled={!canAssign}
      />
      <ChipGroup
        label={t("pages.tool-groups.assign.projects")}
        options={(projects.data ?? []).map((project) => ({
          id: project.id,
          name: `${project.team_name} / ${project.name}`,
        }))}
        selected={projectIds}
        onToggle={(id) => setProjectIds((current) => flip(current, id))}
        disabled={!canAssign}
      />
      {keyGroups.map(({ project, keys }) => (
        <ChipGroup
          key={project.id}
          label={t("pages.tool-groups.assign.keysOf", {
            project: `${project.team_name} / ${project.name}`,
          })}
          options={keys.map((key) => ({ id: key.id, name: key.name || key.key_prefix }))}
          selected={keyIds}
          onToggle={(id) => setKeyIds((current) => flip(current, id))}
          disabled={!canAssign}
        />
      ))}
      {keysReading && (
        <p className="text-xs text-muted-foreground">{t("pages.tool-groups.assign.keysLoading")}</p>
      )}
      {keysRefused && (
        <p role="alert" className="text-xs text-[color:var(--status-warning-text)]">
          {t("pages.tool-groups.assign.keysRefused")}
        </p>
      )}
      {keysUnread && (
        <p role="alert" className="text-xs text-[color:var(--status-warning-text)]">
          {t("pages.tool-groups.assign.keysUnavailable")}
        </p>
      )}
      {keysKnown && keyGroups.length === 0 && (
        <p className="text-xs text-muted-foreground">{t("pages.tool-groups.assign.keysNone")}</p>
      )}
    </div>
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
        {/* load-error-allow: a settings form, not a list */}
        <LoadError
          error={query.error}
          resource={t("errors.resources.mcpSettings")}
          onRetry={() => void query.refetch()}
          target="mcp-settings"
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
  const toast = useToast();
  const [form, setForm] = React.useState(initial);
  const set = (patch: Partial<McpGatewaySettingsRow>) =>
    setForm((current) => ({ ...current, ...patch }));
  // the fields go read-only for a caller the table refuses; the Save beside
  // them says which role it takes. an unanswered gate leaves them editable,
  // like every other control (docs/dev-docs/development/rbac-gating.md)
  const readOnly = useCan()("mcp_settings", "update") === false;
  const save = useMutation({
    mutationFn: () => updateMcpSettings(orgId as string, form),
    onSuccess: (next) => {
      setForm(next);
      onSaved();
      toast.push({ tone: "success", title: t("pages.mcpSettings.savedToast") });
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpSettings.saveFailed"),
        detail: errorText(error, t),
      }),
  });
  return (
    <PageBody>
      <PageLead>{t("pages.mcpSettings.lead")}</PageLead>
      <div className="grid gap-4 lg:grid-cols-2">
        <SettingsPanel
          title={t("pages.mcpSettings.transportDefaults")}
          dimmed={readOnly}
          className="bg-card"
        >
          <div className="grid w-full gap-4">
            <Field label={t("pages.mcpSettings.defaultTransport")} htmlFor="default-transport">
              <Combobox
                id="default-transport"
                value={form.default_transport}
                onChange={(default_transport) => set({ default_transport })}
                options={TRANSPORTS.map((transport) => ({ value: transport, label: transport }))}
              />
            </Field>
            {/* deprecated (#3527): the gateway refuses every body it cannot
                judge, so the saved value is kept as it is and offered as no choice */}
            <div role="group" aria-labelledby="failure-mode" className="grid gap-1.5">
              <FieldLabel id="failure-mode" label={t("pages.mcpSettings.failureMode")} />
              <div>
                <Badge>{t("pages.mcpSettings.failureModeNoEffect")}</Badge>
              </div>
              <p className="text-xs text-muted-foreground">
                {t("pages.mcpSettings.failureModeNoEffectReason")}
              </p>
            </div>
            <SwitchRow
              title={t("pages.mcpSettings.allowUnlisted")}
              hint={t("pages.mcpSettings.allowUnlistedHint")}
              checked={form.allow_unlisted_tools}
              onChange={(value) => set({ allow_unlisted_tools: value })}
            />
          </div>
        </SettingsPanel>
        <SettingsPanel
          title={t("pages.mcpSettings.requestControls")}
          dimmed={readOnly}
          className="bg-card"
        >
          <div className="grid w-full gap-4 sm:grid-cols-2">
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
          <p className="flex w-full items-start gap-2 rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-3 text-xs leading-relaxed text-muted-foreground">
            <Wrench className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />
            {t("pages.mcpSettings.persistNote")}
          </p>
        </SettingsPanel>
      </div>
      {save.error && (
        <p role="alert" className="text-sm text-[color:var(--danger-text)]">
          {errorText(save.error, t)}
        </p>
      )}
      <div className="flex justify-end">
        <GatedButton
          gate="mcp_settings:update"
          control="mcp-settings-save"
          disabled={save.isPending}
          onClick={() => save.mutate()}
        >
          {save.isPending && (
            <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
          )}
          {save.isPending ? t("common.saving") : t("pages.mcpSettings.save")}
        </GatedButton>
      </div>
    </PageBody>
  );
}
