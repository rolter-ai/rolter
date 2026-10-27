import { getJson } from "./request";

/**
 * One subsystem this build ships as something other than stable (#1385).
 *
 * `SUBSYSTEMS` in `crates/rolter-core/src/stability.rs` is the only list;
 * `nav_keys` carries the mapping onto the dashboard's nav leaves so `ui/` never
 * keeps a second copy that can drift from it. An empty `nav_keys` is ordinary
 * and means the subsystem has no screen of its own — a gateway surface or a
 * cross-cutting concept documented rather than navigated.
 */
export interface SubsystemStability {
  id: string;
  /** never `"stable"`: the wire carries the exceptions alone */
  stability: "experimental";
  /**
   * what specifically is unfinished, one sentence, English from the build. The
   * dashboard does not render it: the rail reads `stability.notes.<id>` from
   * the catalogs so the note follows the locale (#1401)
   */
  note: string;
  /** nav leaf keys from `NAV` in `@/lib/nav` */
  nav_keys: string[];
}

/**
 * `GET /api/v1/version`: the running build and the latest stable release the
 * control plane has heard of (#902). The control plane asks GitHub once at
 * boot and every few hours; the browser never does. `latest`, `release_url`
 * and `checked_at` are null until a check has succeeded, and `enabled` is
 * false when `ROLTER_UPDATE_CHECK=false` opted the deployment out.
 */
export interface VersionStatus {
  current: string;
  latest: string | null;
  release_url: string | null;
  update_available: boolean;
  checked_at: string | null;
  enabled: boolean;
}

export function fetchVersion(): Promise<VersionStatus> {
  return getJson<VersionStatus>("/api/v1/version");
}

/**
 * `GET /api/v1/stability`: this build's experimental subsystems (#1385). Only
 * the exceptions travel — an empty array is the healthy answer — and the
 * capability behind it is held by every signed-in caller, so the nav can ask
 * for a viewer as well as an admin.
 */
export function fetchStability(): Promise<SubsystemStability[]> {
  return getJson<SubsystemStability[]>("/api/v1/stability");
}

/**
 * `GET /api/v1/public-url`: the control plane's own public base URL (#2083).
 *
 * Every address rolter hands an identity provider is built from it on the
 * server — the SSO redirect and login URLs, the SCIM base URL — never from the
 * request, so a screen that needs one before the server has built it appends
 * the path to this rather than to `window.location.origin`, which is wrong
 * behind a proxy or under a second hostname. `configured` is false when
 * `ROLTER_PUBLIC_URL` is unset and the default, `http://localhost:4001`, is
 * standing in for it. Held by every signed-in caller.
 */
export interface PublicUrl {
  /** no trailing slash, so an absolute path can be appended as-is */
  public_url: string;
  configured: boolean;
}

export function fetchPublicUrl(): Promise<PublicUrl> {
  return getJson<PublicUrl>("/api/v1/public-url");
}
