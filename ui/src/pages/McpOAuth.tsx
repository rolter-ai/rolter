import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Building2,
  CircleAlert,
  CircleCheck,
  KeyRound,
  Loader2,
  RefreshCw,
  Shield,
  ShieldOff,
} from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link, useLocation } from "react-router";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import {
  ListActionsHeader,
  ListCell,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListTable,
  PageBody,
  Pill,
  RowIconButton,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import {
  fetchMcpGrants,
  fetchMcpServers,
  fetchMcpSessions,
  fetchUsers,
  refreshMcpSession,
  revokeMcpGrant,
  revokeMcpSession,
  type McpOAuthGrantRow,
  type McpOAuthSessionRow,
  type McpServerRow,
  type UserRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { announceConsent, readConsentResult, type ConsentResult } from "@/lib/mcp-consent";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// both screens read crates/rolter-control/src/mcp_oauth.rs, whose three rules
// they exist to make visible: no token material crosses the API boundary, a
// grant is the unit of consent (revoking it cascades to its sessions), and a
// listing only ever carries rows the caller is allowed to see (#561)

const GRANT_GRID = "1.2fr 1.3fr 1.6fr 150px 130px 96px 44px";
const SESSION_GRID = "1.1fr 1.2fr 1.4fr 140px 130px 150px 44px 44px";

// the org's grants, sessions and the lookup tables that give them names. The
// users listing is best-effort: a member may not be allowed to read it, and
// the screens degrade to a short user id rather than failing the page
function useOrgOAuth(orgId?: string) {
  const servers = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
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
  const users = useQuery({
    queryKey: ["mcp-oauth-users", orgId],
    queryFn: () => fetchUsers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  return { servers, grants, sessions, users };
}

function serverLabel(servers: McpServerRow[] | undefined, id: string): string {
  return servers?.find((s) => s.id === id)?.name ?? id.slice(0, 8);
}

function ownerLabel(users: UserRow[] | undefined, id: string): string {
  return users?.find((u) => u.id === id)?.email ?? id.slice(0, 8);
}

// stated on both screens because it is the one thing a reader cannot check for
// themselves: the tokens exist, they are just never serialised out of the
// control plane
function TokenNotice({ children }: { children: React.ReactNode }) {
  return (
    <p className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-2.5 text-xs leading-relaxed text-muted-foreground">
      {children}
    </p>
  );
}

// what a listing contains depends on who asked for it, so the screens say so
// instead of implying the table is the whole org
function ScopeNote({ note }: { note: string }) {
  return <p className="text-xs text-[color:var(--text-subtle)]">{note}</p>;
}

function Scopes({ scopes }: { scopes: string[] }) {
  const { t } = useTranslation();
  if (scopes.length === 0) {
    return (
      <span className="text-xs text-[color:var(--text-subtle)]">
        {t("pages.mcpOAuth.noScopes")}
      </span>
    );
  }
  return (
    <div className="flex flex-wrap gap-1">
      {scopes.map((s) => (
        <Pill key={s} color="var(--text-secondary)" tint="var(--surface-subtle)">
          {s}
        </Pill>
      ))}
    </div>
  );
}

// both screens are org-scoped reads of the same shape, so the three states are
// shared. `resource` is the already-translated noun LoadError interpolates
function ForbiddenNote({
  resource,
  error,
  onRetry,
}: {
  resource: string;
  error: unknown;
  onRetry: () => void;
}) {
  return <LoadError error={error} resource={resource} onRetry={onRetry} />;
}

// `lead` is what a screen says above its body whatever state the body is in —
// the consent outcome on Auth Sessions, which must not wait for the listing
function LoadingBody({ lead }: { lead?: React.ReactNode }) {
  return (
    <PageBody>
      {lead}
      <TableSkeleton rows={5} />
    </PageBody>
  );
}

function NoOrgNote({ resource, lead }: { resource: string; lead?: React.ReactNode }) {
  const { t } = useTranslation();
  return (
    <PageBody>
      {lead}
      <EmptyState
        uxTarget="mcp-oauth-no-org"
        icon={<Building2 />}
        title={t("pages.mcpOAuth.noOrgTitle")}
        description={t("pages.mcpOAuth.noOrgBody", { resource })}
      />
    </PageBody>
  );
}

// ---------------------------------------------------------------------------
// grants: the consent record a user signed for one MCP server

export function OAuthGrants() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  const queryClient = useQueryClient();
  const { servers, grants, sessions, users } = useOrgOAuth(scope.orgId);

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider.
  // readiness tracks `grants` alone — `users` is best-effort and the screen
  // renders without it, so waiting on it would overstate time-to-interactive
  useScreenReady(!grants.isLoading);
  useErrorState(!!grants.error, "oauth-grants");
  const [confirming, setConfirming] = React.useState<McpOAuthGrantRow | null>(null);
  const now = Date.now();

  const revoke = useMutation({
    mutationFn: (id: string) => revokeMcpGrant(id),
    onSuccess: () => {
      setConfirming(null);
      // the cascade also killed sessions, so both listings are stale
      void queryClient.invalidateQueries({ queryKey: ["mcp-grants", scope.orgId] });
      void queryClient.invalidateQueries({ queryKey: ["mcp-sessions", scope.orgId] });
    },
  });

  // live sessions per grant: the number the revoke confirmation has to name,
  // because that is what the user is actually about to tear down
  const liveByGrant = React.useMemo(() => {
    const counts = new Map<string, number>();
    for (const s of sessions.data ?? []) {
      if (s.revoked_at || Date.parse(s.expires_at) <= now) continue;
      counts.set(s.grant_id, (counts.get(s.grant_id) ?? 0) + 1);
    }
    return counts;
  }, [now, sessions.data]);

  if (!scope.isLoading && !scope.orgId)
    return <NoOrgNote resource={t("errors.resources.oauthGrants")} />;
  if (grants.isLoading || scope.isLoading) return <LoadingBody />;

  const rows = grants.data ?? [];
  const active = rows.filter((g) => g.active).length;
  const sessionsKnown = sessions.isSuccess;

  return (
    <PageBody>
      <TokenNotice>{t("pages.mcpOAuth.grantsTokenNotice")}</TokenNotice>

      {grants.isError ? (
        <ForbiddenNote
          resource={t("errors.resources.oauthGrants")}
          error={grants.error}
          onRetry={() => void grants.refetch()}
        />
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-3">
            <span className="text-sm text-muted-foreground">
              {t("pages.mcpOAuth.grantsCount", { count: rows.length, active })}
            </span>
            {revoke.isError && (
              <span className="text-xs text-[color:var(--status-danger-text)]">
                {(revoke.error as Error).message}
              </span>
            )}
          </div>
          <ScopeNote note={t("pages.mcpOAuth.scopeNoteGrants")} />

          {rows.length === 0 ? (
            // deliberately actionless: consent is only ever given by the user
            // from a client, so there is no button here that could produce one
            <EmptyState
              uxTarget="oauth-grants"
              icon={<Shield />}
              title={t("pages.mcpOAuth.grantsEmptyTitle")}
              description={t("pages.mcpOAuth.grantsEmptyBody")}
            />
          ) : (
            <ListTable label={t("screens.oauth-grants.title")} minWidth={980}>
              <ListHeader grid={GRANT_GRID}>
                <ListHeaderCell>{t("pages.mcpOAuth.server")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.owner")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.scopes")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.granted")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.sessions.header")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.state")}</ListHeaderCell>
                <ListActionsHeader />
              </ListHeader>
              {rows.map((g) => {
                const live = liveByGrant.get(g.id) ?? 0;
                // one grant per (server, owner) pair, so both are what tells
                // two rows apart to a screen reader (#1214)
                const owner = ownerLabel(users.data, g.user_id);
                return (
                  <ListRow key={g.id} grid={GRANT_GRID}>
                    <ListCell className="truncate font-mono text-xs font-semibold">
                      {serverLabel(servers.data, g.server_id)}
                    </ListCell>
                    <ListCell className="truncate text-xs text-[color:var(--text-secondary)]">
                      {ownerLabel(users.data, g.user_id)}
                    </ListCell>
                    <ListCell className="grid">
                      <Scopes scopes={g.scopes} />
                    </ListCell>
                    <ListCell
                      className="font-mono text-xs text-[color:var(--text-secondary)]"
                      title={g.granted_at}
                    >
                      {fmt.dateTime(g.granted_at)}
                    </ListCell>
                    <ListCell className="text-xs text-[color:var(--text-secondary)]">
                      {sessionsKnown ? t("pages.mcpOAuth.liveShort", { count: live }) : "—"}
                    </ListCell>
                    <ListCell className="grid">
                      <Badge tone={g.active ? "success" : "neutral"} dot={g.active}>
                        {g.active ? "ACTIVE" : "REVOKED"}
                      </Badge>
                    </ListCell>
                    <ListCell className="grid">
                      <RowIconButton
                        control="oauth-grant-revoke"
                        danger
                        title={
                          g.active
                            ? t("pages.mcpOAuth.grants.revokeTitle")
                            : t("pages.mcpOAuth.alreadyRevoked")
                        }
                        gate="mcp_oauth_grant:delete"
                        aria-label={t("pages.mcpOAuth.grants.revokeAria", {
                          owner,
                          server: serverLabel(servers.data, g.server_id),
                        })}
                        disabled={!g.active || revoke.isPending}
                        onClick={() => setConfirming(g)}
                      >
                        <ShieldOff className="h-3.5 w-3.5" />
                      </RowIconButton>
                    </ListCell>
                  </ListRow>
                );
              })}
            </ListTable>
          )}
        </>
      )}

      <Dialog open={confirming !== null} onOpenChange={(open) => !open && setConfirming(null)}>
        {confirming && (
          <>
            <DialogHeader>
              <DialogTitle>{t("pages.mcpOAuth.confirm.grantTitle")}</DialogTitle>
              <DialogDescription>
                {/* the cascade is stated before the click, not discovered after
                    it: the server revokes the grant and its sessions in one
                    transaction */}
                <Trans
                  i18nKey="pages.mcpOAuth.confirm.grantBody"
                  values={{
                    server: serverLabel(servers.data, confirming.server_id),
                    sessions: sessionsKnown
                      ? t("pages.mcpOAuth.liveSessions", {
                          count: liveByGrant.get(confirming.id) ?? 0,
                        })
                      : t("pages.mcpOAuth.everySession"),
                  }}
                  components={[<span key="server" className="font-mono" />]}
                />
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={() => setConfirming(null)}>
                {t("common.cancel")}
              </Button>
              <Button disabled={revoke.isPending} onClick={() => revoke.mutate(confirming.id)}>
                {t("pages.mcpOAuth.confirm.grantConfirm")}
              </Button>
            </DialogFooter>
          </>
        )}
      </Dialog>
    </PageBody>
  );
}

// ---------------------------------------------------------------------------
// sessions: the token metadata minted under a grant

// where a consent ends (#2166). the Connect button opens the authorization
// server in a new tab, and the control plane redirects that tab here once the
// user comes back, with the outcome in the query string. a failure arrives as
// its family only — never what the authorization server said — so every
// sentence here is the dashboard's own
function ConsentOutcome({
  result,
  servers,
}: {
  result: ConsentResult;
  servers: McpServerRow[] | undefined;
}) {
  const { t } = useTranslation();
  const titleId = React.useId();
  const ok = result.outcome === "completed";
  const server = result.server ? servers?.find((s) => s.id === result.server)?.name : undefined;
  const Icon = ok ? CircleCheck : CircleAlert;
  return (
    <div
      role={ok ? "status" : "alert"}
      aria-labelledby={titleId}
      className={cn(
        "flex flex-col gap-3 rounded-[10px] border bg-[color:var(--surface-subtle)] p-4 sm:flex-row sm:items-start sm:justify-between",
        ok ? "border-[color:var(--status-success)]/40" : "border-[color:var(--status-danger)]/40",
      )}
    >
      <div className="flex min-w-0 items-start gap-3">
        <Icon
          aria-hidden
          className={cn(
            "mt-0.5 h-4 w-4 shrink-0",
            ok
              ? "text-[color:var(--status-success-text)]"
              : "text-[color:var(--status-danger-text)]",
          )}
        />
        <div className="min-w-0 max-w-prose space-y-1">
          <h2 id={titleId} className="text-sm font-semibold text-foreground">
            {server ? (
              <Trans
                i18nKey={
                  ok
                    ? "pages.mcpOAuth.consent.completedTitle"
                    : "pages.mcpOAuth.consent.failedTitle"
                }
                values={{ server }}
                components={[<span key="server" className="font-mono" />]}
              />
            ) : ok ? (
              t("pages.mcpOAuth.consent.completedTitleUnnamed")
            ) : (
              t("pages.mcpOAuth.consent.failedTitleUnnamed")
            )}
          </h2>
          <p className="text-sm leading-relaxed text-muted-foreground">
            {ok
              ? t("pages.mcpOAuth.consent.completedBody")
              : t(`pages.mcpOAuth.consent.reasons.${result.reason}`)}
          </p>
        </div>
      </div>
      {/* under the text on a phone, lined up with it rather than the icon */}
      <Link
        to="/mcp-catalog"
        className={cn(
          buttonVariants({ variant: "outline", size: "sm" }),
          "ml-7 shrink-0 self-start sm:ml-0",
        )}
      >
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden />
        {t("pages.mcpOAuth.consent.backToCatalog")}
      </Link>
    </div>
  );
}

type SessionState = "active" | "expired" | "revoked";

function sessionState(s: McpOAuthSessionRow, now: number): SessionState {
  if (s.revoked_at) return "revoked";
  return Date.parse(s.expires_at) <= now ? "expired" : "active";
}

export function AuthSessions() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  const queryClient = useQueryClient();
  const { servers, grants, sessions, users } = useOrgOAuth(scope.orgId);

  // UX stream (#805); screen key comes from the enclosing UxScreenProvider.
  // readiness tracks `sessions` alone — `users` is best-effort and the screen
  // renders without it, so waiting on it would overstate time-to-interactive
  useScreenReady(!sessions.isLoading);
  useErrorState(!!sessions.error, "auth-sessions");

  // the end of a consent, when the control plane sent the browser here (#2166)
  const location = useLocation();
  const result = React.useMemo(() => readConsentResult(location.search), [location.search]);
  const completed = result?.outcome === "completed" ? result : null;
  // the tab that started the flow is another one, still showing the catalog;
  // it is told once per session, not once per render
  const announced = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!completed || announced.current === completed.session) return;
    announced.current = completed.session;
    announceConsent(completed.session, completed.server);
  }, [completed]);
  const outcome = result && <ConsentOutcome result={result} servers={servers.data} />;
  // one clock for every relative timestamp, so rows do not drift apart
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);

  const revoke = useMutation({
    mutationFn: (id: string) => revokeMcpSession(id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["mcp-sessions", scope.orgId] }),
  });

  // renewing on demand, next to the background sweeper that renews about five
  // minutes before expiry. a refusal from the authorization server revokes the
  // session rather than being retried, so a failure here is worth reading:
  // it usually means the consent behind it is gone (#707)
  const toast = useToast();
  const refresh = useMutation({
    mutationFn: (id: string) => refreshMcpSession(id),
    onSuccess: () => {
      toast.push({ tone: "success", title: t("pages.mcpOAuth.refresh.done") });
      void queryClient.invalidateQueries({ queryKey: ["mcp-sessions", scope.orgId] });
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("pages.mcpOAuth.refresh.failed"),
        detail: errorDetail(error),
      }),
  });

  // grants on this screen already confirm before revoking; a session revoke is
  // just as irreversible, so it asks the same way (#1179). the server label is
  // carried alongside the row because it is resolved from two other queries
  const [confirming, setConfirming] = React.useState<{
    session: McpOAuthSessionRow;
    server: string;
  } | null>(null);
  const startRevoke = (session: McpOAuthSessionRow, server: string) => {
    revoke.reset();
    setConfirming({ session, server });
  };

  const grantById = React.useMemo(() => {
    const map = new Map<string, McpOAuthGrantRow>();
    for (const g of grants.data ?? []) map.set(g.id, g);
    return map;
  }, [grants.data]);

  if (!scope.isLoading && !scope.orgId)
    return <NoOrgNote resource={t("errors.resources.authSessions")} lead={outcome} />;
  if (sessions.isLoading || scope.isLoading) return <LoadingBody lead={outcome} />;

  const rows = sessions.data ?? [];
  const live = rows.filter((s) => sessionState(s, now) === "active").length;

  return (
    <PageBody>
      {outcome}
      <TokenNotice>{t("pages.mcpOAuth.sessionsTokenNotice")}</TokenNotice>

      {sessions.isError ? (
        <ForbiddenNote
          resource={t("errors.resources.authSessions")}
          error={sessions.error}
          onRetry={() => void sessions.refetch()}
        />
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-3">
            <span className="text-sm text-muted-foreground">
              {t("pages.mcpOAuth.sessionsCount", { count: rows.length, live })}
            </span>
            {revoke.isError && !confirming && (
              <span className="text-xs text-[color:var(--status-danger-text)]">
                {(revoke.error as Error).message}
              </span>
            )}
          </div>
          <ScopeNote note={t("pages.mcpOAuth.scopeNoteSessions")} />

          {rows.length === 0 ? (
            <EmptyState
              uxTarget="auth-sessions"
              icon={<KeyRound />}
              title={t("pages.mcpOAuth.sessionsEmptyTitle")}
              description={t("pages.mcpOAuth.sessionsEmptyBody")}
            />
          ) : (
            <ListTable label={t("screens.auth-sessions.title")} minWidth={1024}>
              <ListHeader grid={SESSION_GRID}>
                <ListHeaderCell>{t("pages.mcpOAuth.server")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.owner")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.scopes")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.lastUsed")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.expires")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpOAuth.state")}</ListHeaderCell>
                {/* renew and revoke are a column each, so each gets a name */}
                <ListActionsHeader label={t("pages.mcpOAuth.sessions.colRenew")} />
                <ListActionsHeader label={t("pages.mcpOAuth.sessions.colRevoke")} />
              </ListHeader>
              {rows.map((s) => {
                const grant = grantById.get(s.grant_id);
                const state = sessionState(s, now);
                const server = grant
                  ? serverLabel(servers.data, grant.server_id)
                  : s.grant_id.slice(0, 8);
                const owner = grant ? ownerLabel(users.data, grant.user_id) : "—";
                // the session the consent that brought the reader here minted
                const fresh = s.id === completed?.session;
                return (
                  <ListRow
                    key={s.id}
                    grid={SESSION_GRID}
                    aria-current={fresh ? "true" : undefined}
                    className={fresh ? "bg-[color:var(--red-tint)]" : undefined}
                  >
                    <ListCell className="truncate font-mono text-xs font-semibold">
                      {server}
                    </ListCell>
                    <ListCell className="truncate text-xs text-[color:var(--text-secondary)]">
                      {grant ? ownerLabel(users.data, grant.user_id) : "—"}
                    </ListCell>
                    <ListCell className="grid">
                      <Scopes scopes={s.scopes} />
                    </ListCell>
                    <ListCell className="text-xs text-[color:var(--text-secondary)]">
                      {s.last_used_at
                        ? fmt.relative(s.last_used_at, now)
                        : t("pages.mcpOAuth.never")}
                    </ListCell>
                    <ListCell
                      className="font-mono text-xs text-[color:var(--text-secondary)]"
                      title={s.expires_at}
                    >
                      {fmt.dateTime(s.expires_at)}
                    </ListCell>
                    <ListCell className="flex flex-wrap items-center gap-1.5">
                      <Badge
                        tone={
                          state === "active"
                            ? "success"
                            : state === "expired"
                              ? "warning"
                              : "neutral"
                        }
                        dot={state === "active"}
                      >
                        {state.toUpperCase()}
                      </Badge>
                      {/* renewability is reported as a flag; the refresh token
                          itself is never part of the payload */}
                      {s.has_refresh_token && <Badge tone="info">RENEWABLE</Badge>}
                      {fresh && (
                        <Badge tone="accent">{t("pages.mcpOAuth.consent.newSession")}</Badge>
                      )}
                    </ListCell>
                    {/* renewal is only possible where a refresh token was
                        stored, and a revoked session has nothing to renew */}
                    <ListCell className="grid">
                      <RowIconButton
                        gate="mcp_oauth_session:update"
                        control="oauth-session-refresh"
                        title={t("pages.mcpOAuth.refresh.action", { server })}
                        aria-label={t("pages.mcpOAuth.sessions.refreshAria", {
                          owner,
                          server,
                        })}
                        disabled={
                          !s.has_refresh_token ||
                          state === "revoked" ||
                          (refresh.isPending && refresh.variables === s.id)
                        }
                        onClick={() => refresh.mutate(s.id)}
                      >
                        {refresh.isPending && refresh.variables === s.id ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <RefreshCw className="h-3.5 w-3.5" />
                        )}
                      </RowIconButton>
                    </ListCell>
                    <ListCell className="grid">
                      <RowIconButton
                        control="oauth-session-revoke"
                        danger
                        title={
                          state === "revoked"
                            ? t("pages.mcpOAuth.alreadyRevoked")
                            : t("pages.mcpOAuth.sessions.revokeTitle")
                        }
                        gate="mcp_oauth_session:delete"
                        aria-label={t("pages.mcpOAuth.sessions.revokeAria", {
                          owner,
                          server,
                        })}
                        disabled={
                          state === "revoked" || (revoke.isPending && revoke.variables === s.id)
                        }
                        onClick={() => startRevoke(s, server)}
                      >
                        {revoke.isPending && revoke.variables === s.id ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                          <ShieldOff className="h-3.5 w-3.5" />
                        )}
                      </RowIconButton>
                    </ListCell>
                  </ListRow>
                );
              })}
            </ListTable>
          )}
          <ConfirmDialog
            name="mcp-oauth-session-revoke"
            open={confirming !== null}
            onOpenChange={(open) => !open && setConfirming(null)}
            title={t("pages.mcpOAuth.confirm.sessionTitle", {
              server: confirming?.server,
            })}
            description={t("pages.mcpOAuth.confirm.sessionBody")}
            confirmLabel={t("pages.mcpOAuth.confirm.sessionConfirm")}
            pending={revoke.isPending}
            error={revoke.error}
            onConfirm={() =>
              confirming &&
              revoke.mutate(confirming.session.id, {
                onSuccess: () => setConfirming(null),
              })
            }
          />

          <p className="text-xs text-[color:var(--text-subtle)]">
            {t("pages.mcpOAuth.sessionsFootnote")}
          </p>
        </>
      )}
    </PageBody>
  );
}
