import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building2, KeyRound, Loader2, RefreshCw, Shield, ShieldOff } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  Pill,
  RowIconButton,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
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
function ScopeNote({ noun }: { noun: string }) {
  return (
    <p className="text-xs text-[color:var(--text-subtle)]">
      Org admins see every {noun} in the org; members and viewers see — and may
      revoke — only their own.
    </p>
  );
}

function Scopes({ scopes }: { scopes: string[] }) {
  if (scopes.length === 0) {
    return <span className="text-xs text-[color:var(--text-subtle)]">no scopes</span>;
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

function LoadingBody() {
  return (
    <PageBody>
      <TableSkeleton rows={5} />
    </PageBody>
  );
}

function NoOrgNote({ resource }: { resource: string }) {
  const { t } = useTranslation();
  return (
    <PageBody>
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
      <TokenNotice>
        A grant records consent, not credentials. No access or refresh token is
        ever sent to this dashboard — the sealed tokens stay inside the control
        plane, and revoking consent here is what makes them unusable.
      </TokenNotice>

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
              {rows.length} grants · {active} active
            </span>
            {revoke.isError && (
              <span className="text-xs text-[color:var(--status-danger-text)]">
                {(revoke.error as Error).message}
              </span>
            )}
          </div>
          <ScopeNote noun="grant" />

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
            <ListTable minWidth={980}>
              <ListHeader grid={GRANT_GRID}>
                <span>Server</span>
                <span>Owner</span>
                <span>Scopes</span>
                <span>Granted</span>
                <span>Sessions</span>
                <span>State</span>
                <span />
              </ListHeader>
              {rows.map((g) => {
                const live = liveByGrant.get(g.id) ?? 0;
                // one grant per (server, owner) pair, so both are what tells
                // two rows apart to a screen reader (#1214)
                const owner = ownerLabel(users.data, g.user_id);
                return (
                  <ListRow key={g.id} grid={GRANT_GRID}>
                    <span className="truncate font-mono text-xs font-semibold">
                      {serverLabel(servers.data, g.server_id)}
                    </span>
                    <span className="truncate text-xs text-[color:var(--text-secondary)]">
                      {ownerLabel(users.data, g.user_id)}
                    </span>
                    <Scopes scopes={g.scopes} />
                    <span
                      className="font-mono text-xs text-[color:var(--text-secondary)]"
                      title={g.granted_at}
                    >
                      {fmt.dateTime(g.granted_at)}
                    </span>
                    <span className="text-xs text-[color:var(--text-secondary)]">
                      {sessionsKnown ? t("pages.mcpOAuth.liveShort", { count: live }) : "—"}
                    </span>
                    <Badge tone={g.active ? "success" : "neutral"} dot={g.active}>
                      {g.active ? "ACTIVE" : "REVOKED"}
                    </Badge>
                    <RowIconButton
                      danger
                      title={
                        g.active
                          ? "Revoke this consent and every session under it"
                          : "Already revoked"
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
                  </ListRow>
                );
              })}
            </ListTable>
          )}
        </>
      )}

      <Dialog
        open={confirming !== null}
        onOpenChange={(open) => !open && setConfirming(null)}
      >
        {confirming && (
          <>
            <DialogHeader>
              <DialogTitle>Revoke consent</DialogTitle>
              <DialogDescription>
                {/* the cascade is stated before the click, not discovered after
                    it: the server revokes the grant and its sessions in one
                    transaction */}
                Revoking this grant for{" "}
                <span className="font-mono">
                  {serverLabel(servers.data, confirming.server_id)}
                </span>{" "}
                also revokes{" "}
                {sessionsKnown
                  ? t("pages.mcpOAuth.liveSessions", {
                      count: liveByGrant.get(confirming.id) ?? 0,
                    })
                  : "every session"}{" "}
                under it, in the same transaction. The user must consent again
                before the server can be called on their behalf.
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={() => setConfirming(null)}>
                Cancel
              </Button>
              <Button
                disabled={revoke.isPending}
                onClick={() => revoke.mutate(confirming.id)}
              >
                Revoke consent
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
  // one clock for every relative timestamp, so rows do not drift apart
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);

  const revoke = useMutation({
    mutationFn: (id: string) => revokeMcpSession(id),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: ["mcp-sessions", scope.orgId] }),
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
  const [confirming, setConfirming] = React.useState<
    { session: McpOAuthSessionRow; server: string } | null
  >(null);
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
    return <NoOrgNote resource={t("errors.resources.authSessions")} />;
  if (sessions.isLoading || scope.isLoading) return <LoadingBody />;

  const rows = sessions.data ?? [];
  const live = rows.filter((s) => sessionState(s, now) === "active").length;

  return (
    <PageBody>
      <TokenNotice>
        Sessions are shown as metadata only. The access and refresh tokens are
        sealed with the deployment key and never leave the control plane, so
        this screen can tell you a session is renewable but never what it holds.
      </TokenNotice>

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
              {rows.length} sessions · {live} live
            </span>
            {revoke.isError && !confirming && (
              <span className="text-xs text-[color:var(--status-danger-text)]">
                {(revoke.error as Error).message}
              </span>
            )}
          </div>
          <ScopeNote noun="session" />

          {rows.length === 0 ? (
            <EmptyState
              uxTarget="auth-sessions"
              icon={<KeyRound />}
              title={t("pages.mcpOAuth.sessionsEmptyTitle")}
              description={t("pages.mcpOAuth.sessionsEmptyBody")}
            />
          ) : (
            <ListTable minWidth={1024}>
              <ListHeader grid={SESSION_GRID}>
                <span>Server</span>
                <span>Owner</span>
                <span>Scopes</span>
                <span>Last used</span>
                <span>Expires</span>
                <span>State</span>
                <span />
                <span />
              </ListHeader>
              {rows.map((s) => {
                const grant = grantById.get(s.grant_id);
                const state = sessionState(s, now);
                const server = grant
                  ? serverLabel(servers.data, grant.server_id)
                  : s.grant_id.slice(0, 8);
                const owner = grant ? ownerLabel(users.data, grant.user_id) : "—";
                return (
                  <ListRow key={s.id} grid={SESSION_GRID}>
                    <span className="truncate font-mono text-xs font-semibold">
                      {server}
                    </span>
                    <span className="truncate text-xs text-[color:var(--text-secondary)]">
                      {grant ? ownerLabel(users.data, grant.user_id) : "—"}
                    </span>
                    <Scopes scopes={s.scopes} />
                    <span className="text-xs text-[color:var(--text-secondary)]">
                      {s.last_used_at ? fmt.relative(s.last_used_at, now) : "never"}
                    </span>
                    <span
                      className="font-mono text-xs text-[color:var(--text-secondary)]"
                      title={s.expires_at}
                    >
                      {fmt.dateTime(s.expires_at)}
                    </span>
                    <div className="flex flex-wrap items-center gap-1.5">
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
                    </div>
                    {/* renewal is only possible where a refresh token was
                        stored, and a revoked session has nothing to renew */}
                    <RowIconButton
                      gate="mcp_oauth_session:update"
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
                    <RowIconButton
                      danger
                      title={
                        state === "revoked"
                          ? "Already revoked"
                          : "Revoke this session; the consent behind it stands"
                      }
                      gate="mcp_oauth_session:delete"
                      aria-label={t("pages.mcpOAuth.sessions.revokeAria", {
                        owner,
                        server,
                      })}
                      disabled={state === "revoked" || (revoke.isPending && revoke.variables === s.id)}
                      onClick={() => startRevoke(s, server)}
                    >
                      {revoke.isPending && revoke.variables === s.id ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      ) : (
                        <ShieldOff className="h-3.5 w-3.5" />
                      )}
                    </RowIconButton>
                  </ListRow>
                );
              })}
            </ListTable>
          )}
          <ConfirmDialog
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
            Revoking a session leaves the consent in place — a new session can be
            minted without asking the user again. To withdraw consent itself,
            revoke the grant on OAuth Grants.
          </p>
        </>
      )}
    </PageBody>
  );
}
