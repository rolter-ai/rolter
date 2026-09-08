import { useQuery } from "@tanstack/react-query";

import { fetchVersion, type VersionStatus } from "@/lib/api";

/**
 * What the rail footer needs to know about a newer release (#902).
 *
 * `null` in every state that has nothing to show — checking, disabled,
 * offline, an error, or a build that is current — so the footer renders the
 * plain version and nothing else. The compiled `__APP_VERSION__` stays the
 * displayed version when the endpoint is unreachable; when it answers, its
 * `current` wins, since it is the control plane's own build.
 */
export interface UpdateHint {
  latest: string;
  url: string;
}

const RELEASES_LATEST_URL = "https://github.com/rolter-ai/rolter/releases/latest";

export function updateHintFrom(status: VersionStatus | undefined): UpdateHint | null {
  if (!status?.enabled || !status.update_available || !status.latest) return null;
  return { latest: status.latest, url: status.release_url || RELEASES_LATEST_URL };
}

/**
 * `enabled` is whether there is a session to ask with: the endpoint takes an
 * authenticated caller, and asking from the login screen would only produce a
 * 401 nobody can act on.
 */
export function useVersionStatus(
  fallback: string,
  enabled: boolean,
): {
  version: string;
  update: UpdateHint | null;
} {
  const query = useQuery({
    queryKey: ["version"],
    queryFn: fetchVersion,
    enabled,
    retry: false,
    // the control plane re-checks every six hours; an hour between browser
    // reads is plenty, and a failed read is not retried into a toast
    staleTime: 60 * 60_000,
  });
  return {
    version: query.data?.current || fallback,
    update: updateHintFrom(query.data),
  };
}
