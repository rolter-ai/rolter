// What each alert signal measures, and how the dashboard reads and writes it
// (#2125).
//
// `ALERT_SIGNALS` in `api.ts` is the contract. The units come from `metric_sql`
// in `crates/rolter-control/src/alerting.rs`, and a rule's threshold and
// `last_value` are stored in the unit that query returns:
//
//   - `error_rate` is `countIf(status >= 500) / count()`, a fraction from 0 to
//     1. An operator thinks in percent, and a `5` typed for 5 % made a rule
//     that could never fire, so the form takes a percentage and sends the
//     fraction;
//   - `p95_latency_ms` is milliseconds;
//   - `spend_velocity` is `sum(cost_usd) * 3600 / window`: spend per hour
//     whatever the window, in the settlement currency (`cost_usd` is a historic
//     name, see `useCurrencyCode`);
//   - `request_volume` and `provider_health_flaps` are counts over the window,
//     not rates, and the second counts failed health events rather than flaps.
//
// Everything that turns one of those numbers into text, or text into one, goes
// through this file, so the card, the form and the request cannot disagree.

import { ALERT_SIGNALS } from "@/lib/api";
import type { Formatters } from "@/lib/i18n/format";

export type AlertSignal = (typeof ALERT_SIGNALS)[number];

/** what a signal's value is measured in */
export type AlertUnit =
  "percent" | "milliseconds" | "currencyPerHour" | "requests" | "healthFailures";

export interface AlertSignalSpec {
  unit: AlertUnit;
  /** the form's number is the API's times this: 100 for a fraction taken as a percentage */
  scale: number;
  /** where a new rule's threshold starts, in the API's unit */
  defaultThreshold: number;
  /** the highest value the signal can reach, in the API's unit; a threshold above it never fires */
  max?: number;
  /** the threshold input's `step` */
  step: number | "any";
}

export const ALERT_SIGNAL_SPECS: Record<AlertSignal, AlertSignalSpec> = {
  error_rate: { unit: "percent", scale: 100, defaultThreshold: 0.05, max: 1, step: "any" },
  p95_latency_ms: { unit: "milliseconds", scale: 1, defaultThreshold: 8000, step: 1 },
  spend_velocity: { unit: "currencyPerHour", scale: 1, defaultThreshold: 50, step: "any" },
  request_volume: { unit: "requests", scale: 1, defaultThreshold: 1000, step: 1 },
  provider_health_flaps: { unit: "healthFailures", scale: 1, defaultThreshold: 10, step: 1 },
};

export function isAlertSignal(signal: string): signal is AlertSignal {
  return (ALERT_SIGNALS as readonly string[]).includes(signal);
}

/** the signal's spec, or `null` for one the control plane added after this build */
export function signalSpec(signal: string): AlertSignalSpec | null {
  return isAlertSignal(signal) ? ALERT_SIGNAL_SPECS[signal] : null;
}

// twelve significant digits hold any threshold a person types and drop the
// binary noise a scale leaves behind: 0.07 * 100 is 7.000000000000001, and
// 1.1 / 100 is 0.011000000000000001
const clean = (value: number) => Number(value.toPrecision(12));

/** an API value as the form shows it: `0.05` is `5` for `error_rate` */
export function toFormValue(signal: string, value: number): number {
  const spec = signalSpec(signal);
  return spec ? clean(value * spec.scale) : value;
}

/** the form's number as the API takes it: `5` is `0.05` for `error_rate` */
export function fromFormValue(signal: string, value: number): number {
  const spec = signalSpec(signal);
  return spec ? clean(value / spec.scale) : value;
}

/** the text a new rule's threshold input starts from */
export function defaultThresholdInput(signal: AlertSignal): string {
  return String(toFormValue(signal, ALERT_SIGNAL_SPECS[signal].defaultThreshold));
}

/** the threshold input's `max`, in the form's unit, or `undefined` when unbounded */
export function thresholdInputMax(signal: string): number | undefined {
  const max = signalSpec(signal)?.max;
  return max === undefined ? undefined : toFormValue(signal, max);
}

/**
 * Whether the form's threshold text can be saved. The API takes any finite
 * number from 0, and a bounded signal is held to its bound too, since a
 * threshold past the highest value the signal reaches is a rule that never
 * fires.
 */
export function thresholdValid(signal: string, input: string): boolean {
  const value = Number(input);
  if (input.trim() === "" || !Number.isFinite(value) || value < 0) return false;
  const max = signalSpec(signal)?.max;
  return max === undefined || fromFormValue(signal, value) <= max;
}

/** the i18n key stating which thresholds the form takes for `signal` */
export function thresholdRangeKey(signal: string): string {
  return signalSpec(signal)?.unit === "percent"
    ? "pages.alerting.rules.thresholdRange.percent"
    : "pages.alerting.rules.thresholdRange.nonNegative";
}

type Translate = (key: string, options?: Record<string, unknown>) => string;

/** the signal's translated name, or its id for one this build does not know */
export function signalLabel(signal: string, t: Translate): string {
  return isAlertSignal(signal) ? t(`pages.alerting.signals.${signal}.label`) : signal;
}

/** what the signal counts and over what, in one line, or `null` when unknown */
export function signalDescription(signal: string, t: Translate, currency: string): string | null {
  return isAlertSignal(signal)
    ? t(`pages.alerting.signals.${signal}.description`, { currency })
    : null;
}

/** the threshold field's label, naming the unit it is typed in */
export function thresholdLabel(signal: string, t: Translate, currency: string): string {
  const spec = signalSpec(signal);
  return spec
    ? t(`pages.alerting.units.field.${spec.unit}`, { currency })
    : t("pages.alerting.rules.fieldThreshold");
}

export interface SignalFormat {
  fmt: Formatters;
  t: Translate;
  /** the settlement currency, from `useCurrencyCode` */
  currency: string;
  /** the rule's window, which a count is read over */
  windowSecs: number;
}

/**
 * A threshold or reading in the signal's unit: `5%`, `840 ms`, `$12.50/h`,
 * `340 requests in 5m`. A signal this build does not know is shown as its bare
 * number rather than under a unit guessed for it.
 */
export function formatSignalValue(
  signal: string,
  value: number,
  { fmt, t, currency, windowSecs }: SignalFormat,
): string {
  const spec = signalSpec(signal);
  if (!spec) return fmt.number(value);
  // Intl joins some units to their number with a plain space (ru `840 мс`,
  // `5 мин`), which lets a narrow card break the number from its unit
  const keep = (text: string) => text.replace(/ /g, "\u00a0");
  switch (spec.unit) {
    case "percent":
      return fmt.number(value, { style: "percent", maximumFractionDigits: 2 });
    case "milliseconds":
      return keep(
        fmt.number(value, {
          style: "unit",
          unit: "millisecond",
          unitDisplay: "short",
          maximumFractionDigits: 0,
        }),
      );
    case "currencyPerHour":
      return t("pages.alerting.units.perHour", { amount: fmt.currency(value, currency) });
    case "requests":
    case "healthFailures":
      // `count` picks the plural form and `value` is what is printed, so a
      // thousand reads with the locale's grouping rather than as `1000`
      return t(`pages.alerting.units.${spec.unit}In`, {
        count: value,
        value: fmt.number(value),
        window: keep(fmt.duration(windowSecs)),
      });
  }
}
