import { useTranslation } from "react-i18next";

import { DEFAULT_LOCALE, type Locale } from "./index";

// number/date/currency formatting bound to the active locale (#489). screens
// used bare `toLocaleString()` before, which silently follows the *browser*
// locale — so a ru-RU browser rendered russian separators inside an english
// panel. going through here keeps formatting and copy in the same language.

// Intl formatters are expensive to construct and immutable once built, so one
// per (locale, options) pair is cached for the life of the tab
const cache = new Map<string, Intl.NumberFormat | Intl.DateTimeFormat | Intl.RelativeTimeFormat>();

function numberFormat(locale: string, options?: Intl.NumberFormatOptions): Intl.NumberFormat {
  const key = `n:${locale}:${JSON.stringify(options ?? {})}`;
  let formatter = cache.get(key) as Intl.NumberFormat | undefined;
  if (!formatter) {
    formatter = new Intl.NumberFormat(locale, options);
    cache.set(key, formatter);
  }
  return formatter;
}

function dateFormat(locale: string, options?: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = `d:${locale}:${JSON.stringify(options ?? {})}`;
  let formatter = cache.get(key) as Intl.DateTimeFormat | undefined;
  if (!formatter) {
    formatter = new Intl.DateTimeFormat(locale, options);
    cache.set(key, formatter);
  }
  return formatter;
}

function relativeFormat(locale: string): Intl.RelativeTimeFormat {
  const key = `r:${locale}`;
  let formatter = cache.get(key) as Intl.RelativeTimeFormat | undefined;
  if (!formatter) {
    // `short` over `narrow`: russian narrow renders "3 minutes ago" as the
    // bare "-3 мин", which reads as a negative quantity rather than as the past
    formatter = new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "short" });
    cache.set(key, formatter);
  }
  return formatter;
}

/** the house short date — `medium` so `10/5` is never read as 5 October */
const DATE: Intl.DateTimeFormatOptions = { dateStyle: "medium" };

// log and audit rows are scanned as a column, so the clock is always h23: an
// AM/PM stamp sorts badly by eye and doubles the width of the cell
const CLOCK: Intl.DateTimeFormatOptions = {
  hourCycle: "h23",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
};

const STAMP: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  ...CLOCK,
};

// `fractionalSecondDigits` is an ES2021 Intl option and the tsconfig lib stops
// at ES2020, so it is attached through an assertion rather than by widening the
// lib for one field. every engine the dashboard supports honours it
const STAMP_MS = {
  ...STAMP,
  fractionalSecondDigits: 3,
} as Intl.DateTimeFormatOptions;

// how long each unit lasts, largest first — the ladder `relative()` walks
const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["day", 86_400],
  ["hour", 3600],
  ["minute", 60],
  ["second", 1],
];

export interface Formatters {
  locale: Locale;
  number: (value: number, options?: Intl.NumberFormatOptions) => string;
  /** axis and tile labels — 1234 reads as `1.2K`, not as seven characters */
  compact: (value: number) => string;
  /** money — defaults to USD, the currency the gateway prices in */
  currency: (value: number, currency?: string) => string;
  percent: (value: number, fractionDigits?: number) => string;
  /** a coarse duration in seconds, rendered in the largest unit that fits */
  duration: (seconds: number) => string;
  date: (value: Date | string | number, options?: Intl.DateTimeFormatOptions) => string;
  /** compact date+time for log rows and audit trails */
  dateTime: (value: Date | string | number) => string;
  /** the same stamp with milliseconds — request logs are ordered by them */
  dateTimeMs: (value: Date | string | number) => string;
  /** clock only, for a column whose rows all sit in the same day */
  time: (value: Date | string | number) => string;
  /** clock without seconds, for chart buckets */
  timeShort: (value: Date | string | number) => string;
  /** `3m ago` / `in 2h`, relative to `now` (defaults to the current time) */
  relative: (value: Date | string | number, now?: Date | string | number) => string;
}

function toDate(value: Date | string | number): Date | null {
  const date = value instanceof Date ? value : new Date(value);
  // a missing or malformed timestamp is a data problem, not a crash: Intl
  // throws RangeError on an invalid date and would take the whole table down
  return Number.isNaN(date.getTime()) ? null : date;
}

export function formattersFor(locale: Locale): Formatters {
  const stamp = (value: Date | string | number, options: Intl.DateTimeFormatOptions) => {
    const date = toDate(value);
    return date === null ? "" : dateFormat(locale, options).format(date);
  };
  return {
    locale,
    number: (value, options) => numberFormat(locale, options).format(value),
    compact: (value) =>
      numberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(value),
    currency: (value, currency = "USD") =>
      numberFormat(locale, {
        style: "currency",
        currency,
        // sub-cent gateway costs read as $0.00 without this
        maximumFractionDigits: Math.abs(value) < 1 ? 4 : 2,
      }).format(value),
    percent: (value, fractionDigits = 1) =>
      numberFormat(locale, {
        style: "percent",
        minimumFractionDigits: fractionDigits,
        maximumFractionDigits: fractionDigits,
      }).format(value),
    duration: (seconds) => {
      const scale: [string, number] =
        seconds >= 3600 ? ["hour", 3600] : seconds >= 60 ? ["minute", 60] : ["second", 1];
      return numberFormat(locale, {
        style: "unit",
        unit: scale[0],
        unitDisplay: "narrow",
        maximumFractionDigits: scale[1] === 1 ? 0 : 1,
      }).format(seconds / scale[1]);
    },
    date: (value, options) => stamp(value, options ?? DATE),
    dateTime: (value) => stamp(value, STAMP),
    dateTimeMs: (value) => stamp(value, STAMP_MS),
    time: (value) => stamp(value, CLOCK),
    timeShort: (value) => stamp(value, { ...CLOCK, second: undefined }),
    relative: (value, now) => {
      const date = toDate(value);
      if (date === null) return "";
      const from = now === undefined ? Date.now() : (toDate(now)?.getTime() ?? Date.now());
      const seconds = (date.getTime() - from) / 1000;
      const magnitude = Math.abs(seconds);
      const [unit, size] = UNITS.find(([, s]) => magnitude >= s) ?? UNITS[UNITS.length - 1];
      const rounded = Math.trunc(seconds / size);
      return relativeFormat(locale).format(rounded, unit);
    },
  };
}

/** formatters for the locale currently rendered — re-derives on every switch */
export function useFormat(): Formatters {
  const { i18n } = useTranslation();
  const locale = (i18n.resolvedLanguage ?? DEFAULT_LOCALE) as Locale;
  return formattersFor(locale);
}
