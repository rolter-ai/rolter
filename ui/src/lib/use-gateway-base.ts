import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { fetchClientSettings } from "@/lib/api";
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
 * The gateway address for a snippet on this screen (#2218).
 *
 * Reads the saved public base URL from the same query Client Settings uses,
 * so any number of snippets share one request, and hands it to
 * {@link gatewayBase}. Client settings are superadmin-only
 * (`client_settings:read`), so only a caller the gate has cleared asks: a
 * lesser role, or one whose gate has not answered yet, gets the `/gw` proxy
 * rather than a 403 per snippet.
 */
export function useGatewayBase(): GatewayBase {
  const can = useCan();
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
  const saved = settings.data?.public_base_url;
  return React.useMemo(() => gatewayBase(typeof saved === "string" ? saved : null), [saved]);
}
