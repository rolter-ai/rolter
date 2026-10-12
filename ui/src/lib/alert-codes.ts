// The words the dashboard puts on the stable codes alerting returns (#3049).
//
// A history row carries `detail_code` / `detail_params` and a rule carries
// `last_error_code` / `last_error_params` (#2334). `detail` and `last_error`
// stay beside them as the English fallback, so a code this build does not know,
// a row written before codes existed, or a code whose numbers did not arrive
// reads as the control plane wrote it rather than as a blank or a catalog key.
// The codes are listed in `docs/user-docs/observability/alerting.mdx`.

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** the numbers a code's sentence interpolates; a code without an entry has none */
const DELIVERY_CODES = {
  http_ok: ["status"],
  http_error: ["status"],
  timeout: ["seconds"],
  connect_failed: [],
  delivery_failed: [],
  egress_denied: [],
  secret_unsealable: [],
  secret_invalid: [],
  client_unavailable: [],
  no_channel: [],
  channel_disabled: [],
} as const satisfies Record<string, readonly string[]>;

const RULE_ERROR_CODES = {
  clickhouse_not_configured: [],
  clickhouse_unreachable: [],
  clickhouse_timeout: [],
  query_timeout: ["seconds"],
  clickhouse_error: [],
  clickhouse_bad_response: [],
  query_failed: [],
  unknown_signal: ["signal"],
} as const satisfies Record<string, readonly string[]>;

type Params = Record<string, unknown> | null | undefined;

function coded(
  scope: "delivery" | "ruleError",
  table: Record<string, readonly string[]>,
  code: string | null | undefined,
  params: Params,
  fallback: string | null | undefined,
  t: Translate,
): string | null {
  const needs = code && Object.prototype.hasOwnProperty.call(table, code) ? table[code] : undefined;
  // a numeral missing from the params would print as a literal `{{seconds}}`
  const complete = needs?.every((name) => {
    const value = params?.[name];
    return typeof value === "string" || typeof value === "number";
  });
  if (!code || !needs || !complete) return fallback ?? null;
  return t(`pages.alerting.codes.${scope}.${code}`, params ?? {});
}

/** a history row's detail, in the dashboard's language; the English text for what it cannot say */
export function deliveryDetail(
  row: { detail: string | null; detail_code?: string | null; detail_params?: Params },
  t: Translate,
): string | null {
  return coded("delivery", DELIVERY_CODES, row.detail_code, row.detail_params, row.detail, t);
}

/** a rule's last evaluation error, in the dashboard's language; the English text for what it cannot say */
export function ruleErrorText(
  rule: {
    last_error: string | null;
    last_error_code?: string | null;
    last_error_params?: Params;
  },
  t: Translate,
): string | null {
  return coded(
    "ruleError",
    RULE_ERROR_CODES,
    rule.last_error_code,
    rule.last_error_params,
    rule.last_error,
    t,
  );
}

/** a failed Evaluate answers with the rule error's code but not its numbers */
export function evaluateErrorText(
  error: unknown,
  t: Translate,
  fallback: (error: unknown) => string | undefined,
): string | undefined {
  const code = error instanceof Error && "code" in error ? String(error.code) : undefined;
  return coded("ruleError", RULE_ERROR_CODES, code, null, null, t) ?? fallback(error);
}

/** whether a row was written for a window that held no data */
export function isNoDataRow(row: { detail_params?: Params }): boolean {
  return row.detail_params?.no_data === true;
}
