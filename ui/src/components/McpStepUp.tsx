import { useQuery } from "@tanstack/react-query";
import { Loader2, ShieldPlus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  fetchMcpGrants,
  fetchMcpServers,
  fetchMcpSessions,
  type McpLogDetail,
  type McpOAuthGrantRow,
  type McpOAuthSessionRow,
  type McpServerRow,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { heldScopes, missingScopes, parseScopes } from "@/lib/mcp-step-up";
import { useScope } from "@/lib/scope";
import { useMcpConnect } from "@/lib/use-mcp-connect";

/**
 * "Reconnect with more access" (#3296): consent again for the server with the
 * scopes it refused the call for added to the ones already held.
 *
 * The request and the pop-up handling are `useMcpConnect`'s, shared with
 * Connect and Reconnect. `scopes` are the ones to add; nothing is sent for an
 * empty list, since a step-up with nothing to add is just a Connect.
 */
export function StepUpButton({
  server,
  scopes,
  describedBy,
}: {
  server: Pick<McpServerRow, "id" | "name">;
  scopes: string[];
  describedBy?: string;
}) {
  const { t } = useTranslation();
  const connect = useMcpConnect(server, scopes);
  return (
    <Button
      variant="outline"
      disabled={scopes.length === 0 || connect.isPending}
      aria-describedby={describedBy}
      aria-label={t("pages.mcpStepUp.actionNamed", { name: server.name })}
      onClick={() => connect.mutate()}
    >
      {connect.isPending ? (
        <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />
      ) : (
        <ShieldPlus className="mr-2 h-4 w-4" aria-hidden />
      )}
      {t("pages.mcpStepUp.action")}
    </Button>
  );
}

/**
 * On an OAuth server's card: the scopes the server requires that the caller's
 * live session does not hold, with the action that asks for them (#3296).
 * Nothing when the session covers them, when there is no live session (that is
 * Connect's business) and when the listings are not known.
 */
export function ServerStepUp({
  server,
  grants,
  sessions,
  userId,
}: {
  server: McpServerRow;
  grants: McpOAuthGrantRow[] | undefined;
  sessions: McpOAuthSessionRow[] | undefined;
  userId: string | undefined;
}) {
  const { t } = useTranslation();
  const missing = missingScopes(
    server,
    heldScopes(grants, sessions, userId, server.id, Date.now()),
  );
  if (missing.length === 0) return null;
  const id = `mcp-step-up-${server.id}`;
  return (
    <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-2">
      <p id={id} className="min-w-0 flex-1 text-xs text-[color:var(--status-warning-text)]">
        {t("pages.mcpStepUp.missing", { count: missing.length, scopes: missing.join(", ") })}
      </p>
      <StepUpButton server={server} scopes={missing} describedBy={id} />
    </div>
  );
}

/**
 * In the MCP Logs detail of an `auth_denied` call: what a step-up would add
 * (#3296).
 *
 * A refusal for `insufficient_scope` records the scopes the upstream asked for
 * (#3375), and those are what the action adds. A row without them (written
 * before the log recorded them, or refused for another reason) offers the ones
 * the server declares that the caller's session lacks. When it lacks none, the
 * upstream asked for something the server does not declare, and the owner names
 * it. Only the key's owner can
 * consent: the step-up widens their own grant, so anyone else is told so.
 * The three reads are best-effort and shared with the catalog, and the note
 * says nothing for a server that is not OAuth, where `auth_denied` means
 * something a new consent does not fix.
 */
export function LogStepUp({ detail }: { detail: McpLogDetail }) {
  const { t } = useTranslation();
  const { orgId } = useScope();
  const auth = useOptionalAuth();
  const enabled = !!orgId && detail.status === "auth_denied";
  const servers = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled,
    retry: false,
  });
  const grants = useQuery({
    queryKey: ["mcp-grants", orgId],
    queryFn: () => fetchMcpGrants(orgId as string),
    enabled,
    retry: false,
  });
  const sessions = useQuery({
    queryKey: ["mcp-sessions", orgId],
    queryFn: () => fetchMcpSessions(orgId as string),
    enabled,
    retry: false,
  });
  const [typed, setTyped] = React.useState("");
  const scopesId = React.useId();
  const heldId = React.useId();

  const server = servers.data?.find((s) => s.slug === detail.server && s.auth_kind === "oauth");
  if (!enabled || !server) return null;
  const owner = !!auth?.user?.id && auth.user.id === detail.user_id;
  const held = heldScopes(grants.data, sessions.data, auth?.user?.id, server.id, Date.now());
  const missing = missingScopes(server, held);
  const named = parseScopes(typed);
  // what the upstream asked for, less what the session already holds
  const asked = detail.error_scopes ?? [];
  const toAdd = asked.filter((scope) => !held?.includes(scope));

  return (
    <section
      aria-labelledby={scopesId}
      className="flex flex-col gap-2 rounded-[8px] border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3"
    >
      <h3 id={scopesId} className="text-sm font-medium">
        {t("pages.mcpStepUp.logTitle")}
      </h3>
      <p className="text-xs leading-relaxed text-muted-foreground">
        {t("pages.mcpStepUp.logBody", { server: server.name })}
      </p>
      {!owner ? (
        <p className="text-xs text-muted-foreground">{t("pages.mcpStepUp.notOwner")}</p>
      ) : asked.length > 0 ? (
        <>
          <p className="text-xs text-[color:var(--status-warning-text)]">
            {t("pages.mcpStepUp.asked", { count: asked.length, scopes: asked.join(", ") })}
          </p>
          <div>
            <StepUpButton server={server} scopes={toAdd} describedBy={heldId} />
          </div>
          {toAdd.length === 0 && (
            <p id={heldId} className="text-xs text-muted-foreground">
              {t("pages.mcpStepUp.askedHeld")}
            </p>
          )}
        </>
      ) : missing.length > 0 ? (
        <>
          <p className="text-xs text-[color:var(--status-warning-text)]">
            {t("pages.mcpStepUp.missing", { count: missing.length, scopes: missing.join(", ") })}
          </p>
          <div>
            <StepUpButton server={server} scopes={missing} />
          </div>
        </>
      ) : (
        <>
          <Field label={t("pages.mcpStepUp.scopesLabel")} hint={t("pages.mcpStepUp.scopesHint")}>
            <Input
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              placeholder={t("pages.mcpStepUp.scopesPlaceholder")}
            />
          </Field>
          <div>
            <StepUpButton server={server} scopes={named} />
          </div>
        </>
      )}
    </section>
  );
}
