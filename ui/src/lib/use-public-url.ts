import { useQuery } from "@tanstack/react-query";

import { fetchPublicUrl } from "@/lib/api";

/**
 * The query every screen that hands out an address for an identity provider
 * reads the control plane's public base through (#2083, #2079).
 *
 * One key, so the Single Sign-On and User Provisioning screens share a single
 * request and agree on whether `ROLTER_PUBLIC_URL` is set. Both read it through
 * `usePublicUrl()`, which is the only place the key is spelled out, and so does
 * the Connectors screen's collector-config dialog (#2106).
 */
export const PUBLIC_URL_QUERY_KEY = ["public-url"] as const;

/**
 * The control plane's public base URL, and whether `ROLTER_PUBLIC_URL` set it.
 *
 * Held by every signed-in caller, so it is asked for unconditionally. It is read
 * once at startup on the server, so it never goes stale within a session.
 * `retry` is off so a failure reaches the screen, which offers its own retry.
 */
export function usePublicUrl() {
  return useQuery({
    queryKey: PUBLIC_URL_QUERY_KEY,
    queryFn: fetchPublicUrl,
    retry: false,
    staleTime: Infinity,
  });
}
