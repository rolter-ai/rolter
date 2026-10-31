import { type AnalyticsWindow, getAnalytics, windowParams } from "./analytics";
import { type BudgetSpendRow, type BudgetSpendStatus } from "./budgets";
import { type DataEnvelope, getJson, sendJson } from "./request";
import { type MembershipRow, type UserRow } from "./users";
import { type KeyBudgetCap, type KeyRateCap, type VirtualKeyRow } from "./virtual-keys";

/**
 * A membership as `/auth/me` serialises it (rolter-store `Membership`).
 *
 * Same row as [`MembershipRow`], with `source` always present.
 */
export interface MeMembership extends MembershipRow {
  source: string;
  /**
   * The org and team the membership sits under. A project membership names
   * neither on its own row; the scope switcher needs both to land a project
   * member on their own project (#1846). Absent from a control plane older
   * than that.
   */
  scope_org_id?: string | null;
  scope_team_id?: string | null;
}

/** `{user, memberships}` — crates/rolter-control/src/auth.rs `MeResponse` */
export interface MeResponse {
  user: UserRow;
  memberships: MeMembership[];
  /**
   * The saved Client Settings public base URL, or null. Readable by every role,
   * unlike `client_settings:read` (#2512). Absent from an older control plane.
   */
  gateway_base_url?: string | null;
  /**
   * A SCIM directory owns `user.display_name`, so a change to it answers 409.
   * The bio stays editable. Absent from a control plane older than #1823.
   */
  display_name_managed?: boolean;
  /**
   * `false` for an account that signs in through single sign-on only: it has no
   * local password, and changing one answers `409 no_local_password` (#2804).
   * Absent from a control plane older than that, which reads as `true`.
   */
  has_local_password?: boolean;
}

/** longest display name, in characters (`users_display_name_shape`) */
export const MAX_DISPLAY_NAME_LEN = 80;

/** longest bio, in characters (`users_bio_shape`) */
export const MAX_BIO_LEN = 500;

/**
 * `PATCH /me/profile` body. Send only the fields being changed: an omitted
 * field is left alone, `null` clears one, and whitespace-only is a 400.
 */
export interface ProfileUpdate {
  display_name?: string | null;
  bio?: string | null;
}

/** what `PATCH /me/profile` answers: the profile as stored, trimmed */
export interface ProfileResult {
  display_name: string | null;
  bio: string | null;
  display_name_managed: boolean;
}

/** Set the signed-in account's own display name and bio (#1823). */
export function updateMyProfile(body: ProfileUpdate): Promise<ProfileResult> {
  return sendJson<ProfileResult>("PATCH", "/api/v1/me/profile", body);
}

/**
 * Who the stored session token belongs to.
 *
 * The shell calls this on boot: a 200 says the token is still live and carries
 * the account's superadmin flag and memberships straight from the server, a
 * 401 says it is dead and the session is cleared (#1196). Not mounted at all
 * on an open-mode control plane with no store, which answers 404 — a
 * deployment shape, not a rejected session.
 */
export function fetchMe(): Promise<MeResponse> {
  return getJson<MeResponse>("/api/v1/auth/me");
}

/** shortest password the control plane accepts, in characters (`hash_password`) */
export const MIN_PASSWORD_LEN = 8;

/** `POST /auth/password` body: both fields are required and no others are taken */
export interface ChangePasswordInput {
  current_password: string;
  new_password: string;
}

/**
 * Change the signed-in account's own password (#2804).
 *
 * A wrong `current_password` is a `400 invalid_field` naming that field, never a
 * `401`: the session is fine, and a 401 would sign the dashboard out. It counts
 * against the same failed-attempt budget as a sign-in, so a spent one answers
 * `429 too_many_attempts` with the wait in `Retry-After`. The account's other
 * sessions end and the answer says how many; this one stays.
 */
export function changeMyPassword(
  input: ChangePasswordInput,
): Promise<{ sessions_revoked: number }> {
  return sendJson<{ sessions_revoked: number }>("POST", "/api/v1/auth/password", input);
}

// --- second factor (crates/rolter-control/src/mfa.rs, #1078) ---

// a key the current user owns, enriched with its project/org names; never
// carries the key hash
export interface OwnedKeyRow {
  id: string;
  project_id: string;
  project_name: string;
  org_name: string;
  key_prefix: string;
  name?: string | null;
  models: string[];
  disabled: boolean;
  expires_at?: string | null;
  /// see `VirtualKeyRow.purpose` — `playground` marks a self-expiring key the
  /// Playground minted for one sitting
  purpose?: string | null;
  created_at: string;
}

// returned from mint/rotate — carries the plaintext secret, shown once
export interface MintedKey extends VirtualKeyRow {
  key: string;
}

export interface MintKeyInput {
  /** required: the plaintext is shown once, so an unnamed key is
   *  unattributable forever after (#945). the server rejects a blank one */
  name: string;
  models?: string[];
  providers?: string[];
  cache?: boolean | null;
  /** key lifetime in days. omitted means "never expires" — a choice the
   *  operator makes deliberately, never the result of leaving a field alone */
  expires_in_days?: number;
  /** optional spend cap, written with the key (#1171) */
  budget?: KeyBudgetCap;
  /** optional rate cap, written with the key (#1171) */
  rate_limit?: KeyRateCap;
}

export interface MyUsageRow {
  virtual_key_id: string;
  requests: number | string;
  tokens: number | string;
  cost_usd: number | string;
  errors: number | string;
}

/** the screens a saved filter preset can belong to (#1825) */
export type SavedViewSurface = "llm_logs" | "dashboard";

/**
 * The filter set a preset holds. Keys mirror the query parameters the screen
 * sends; the control plane refuses any it does not allow-list for the surface.
 */
export interface SavedViewFilters {
  /** a window's name, or `custom` with the days in `since` and `until` (#3407) */
  window?: string;
  /** the first day of a `custom` window, `YYYY-MM-DD` */
  since?: string;
  /** the last day of a `custom` window, `YYYY-MM-DD`, itself included */
  until?: string;
  status?: string;
  model?: string;
  key?: string;
  business_unit?: string[];
  customer?: string[];
  bucket?: string;
}

/** a filter entry the caller can no longer read, so it was left out of `effective_filters` */
export interface SavedViewUnavailable {
  filter: "key" | "business_unit" | "customer";
  id: string;
}

export interface SavedView {
  id: string;
  surface: SavedViewSurface;
  name: string;
  /** as stored; never apply this one */
  filters: SavedViewFilters;
  /** `filters` without what `unavailable` names: what to apply */
  effective_filters: SavedViewFilters;
  unavailable: SavedViewUnavailable[];
  created_at: string;
  updated_at: string;
}

const SAVED_VIEWS = "/api/v1/me/saved-views";

/** the caller's presets for one screen, oldest first */
export function fetchSavedViews(surface: SavedViewSurface): Promise<SavedView[]> {
  return getJson<SavedView[]>(`${SAVED_VIEWS}?surface=${surface}`);
}

export function createSavedView(input: {
  surface: SavedViewSurface;
  name: string;
  filters: SavedViewFilters;
}): Promise<SavedView> {
  return sendJson<SavedView>("POST", SAVED_VIEWS, input);
}

/** a `filters` value replaces the whole stored set */
export function updateSavedView(
  id: string,
  patch: { name?: string; filters?: SavedViewFilters },
): Promise<SavedView> {
  return sendJson<SavedView>("PATCH", `${SAVED_VIEWS}/${id}`, patch);
}

export function deleteSavedView(id: string): Promise<void> {
  return sendJson<void>("DELETE", `${SAVED_VIEWS}/${id}`);
}

export function fetchMyKeys(): Promise<OwnedKeyRow[]> {
  return getJson<OwnedKeyRow[]>("/api/v1/me/virtual-keys");
}

export function mintMyKey(projectId: string, input: MintKeyInput): Promise<MintedKey> {
  return sendJson<MintedKey>("POST", `/api/v1/me/projects/${projectId}/virtual-keys`, input);
}

/**
 * The `purpose` the control plane stamps on a key the Playground minted, so a
 * row can say what it is instead of leaving a reader to infer it from a short
 * expiry (`PLAYGROUND_PURPOSE` in crates/rolter-control/src/me.rs).
 */
export const PLAYGROUND_PURPOSE = "playground";

/**
 * Mint the Playground's own key for `projectId` (#944).
 *
 * Takes no input on purpose: the server scopes the key to the routes the
 * project actually has and fixes its lifetime at half an hour, so the browser
 * cannot ask for a wider or longer-lived key than the one it is handed. A
 * project with no routes yet gets a key that reaches the built-in `fake-llm`
 * alone, never an empty list, which would reach every model (#2300).
 */
export function mintPlaygroundKey(projectId: string): Promise<MintedKey> {
  return sendJson<MintedKey>("POST", `/api/v1/me/projects/${projectId}/playground-key`);
}

export function rotateMyKey(id: string): Promise<MintedKey> {
  return sendJson<MintedKey>("POST", `/api/v1/me/virtual-keys/${id}/rotate`);
}

export function deleteMyKey(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/me/virtual-keys/${id}`);
}

// per-key usage/spend over the window; throws AnalyticsUnavailableError (503)
// when the deployment has no ClickHouse configured
export function fetchMyUsage(window: AnalyticsWindow = {}): Promise<MyUsageRow[]> {
  return getAnalytics<DataEnvelope<MyUsageRow>>(`/api/v1/me/usage${windowParams(window)}`).then(
    (r) => r.data,
  );
}

/** one of the signed-in account's own budgets, with what the gateway has counted (#1830) */
export interface MyBudgetRow extends BudgetSpendRow {
  /// decimal, returned as text
  limit_usd: string;
  /// as stored: `daily`, `monthly`, `total`, or a spelling such as `30d`
  period: string;
  within_org_id: string | null;
  within_project_id: string | null;
}

export interface MyRateLimitRow {
  id: string;
  rpm: number | null;
  tpm: number | null;
  within_org_id: string | null;
  within_project_id: string | null;
}

export interface MyBudgetReport {
  status: BudgetSpendStatus;
  budgets: MyBudgetRow[];
  rate_limits: MyRateLimitRow[];
}

/**
 * The budgets and rate limits that count every key the signed-in account
 * minted, with the spend so far. A body that is not a report reads as
 * unavailable, so a malformed answer never shows a budget as untouched.
 */
export async function fetchMyBudgets(): Promise<MyBudgetReport> {
  const body = await getJson<Partial<MyBudgetReport> | null>("/api/v1/me/budgets");
  if (!body || !Array.isArray(body.budgets)) {
    return { status: "unavailable", budgets: [], rate_limits: [] };
  }
  return {
    status: body.status ?? "unavailable",
    budgets: body.budgets,
    rate_limits: Array.isArray(body.rate_limits) ? body.rate_limits : [],
  };
}
