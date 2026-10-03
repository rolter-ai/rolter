import * as React from "react";
import { useTranslation } from "react-i18next";

import { readCachedPreferences } from "@/lib/preferences-cache";

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

// the time zone every date, clock and chart axis is drawn in (#2448). the
// account's `chart_time_zone` preference, or `undefined` for the browser's own
// zone. the server only checks the *shape* of a zone name, so a well-formed name
// this engine does not know must fall back to the local zone rather than throw
// a RangeError out of every formatter on every screen
let activeTimeZone: string | undefined;
const zoneListeners = new Set<() => void>();

/** `zone` if this engine knows it, else `undefined` — the browser's zone. */
export function validTimeZone(zone: string | null | undefined): string | undefined {
  if (!zone) return undefined;
  try {
    new Intl.DateTimeFormat(undefined, { timeZone: zone });
    return zone;
  } catch {
    return undefined;
  }
}

/** Set the chart time zone; an unknown or empty one means the browser's. */
export function setChartTimeZone(zone: string | null | undefined): void {
  const next = validTimeZone(zone);
  if (next === activeTimeZone) return;
  activeTimeZone = next;
  for (const listener of zoneListeners) listener();
}

export function chartTimeZone(): string | undefined {
  return activeTimeZone;
}

function subscribeTimeZone(listener: () => void): () => void {
  zoneListeners.add(listener);
  return () => {
    zoneListeners.delete(listener);
  };
}

// first paint: the cached preference, before the fetch has answered
activeTimeZone = validTimeZone(readCachedPreferences()?.chart_time_zone);

/** the house short date — `medium` so `10/5` is never read as 5 October */
const DATE: Intl.DateTimeFormatOptions = { dateStyle: "medium" };

// the day without its year, for a stamp that sits beside a clock in a narrow
// column: a named month for the same reason as `DATE`
const DAY: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" };

// numeric day, only compared against itself to tell whether two moments share a day
const DAY_KEY: Intl.DateTimeFormatOptions = { year: "numeric", month: "numeric", day: "numeric" };

// log and audit rows are scanned as a column, so the clock is always h23: an
// AM/PM stamp sorts badly by eye and doubles the width of the cell
const CLOCK: Intl.DateTimeFormatOptions = {
  hourCycle: "h23",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
};

// `fractionalSecondDigits` is an ES2021 Intl option and the tsconfig lib stops
// at ES2020, so it is attached through an assertion rather than by widening the
// lib for one field. every engine the dashboard supports honours it
const CLOCK_MS = {
  ...CLOCK,
  fractionalSecondDigits: 3,
} as Intl.DateTimeFormatOptions;

// a named month so `08/06` is never read as 8 June or 6 August, and the zone
// so the instant is unambiguous on an audit or incident timeline (#2219)
const STAMP: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "short",
  day: "numeric",
  ...CLOCK,
  timeZoneName: "short",
};

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
  /** the clock with milliseconds, for log rows that land inside one second */
  timeMs: (value: Date | string | number) => string;
  /** clock without seconds, for chart buckets */
  timeShort: (value: Date | string | number) => string;
  /**
   * The short date (`Oct 5`) a moment fell on, or `""` when it fell on the same
   * local day as `now` (defaults to the current time). For a column of clock
   * times over a window that can cross midnight: the clock is always shown, and
   * the day only on the rows where the clock alone would read as today's
   */
  dayUnlessToday: (value: Date | string | number, now?: Date | string | number) => string;
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
    if (date === null) return "";
    const zoned = activeTimeZone ? { ...options, timeZone: activeTimeZone } : options;
    return dateFormat(locale, zoned).format(date);
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
    timeMs: (value) => stamp(value, CLOCK_MS),
    timeShort: (value) => stamp(value, { ...CLOCK, second: undefined }),
    dayUnlessToday: (value, now) => {
      const date = toDate(value);
      if (date === null) return "";
      const today = (now === undefined ? null : toDate(now)) ?? new Date();
      // the same calendar day *in the chart zone*, not in the browser's
      if (stamp(date, DAY_KEY) === stamp(today, DAY_KEY)) return "";
      return stamp(date, DAY);
    },
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
  // re-render every consumer when the zone preference changes
  React.useSyncExternalStore(subscribeTimeZone, chartTimeZone, chartTimeZone);
  const locale = (i18n.resolvedLanguage ?? DEFAULT_LOCALE) as Locale;
  return formattersFor(locale);
}
