import { useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  ApiError,
  fetchPreferences,
  isOpenModeNoSession,
  preferencesDocument,
  putPreferences,
  type PreferencesResponse,
  type UserPreferences,
} from "@/lib/api";
import { useOptionalAuth } from "@/lib/auth";
import { applyLanguagePreference, clearStoredLocale, storedLocale } from "@/lib/i18n";
import { setChartTimeZone } from "@/lib/i18n/format";
import {
  clearCachedPreferences,
  readCachedPreferences,
  writeCachedPreferences,
} from "@/lib/preferences-cache";

// the signed-in account's server-side preferences (#2448): language, default
// scope, Playground model and chart time zone. the document used to live in
// this browser's localStorage, so a new laptop started from nothing.
//
// the first paint uses the cached copy of the last answer; the fetch that
// follows replaces it and the server's values win. nothing else holds a copy
// of a preference — language and zone are applied from here, the scope hook and
// the Playground read it from here

export const PREFERENCES_QUERY_KEY = ["me", "preferences"] as const;

// the pre-server scope pick, read once to seed an empty document
const LEGACY_SCOPE_KEY = "rolter.scope";

export interface PreferencesState {
  /** the last answer in hand: the cached one on first paint, then the server's */
  preferences: PreferencesResponse | undefined;
  /** the first read is still on its way and there is no cached copy */
  isPending: boolean;
  error: Error | null;
  /** open mode: no accounts, so there is no one to hold preferences (#942) */
  unavailable: boolean;
  refetch: () => void;
  /** save `patch` over the latest document; see `savePreferences` */
  save: (patch: Partial<UserPreferences>) => Promise<PreferencesResponse>;
}

const PreferencesContext = React.createContext<PreferencesState | null>(null);

const EMPTY_KEYS: (keyof UserPreferences)[] = [
  "language",
  "default_org_id",
  "default_team_id",
  "default_project_id",
  "default_playground_model",
  "chart_time_zone",
];

/** A document with nothing set — what the server holds for an account that never saved one. */
export function isEmptyDocument(doc: UserPreferences): boolean {
  return EMPTY_KEYS.every((key) => doc[key] === null || doc[key] === undefined);
}

interface LegacyValues {
  language: string | null;
  scope: { org: string | null; team: string | null; project: string | null };
}

function readLegacy(): LegacyValues {
  let scope: LegacyValues["scope"] = { org: null, team: null, project: null };
  try {
    const raw = localStorage.getItem(LEGACY_SCOPE_KEY);
    if (raw) {
      const stored = JSON.parse(raw) as { orgId?: string; teamId?: string; projectId?: string };
      scope = {
        org: stored.orgId ?? null,
        team: stored.teamId ?? null,
        project: stored.projectId ?? null,
      };
    }
  } catch {
    // unreadable: nothing to migrate
  }
  return { language: storedLocale(), scope };
}

/** The document the legacy localStorage values would make, or `null` when there are none. */
export function legacyDocument(
  base: UserPreferences,
  legacy: LegacyValues = readLegacy(),
): UserPreferences | null {
  if (!legacy.language && !legacy.scope.org) return null;
  return {
    ...base,
    language: legacy.language,
    default_org_id: legacy.scope.org,
    default_team_id: legacy.scope.team,
    default_project_id: legacy.scope.project,
  };
}

/**
 * Applies the document on the page: language, then the chart time zone.
 *
 * Kept apart from the provider so the same step runs for the cached copy and
 * for the server's answer.
 */
function applyPreferences(doc: UserPreferences): void {
  setChartTimeZone(doc.chart_time_zone);
  void applyLanguagePreference(doc.language);
}

export function PreferencesProvider({ children }: { children: React.ReactNode }) {
  const auth = useOptionalAuth();
  const signedIn = !!auth?.token;
  const queryClient = useQueryClient();

  // read once: a cached copy of another account's document must not seed the
  // query after a sign-out, which `clearCachedPreferences` below takes care of
  const [cached] = React.useState(() => (signedIn ? readCachedPreferences() : undefined));
  // the cached copy paints first. the zone and language are normally already in
  // place (format.ts and i18n read the same cache at load); this covers a
  // session that began after load, and runs before the browser paints
  React.useLayoutEffect(() => {
    if (cached) applyPreferences(preferencesDocument(cached));
  }, [cached]);
  const query = useQuery({
    queryKey: PREFERENCES_QUERY_KEY,
    queryFn: fetchPreferences,
    enabled: signedIn,
    retry: false,
    // the cached copy is a first paint, never a fresh answer: epoch zero makes
    // it stale at once, so the mount fetches and the server's values win
    initialData: cached,
    initialDataUpdatedAt: 0,
  });

  const unavailable = isOpenModeNoSession(query.error);

  // sign-out (or an expired session): drop the cached document and the
  // selection made under it, and go back to the browser's own defaults
  const wasSignedIn = React.useRef(signedIn);
  React.useEffect(() => {
    if (wasSignedIn.current && !signedIn) {
      clearCachedPreferences();
      // the stored locale is the same account's first-paint copy of `language`
      clearStoredLocale();
      try {
        localStorage.removeItem(LEGACY_SCOPE_KEY);
      } catch {
        // nothing to drop
      }
      queryClient.removeQueries({ queryKey: PREFERENCES_QUERY_KEY });
      setChartTimeZone(null);
    }
    wasSignedIn.current = signedIn;
  }, [signedIn, queryClient]);

  // first paint: the cached zone is already in the formatters (format.ts reads
  // the same cache at load), and the cached language is the stored locale
  // `i18n` booted with. what remains is the server's word
  const answered = query.dataUpdatedAt > 0 ? query.data : undefined;
  const migrating = React.useRef(false);
  React.useEffect(() => {
    if (!answered) return;
    const doc = preferencesDocument(answered);
    if (isEmptyDocument(doc)) {
      // one-time move of the localStorage language and scope into the server
      // document, only while that document is empty so it can never overwrite
      // a choice made anywhere else
      const seeded = legacyDocument(doc);
      if (seeded) {
        // a second pass (strict mode, a refetch) while the first is in flight
        if (migrating.current) return;
        migrating.current = true;
        void putPreferences(seeded)
          .then((saved) => {
            writeCachedPreferences(saved);
            queryClient.setQueryData(PREFERENCES_QUERY_KEY, saved, { updatedAt: Date.now() });
          })
          .catch(() => {
            // the copy stays in this browser, and the next load tries again;
            // applying the empty document now would erase the language it holds
            migrating.current = false;
          });
        return;
      }
    }
    writeCachedPreferences(answered);
    applyPreferences(doc);
  }, [answered, queryClient]);

  const { data, isPending, error, refetch } = query;
  const value = React.useMemo<PreferencesState>(
    () => ({
      preferences: data,
      isPending: signedIn && isPending,
      error,
      unavailable,
      refetch: () => void refetch(),
      save: (patch) => savePreferences(queryClient, patch),
    }),
    [data, isPending, error, refetch, signedIn, unavailable, queryClient],
  );
  return <PreferencesContext.Provider value={value}>{children}</PreferencesContext.Provider>;
}

/** The preferences, or `null` outside a provider (stories, the login screen). */
export function useOptionalPreferences(): PreferencesState | null {
  return React.useContext(PreferencesContext);
}

/**
 * Save `patch` over the account's document.
 *
 * `PUT` replaces the whole document, so a save built from a stale copy would
 * wipe a key another tab or screen had just set. The document is read again
 * first and the patch laid over *that*; the full merged object is what is sent.
 */
export async function savePreferences(
  queryClient: ReturnType<typeof useQueryClient>,
  patch: Partial<UserPreferences>,
): Promise<PreferencesResponse> {
  const latest = await fetchPreferences();
  const saved = await putPreferences({ ...preferencesDocument(latest), ...patch });
  queryClient.setQueryData(PREFERENCES_QUERY_KEY, saved, { updatedAt: Date.now() });
  return saved;
}

/** Which field a refused save names, from the 400's message, if it names one. */
export function refusedField(
  error: unknown,
): "language" | "default_playground_model" | "chart_time_zone" | null {
  if (!(error instanceof ApiError) || error.status !== 400) return null;
  for (const field of ["language", "default_playground_model", "chart_time_zone"] as const) {
    if (error.message.includes(field)) return field;
  }
  return null;
}
