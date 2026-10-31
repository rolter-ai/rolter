import { getJson, sendJson } from "./request";

/**
 * The self-service preferences document (`/api/v1/me/preferences`,
 * `crates/rolter-control/src/me.rs`). A `null` key means "use the default":
 * the browser's language and zone, the Playground's own pick.
 */
export interface UserPreferences {
  language: string | null;
  default_org_id: string | null;
  default_team_id: string | null;
  default_project_id: string | null;
  default_playground_model: string | null;
  chart_time_zone: string | null;
}

/** The scope the control plane says the account can still read, computed on every GET. */
export interface EffectiveDefaultScope {
  org_id: string | null;
  team_id: string | null;
  project_id: string | null;
}

/**
 * `GET`/`PUT` answer: the document plus `effective_default_scope`. The dashboard
 * opens on the latter and never on the raw `default_*_id` keys, which can name a
 * scope the account lost access to.
 */
export interface PreferencesResponse extends UserPreferences {
  effective_default_scope: EffectiveDefaultScope | null;
}

export const PREFERENCE_KEYS = [
  "language",
  "default_org_id",
  "default_team_id",
  "default_project_id",
  "default_playground_model",
  "chart_time_zone",
] as const satisfies readonly (keyof UserPreferences)[];

/** The six stored keys of a response, without the computed scope. */
export function preferencesDocument(source: UserPreferences): UserPreferences {
  return {
    language: source.language ?? null,
    default_org_id: source.default_org_id ?? null,
    default_team_id: source.default_team_id ?? null,
    default_project_id: source.default_project_id ?? null,
    default_playground_model: source.default_playground_model ?? null,
    chart_time_zone: source.chart_time_zone ?? null,
  };
}

export function fetchPreferences(): Promise<PreferencesResponse> {
  return getJson<PreferencesResponse>("/api/v1/me/preferences");
}

/**
 * Replaces the whole document: a key left out is cleared, and the computed
 * `effective_default_scope` is refused with a 400. Everything goes through
 * `preferencesDocument` so neither mistake can be made by a caller.
 */
export function putPreferences(document: UserPreferences): Promise<PreferencesResponse> {
  return sendJson<PreferencesResponse>(
    "PUT",
    "/api/v1/me/preferences",
    preferencesDocument(document),
  );
}
