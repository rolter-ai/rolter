import * as React from "react";
import { useQuery } from "@tanstack/react-query";

import {
  fetchStability,
  fetchVersion,
  type SubsystemStability,
  type VersionStatus,
} from "@/lib/api";

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

/**
 * The nav leaf keys this build ships as experimental, and why (#1386).
 *
 * Keyed by nav leaf key rather than by subsystem id: the rail asks "is this
 * entry marked", and `nav_keys` on the wire is what makes that answerable
 * without a second list in `ui/`. A subsystem with no nav entry contributes
 * nothing here — it is documented, not navigated.
 */
export type ExperimentalNavKeys = ReadonlyMap<string, string>;

export function experimentalNavKeysFrom(
  subsystems: readonly SubsystemStability[] | undefined,
): ExperimentalNavKeys {
  const marked = new Map<string, string>();
  for (const entry of subsystems ?? []) {
    // the level rides on each entry, so membership of the list is never what
    // the marker is inferred from
    if (entry.stability !== "experimental") continue;
    for (const key of entry.nav_keys ?? []) marked.set(key, entry.note);
  }
  return marked;
}

/**
 * Which nav entries carry the experimental marker.
 *
 * Its own request to `/api/v1/stability` (#1385): the answer is a fact about
 * the binary, so it is read once per session and kept for an hour. Deliberately
 * tolerant of every way the answer can fail to arrive: a 404 from an older
 * control plane, a network error, or a session still being checked all yield
 * an empty map. Nothing is marked and nothing is thrown, because a rail that
 * will not render is a worse outcome than a rail missing two badges.
 */
export function useStability(enabled: boolean): ExperimentalNavKeys {
  const query = useQuery({
    queryKey: ["stability"],
    queryFn: fetchStability,
    enabled,
    retry: false,
    staleTime: 60 * 60_000,
  });
  const subsystems = query.data;
  return React.useMemo(() => experimentalNavKeysFrom(subsystems), [subsystems]);
}
