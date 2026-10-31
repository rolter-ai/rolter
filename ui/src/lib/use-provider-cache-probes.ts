import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { fetchProviderCacheProbes, type ProviderCacheVerdict } from "@/lib/api";

export const PROVIDER_CACHE_PROBES_KEY = "provider-cache-probes";

/**
 * Every stored cache-reporting verdict of an org, loaded once and shared by the
 * screens that show it (Providers, Models, Provider Groups) through one query
 * key.
 *
 * `byProvider` is `undefined` until the read has answered, and stays so when it
 * failed. A screen shows nothing then: an absent map is "unknown", while an
 * empty one is "no provider was ever probed", and only the second may be shown
 * as "not probed yet".
 */
export function useProviderCacheProbes(orgId: string | null | undefined) {
  const probes = useQuery({
    queryKey: [PROVIDER_CACHE_PROBES_KEY, orgId],
    queryFn: () => fetchProviderCacheProbes(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const byProvider = React.useMemo(
    () =>
      probes.data
        ? new Map<string, ProviderCacheVerdict>(probes.data.map((v) => [v.provider_id, v]))
        : undefined,
    [probes.data],
  );
  return { probes, byProvider };
}
