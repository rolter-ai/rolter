import type { PreferencesResponse } from "@/lib/api";

// the last preferences answer, kept so the first paint after a reload uses the
// account's own language, zone and scope instead of waiting for the fetch. the
// server's answer replaces it the moment it arrives, so this is only ever a
// guess at the right values, never a second source of truth. it is dropped on
// sign-out: a cached document is one account's, and the next to sign in on the
// same browser must not paint with it
export const PREFERENCES_CACHE_KEY = "rolter.preferences";

export function readCachedPreferences(): PreferencesResponse | undefined {
  try {
    const raw = localStorage.getItem(PREFERENCES_CACHE_KEY);
    if (!raw) return undefined;
    const value: unknown = JSON.parse(raw);
    return value && typeof value === "object" ? (value as PreferencesResponse) : undefined;
  } catch {
    // unreadable or not json — treated as "not cached", never as a failure
    return undefined;
  }
}

export function writeCachedPreferences(value: PreferencesResponse): void {
  try {
    localStorage.setItem(PREFERENCES_CACHE_KEY, JSON.stringify(value));
  } catch {
    // storage unavailable: the next paint just waits for the fetch
  }
}

export function clearCachedPreferences(): void {
  try {
    localStorage.removeItem(PREFERENCES_CACHE_KEY);
  } catch {
    // nothing to clear when storage is unavailable
  }
}
