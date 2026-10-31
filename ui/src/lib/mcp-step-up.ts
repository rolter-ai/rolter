import type { McpOAuthGrantRow, McpOAuthSessionRow, McpServerRow } from "@/lib/api";

/**
 * The scopes the caller's live session on a server holds (#3296), or
 * `undefined` when there is none to read them from.
 *
 * The gateway checks a server's `required_scopes` against one session of the
 * key's owner, so the session that is live the longest is the one read here;
 * an expired or revoked session holds nothing.
 */
export function heldScopes(
  grants: McpOAuthGrantRow[] | undefined,
  sessions: McpOAuthSessionRow[] | undefined,
  userId: string | undefined,
  serverId: string,
  now: number,
): string[] | undefined {
  if (!userId || !grants || !sessions) return undefined;
  const mine = new Set(
    grants
      .filter((g) => g.active && g.user_id === userId && g.server_id === serverId)
      .map((g) => g.id),
  );
  let best: McpOAuthSessionRow | undefined;
  for (const session of sessions) {
    if (!mine.has(session.grant_id) || session.revoked_at) continue;
    const ends = Date.parse(session.expires_at);
    if (ends <= now) continue;
    if (!best || ends > Date.parse(best.expires_at)) best = session;
  }
  return best?.scopes;
}

/**
 * The scopes the server declares it needs that the session does not hold: what
 * the gateway refuses with `mcp_scope_denied`, and what a step-up asks for.
 * Empty when the session covers them, and when the held scopes are unknown.
 */
export function missingScopes(
  server: Pick<McpServerRow, "required_scopes">,
  held: string[] | undefined,
): string[] {
  if (!held) return [];
  return server.required_scopes.filter((scope) => !held.includes(scope));
}

/** Scope tokens typed into a field: split on whitespace or commas, deduplicated. */
export function parseScopes(text: string): string[] {
  return [...new Set(text.split(/[\s,]+/).filter(Boolean))];
}
