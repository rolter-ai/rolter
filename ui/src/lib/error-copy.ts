// translated copy for a failed control-plane call (#2216).
//
// the control plane's own `message` is English and sometimes internal ("insufficient
// role for this resource"), so an inline error never leads with it. a stable `code`
// is read first, then the HTTP status, and only a message neither can explain is
// shown at all — tucked below a generic translated line as `detail`

import type { TFunction } from "i18next";

import { ApiError } from "@/lib/api";
import { GatewayError, isKeyRefusal } from "@/lib/gateway";

/** the `code`s the control plane sends that the dashboard has a sentence for */
export const KNOWN_ERROR_CODES = [
  "account_deactivated",
  "account_outside_org",
  "analytics_query_failed",
  "analytics_query_timeout",
  "api_token_inactive",
  "config_export_binding_shadowed",
  "config_export_needs_org",
  "idp_error",
  "idp_verification_failed",
  "internal",
  "invalid_body",
  "invalid_credentials",
  "invalid_cursor",
  "invalid_exchange_code",
  "invalid_field",
  "invalid_query",
  "invalid_time_bound",
  "kek_unavailable",
  "last_org_admin",
  "last_superadmin",
  "mfa_enrolment_required",
  "name_taken",
  "no_local_password",
  "no_mapped_group",
  "no_such_endpoint",
  "open_mode_no_session",
  "password_login_disabled",
  "referenced",
  "request_timeout",
  "scope_mismatch",
  "sso_disabled",
  "state_expired",
  "store_timeout",
  "time_bound_out_of_range",
  "too_many_attempts",
  "too_many_buckets",
  "unauthenticated",
  "unknown_provider",
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
// and the rule it broke (#2567), an `invalid_body` one says what did not parse
// (#2729)
const keepsDetail = new Set<string>([
  "invalid_field",
  "invalid_body",
  "referenced",
  "scope_mismatch",
]);

/**
 * The inline copy for a failure: a translated line, plus the raw server message
 * as `detail` when the code and the status were both unknown to the dashboard.
 */
export function describeError(error: unknown, t: TFunction): ErrorCopy {
  if (error instanceof GatewayError) return describeGatewayError(error, t);
  if (!(error instanceof ApiError)) return { message: t("errors.api.unreachable") };
  if (error.code && known.has(error.code)) {
    // the field the refusal is about, when the control plane named one (#2735)
    const message =
      error.code === "invalid_field" && error.field
        ? t("errors.api.invalidFieldNamed", { field: error.field })
        : t(`errors.api.codes.${error.code}`);
    if (keepsDetail.has(error.code)) return { message, detail: error.message || undefined };
    return { message };
  }
  if (error.status === 401) return { message: t("errors.api.unauthorized") };
  if (error.status === 403) return { message: t("errors.api.forbidden") };
  if (error.status === 429) return { message: t("errors.api.rateLimited") };
  if (error.status >= 500) return { message: t("errors.api.server") };
  return { message: t("errors.api.generic"), detail: error.message || undefined };
}

/** the gateway's own `error.code`s that the dashboard has a sentence for */
export const KNOWN_GATEWAY_CODES = [
  "model_not_found",
  "rate_limit_exceeded",
  "insufficient_quota",
  "guardrail_blocked",
  "plugin_blocked",
  "model_unpriced",
  "provider_credential_missing",
  "upstream_auth_failed",
] as const;

const knownGateway = new Set<string>(KNOWN_GATEWAY_CODES);

/**
 * The copy for a failed call to the gateway, which is not the control plane:
 * its `401` is about the virtual key the request carried, never the dashboard
 * session, so it never says "sign in again" (#3643). Rolter's own code is read
 * first; a `401` or `403` that is not rolter refusing the key came from the
 * provider, and says so; anything else names its status. The message under it
 * is the gateway's or the upstream's own words, which is what the operator came
 * to read, so it is always kept as detail, and `requestId` lets the screen open
 * the request in LLM Logs, where the attempts are listed.
 */
export function describeGatewayError(
  error: GatewayError,
  t: TFunction,
): ErrorCopy & { requestId?: string } {
  const detail = error.message || undefined;
  const requestId = error.requestId;
  if (isKeyRefusal(error)) return { message: t("errors.gateway.keyRefused"), detail, requestId };
  if (error.code && knownGateway.has(error.code)) {
    return { message: t(`errors.gateway.codes.${error.code}`), detail, requestId };
  }
  if (error.status === 401 || error.status === 403) {
    return {
      message: t("errors.gateway.upstreamAuth", { status: error.status }),
      detail,
      requestId,
    };
  }
  if (error.status === 429) return { message: t("errors.gateway.rateLimited"), detail, requestId };
  return { message: t("errors.gateway.failed", { status: error.status }), detail, requestId };
}

/**
 * The translated copy for a refusal the dashboard has a sentence for, or `null`
 * for everything else (#3484).
 *
 * A confirmation dialog shows the control plane's own words by design, so a
 * thrown value that is not a coded `ApiError`, and one whose code is not in
 * {@link KNOWN_ERROR_CODES}, stays exactly as it was thrown. A known code is
 * translated; the server's message follows as `detail` only where
 * {@link describeError} keeps it, because the sentence cannot say which input
 * was wrong.
 */
export function describeCodedError(error: unknown, t: TFunction): ErrorCopy | null {
  if (!(error instanceof ApiError) || !error.code || !known.has(error.code)) return null;
  return describeError(error, t);
}

/**
 * `describeError` as one line, for a toast's detail or a banner: the translated
 * sentence, followed by the server's own words wherever they say something the
 * sentence cannot (which field, which rule).
 */
export function errorText(error: unknown, t: TFunction): string {
  const { message, detail } = describeError(error, t);
  return detail ? `${message} ${detail}` : message;
}
