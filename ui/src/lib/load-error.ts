// Classification of a failed screen load (#962).
//
// Every screen used to render the same sentence — "Failed to load X." — for
// causes needing completely different responses. During the #924 dogfooding
// pass the Keys screen said "Failed to load your keys." while the real cause
// was every `/api/v1/me/*` route returning 401 (#942). The message pointed at
// key configuration; the actual cause was found afterwards by reading traces.
//
// An error that cannot separate "you are not signed in" from "the server is
// down" costs more time than no error at all, because it invites a wrong
// hypothesis and the operator spends their attention there first.

import {
  AnalyticsUnavailableError,
  ApiError,
  isEndpointNotMounted,
  isOpenModeNoSession,
} from "@/lib/api";

export type LoadErrorKind =
  /** no session, or it expired — signing in again fixes it */
  | "unauthenticated"
  /** signed in, but this role may not read this. Retrying cannot help */
  | "forbidden"
  /**
   * the control plane has no admin token, so it has no local accounts and the
   * per-user endpoints cannot be served to anyone (#942). Looks like a 401 and
   * is emphatically not one: signing in is not the fix, configuring
   * `ROLTER_ADMIN_TOKEN` is
   */
  | "openMode"
  /**
   * the control plane runs without a database, so this endpoint is not
   * mounted at all (#1204). Neither signing in nor retrying can help;
   * configuring `ROLTER_DATABASE_URL` is the fix
   */
  | "noStore"
  /**
   * the analytics endpoint is mounted and answered, and the answer was "this
   * deployment has no analytics store" — a 503 from a control plane with no
   * `clickhouse_url`, or a 404 from one too old to serve the route at all
   * (#1236). The sibling of `noStore`: same shape of answer, different setting
   * to change, so retrying and signing in are both beside the point
   */
  | "noAnalytics"
  /** the request never got an answer — wrong URL, down, CORS, offline */
  | "unreachable"
  /** the control plane answered, and the answer was a failure */
  | "server"
  /** answered with something we did not anticipate */
  | "unknown";

/**
 * What went wrong, as far as it can be told from the error alone.
 *
 * A thrown value that is not an [`ApiError`] never reached the control plane:
 * `fetch` rejects with a `TypeError` when it cannot connect, so "no status" is
 * the signal for unreachable rather than a fallback for it.
 */
export function classifyLoadError(error: unknown): LoadErrorKind {
  if (isOpenModeNoSession(error)) return "openMode";
  if (isEndpointNotMounted(error)) return "noStore";
  // thrown instead of an ApiError by the analytics fetchers, so it has to be
  // read before the "no status means it never connected" rule below
  if (error instanceof AnalyticsUnavailableError) return "noAnalytics";
  if (!(error instanceof ApiError)) return "unreachable";
  if (error.status === 401) return "unauthenticated";
  if (error.status === 403) return "forbidden";
  if (error.status >= 500) return "server";
  return "unknown";
}

/**
 * Whether retrying this could plausibly succeed without the operator changing
 * something first.
 *
 * Offering a retry that cannot work is its own small lie: it suggests the
 * failure was transient when it was a permission or a deployment setting.
 */
export function isRetryable(kind: LoadErrorKind): boolean {
  return kind === "unreachable" || kind === "server" || kind === "unknown";
}

/** Whether the operator's route out of this is the sign-in screen. */
export function needsSignIn(kind: LoadErrorKind): boolean {
  return kind === "unauthenticated";
}
