import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { fetchClientSettings } from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { useCan } from "@/lib/can";
import { gatewayBase, type GatewayBase } from "@/lib/gateway";

/**
 * The query the Client Settings screen loads and writes through.
 *
 * Shared rather than spelled out twice so a save there — which writes this key
 * and invalidates it — reaches every snippet already on screen.
 */
export const CLIENT_SETTINGS_QUERY_KEY = ["client-settings"] as const;

/**
 * The gateway address for a snippet on this screen (#2218), or `null` when
 * there is none to show.
 *
 * Reads the saved public base URL from the same query Client Settings uses,
 * so any number of snippets share one request, and hands it to
 * {@link gatewayBase}. Client settings are superadmin-only
 * (`client_settings:read`), so only a caller the gate has cleared asks. Every
 * other signed-in role takes the address `/auth/me` carries (#2512), which is
 * not a secret, so no role gets a 403 per snippet.
 *
 * `null` is never answered with the dashboard's `/gw` proxy (#2486): it needs
 * a dashboard session, and a snippet is for a client that has none. Callers
 * show {@link GatewayBasePrompt} instead.
 */
export function useGatewayBase(): GatewayBase | null {
  const can = useCan();
  const fromSession = useOptionalAuth()?.gatewayBaseUrl ?? null;
  const readable = can("client_settings", "read") === true;
  const settings = useQuery({
    queryKey: CLIENT_SETTINGS_QUERY_KEY,
    queryFn: fetchClientSettings,
    enabled: readable,
    retry: false,
    // the value changes when a superadmin saves Client Settings, which
    // invalidates this key itself; nothing else moves it
    staleTime: 300_000,
  });
  // the settings query wins: it is invalidated by a save, while the session's
  // copy is read once when the session boots
  const fetched = settings.data?.public_base_url;
  const saved = typeof fetched === "string" ? fetched : fromSession;
  return React.useMemo(() => gatewayBase(saved), [saved]);
}
