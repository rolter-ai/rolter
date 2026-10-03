// translated copy for a failed control-plane call (#2216).
//
// the control plane's own `message` is English and sometimes internal ("insufficient
// role for this resource"), so an inline error never leads with it. a stable `code`
// is read first, then the HTTP status, and only a message neither can explain is
// shown at all — tucked below a generic translated line as `detail`

import type { TFunction } from "i18next";

import { ApiError } from "@/lib/api";
import { GatewayError } from "@/lib/gateway";

/** the `code`s the control plane sends that the dashboard has a sentence for */
export const KNOWN_ERROR_CODES = [
  "analytics_query_failed",
  "internal",
  "invalid_credentials",
  "invalid_cursor",
  "invalid_exchange_code",
  "invalid_field",
  "invalid_query",
  "invalid_time_bound",
  "last_org_admin",
  "last_superadmin",
  "mfa_enrolment_required",
  "name_taken",
  "no_such_endpoint",
  "open_mode_no_session",
  "password_login_disabled",
  "referenced",
  "scope_mismatch",
  "too_many_attempts",
  "unauthenticated",
] as const;

export interface ErrorCopy {
  /** what went wrong, in the dashboard's language */
  message: string;
  /** the control plane's own words, only where nothing translated explains them */
  detail?: string;
}

const known = new Set<string>(KNOWN_ERROR_CODES);

// codes whose translated line cannot say which input was wrong, so the server's
// words stay underneath as detail: an `invalid_field` message names the field
// and the rule it broke (#2567)
const keepsDetail = new Set<string>(["invalid_field"]);

/**
 * The inline copy for a failure: a translated line, plus the raw server message
 * as `detail` when the code and the status were both unknown to the dashboard.
 */
export function describeError(error: unknown, t: TFunction): ErrorCopy {
  // the gateway's message is the upstream model's own words, which is what the
  // operator came to read, so it is kept as detail under a translated lead
  if (error instanceof GatewayError) {
    if (error.status === 401) return { message: t("errors.api.unauthorized") };
    return { message: t("errors.api.generic"), detail: error.message || undefined };
  }
  if (!(error instanceof ApiError)) return { message: t("errors.api.unreachable") };
  if (error.code && known.has(error.code)) {
    const message = t(`errors.api.codes.${error.code}`);
    if (keepsDetail.has(error.code)) return { message, detail: error.message || undefined };
    return { message };
  }
  if (error.status === 401) return { message: t("errors.api.unauthorized") };
  if (error.status === 403) return { message: t("errors.api.forbidden") };
  if (error.status === 429) return { message: t("errors.api.rateLimited") };
  if (error.status >= 500) return { message: t("errors.api.server") };
  return { message: t("errors.api.generic"), detail: error.message || undefined };
}
