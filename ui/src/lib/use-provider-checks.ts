import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { fetchProviderChecks, type ProviderCheck } from "@/lib/api";

export const PROVIDER_CHECKS_KEY = "provider-checks";

/**
 * Every provider check of an org (#3649): whether its key variable is set and
 * how its last connection test went, loaded once and shared by the screens
 * that show it through one query key. A test in the provider sheet invalidates
 * the key, so the list reads the new result without testing again.
 *
 * `byProvider` is `undefined` until the read has answered, and stays so when it
 * failed: a screen shows nothing then rather than "not tested".
 */
export function useProviderChecks(orgId: string | null | undefined) {
  const checks = useQuery({
    queryKey: [PROVIDER_CHECKS_KEY, orgId],
    queryFn: () => fetchProviderChecks(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const byProvider = React.useMemo(
    () =>
      checks.data
        ? new Map<string, ProviderCheck>(checks.data.map((c) => [c.provider_id, c]))
        : undefined,
    [checks.data],
  );
  return { checks, byProvider };
}
