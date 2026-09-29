import * as React from "react";

import type { McpOAuthGrantRow, McpOAuthSessionRow } from "@/lib/api";

// the end of an MCP OAuth consent, as the control plane hands it to the
// dashboard (#2166). the authorization server sends the user's browser back to
// `/auth/mcp/callback`, and the control plane redirects a browser on to Auth
// Sessions with the outcome in the query string:
//
//   consent=completed&session=<id>&server=<id>
//   consent=failed&reason=<code>[&server=<id>]
//
// only identifiers the caller can already list travel this way, never a code or
// a token. `crates/rolter-control/src/mcp_oauth_flow.rs` owns the codes, and
// `mcp-consent.test.ts` holds this list to that one

/** Every failure code the control plane sends, each with its own copy. */
export const CONSENT_FAILURES = [
  "access_denied",
  "authorization_failed",
  "state_invalid",
  "issuer_mismatch",
  "token_exchange_failed",
  "not_configured",
  "internal_error",
] as const;

/**
 * A failure code, or `unknown` for one this build has no copy for: a newer
 * control plane may send a code the dashboard has never heard of, and it still
 * gets a sentence rather than a raw key.
 */
export type ConsentFailure = (typeof CONSENT_FAILURES)[number] | "unknown";

export type ConsentResult =
  | { outcome: "completed"; session: string; server: string | null }
  | { outcome: "failed"; reason: ConsentFailure; server: string | null };

const isKnownFailure = (code: string | null): code is (typeof CONSENT_FAILURES)[number] =>
  (CONSENT_FAILURES as readonly (string | null)[]).includes(code);

/** The outcome in a location's query string, or `null` when it carries none. */
export function readConsentResult(search: string): ConsentResult | null {
  const params = new URLSearchParams(search);
  const server = params.get("server") || null;
  switch (params.get("consent")) {
    case "completed": {
      const session = params.get("session");
      return session ? { outcome: "completed", session, server } : null;
    }
    case "failed": {
      const reason = params.get("reason");
      return { outcome: "failed", reason: isKnownFailure(reason) ? reason : "unknown", server };
    }
    default:
      return null;
  }
}

/** The channel a completed consent is announced on, to every tab of the dashboard. */
export const CONSENT_CHANNEL = "rolter.mcp-consent";

/** What the landing tab tells the others: which session now exists, on which server. */
export interface ConsentAnnouncement {
  kind: "mcp-consent-completed";
  session: string;
  server: string | null;
}

const isAnnouncement = (data: unknown): data is ConsentAnnouncement =>
  typeof data === "object" &&
  data !== null &&
  (data as { kind?: unknown }).kind === "mcp-consent-completed" &&
  typeof (data as { session?: unknown }).session === "string";

/**
 * Tell every other tab that a consent completed. The Connect button opens the
 * authorization server in a new tab, so the tab that started the flow is never
 * the one it ends in; this is how that tab learns without a reload. A browser
 * with no `BroadcastChannel` falls back to the refetch a query does when its
 * window regains focus.
 */
export function announceConsent(session: string, server: string | null): void {
  if (typeof BroadcastChannel === "undefined") return;
  const channel = new BroadcastChannel(CONSENT_CHANNEL);
  const message: ConsentAnnouncement = { kind: "mcp-consent-completed", session, server };
  // the message is queued for every other channel as it is posted, so closing
  // straight after does not lose it
  channel.postMessage(message);
  channel.close();
}

/** Call `onCompleted` whenever another tab announces a completed consent. */
export function useConsentAnnouncements(onCompleted: (message: ConsentAnnouncement) => void) {
  const handler = React.useRef(onCompleted);
  React.useEffect(() => {
    handler.current = onCompleted;
  });
  React.useEffect(() => {
    if (typeof BroadcastChannel === "undefined") return;
    const channel = new BroadcastChannel(CONSENT_CHANNEL);
    channel.onmessage = (event: MessageEvent) => {
      if (isAnnouncement(event.data)) handler.current(event.data);
    };
    return () => channel.close();
  }, []);
}

/**
 * The servers `userId` holds a live session on: an active grant of theirs with
 * a session under it that is neither revoked nor expired.
 *
 * Filtered to the one account on purpose. An org admin's listings carry every
 * member's grants, and a card that read "connected" because a colleague had
 * consented would be telling the admin something false about their own access.
 */
export function connectedServers(
  grants: McpOAuthGrantRow[] | undefined,
  sessions: McpOAuthSessionRow[] | undefined,
  userId: string | undefined,
  now: number,
): Set<string> {
  const connected = new Set<string>();
  if (!userId || !grants || !sessions) return connected;
  const mine = new Map<string, string>();
  for (const grant of grants) {
    if (grant.active && grant.user_id === userId) mine.set(grant.id, grant.server_id);
  }
  for (const session of sessions) {
    const server = mine.get(session.grant_id);
    if (server && !session.revoked_at && Date.parse(session.expires_at) > now) {
      connected.add(server);
    }
  }
  return connected;
}
