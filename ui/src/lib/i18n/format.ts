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

/** the chart time zone, re-rendering the caller whenever it changes */
export function useChartTimeZone(): string | undefined {
  return React.useSyncExternalStore(subscribeTimeZone, chartTimeZone, chartTimeZone);
}

// first paint: the cached preference, before the fetch has answered
activeTimeZone = validTimeZone(readCachedPreferences()?.chart_time_zone);

/** a wall-clock reading in some zone; `month` counts from 0, as `Date`'s does */
export interface WallClock {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
  ms: number;
}

const PART_FORMATS = new Map<string, Intl.DateTimeFormat>();

/**
 * What the clock on the wall reads at `instant` in `zone`, the browser's when
 * `undefined`: the calendar arithmetic of a reporting window counts days in the
 * chart zone with it (#3500).
 */
export function wallClock(instant: Date, zone: string | undefined): WallClock {
  if (!zone) {
    return {
      year: instant.getFullYear(),
      month: instant.getMonth(),
      day: instant.getDate(),
      hour: instant.getHours(),
      minute: instant.getMinutes(),
      second: instant.getSeconds(),
      ms: instant.getMilliseconds(),
    };
  }
  let format = PART_FORMATS.get(zone);
  if (!format) {
    format = new Intl.DateTimeFormat("en-US", {
      timeZone: zone,
      hourCycle: "h23",
      year: "numeric",
      month: "numeric",
      day: "numeric",
      hour: "numeric",
      minute: "numeric",
      second: "numeric",
    });
    PART_FORMATS.set(zone, format);
  }
  const parts = format.formatToParts(instant);
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    Number(parts.find((p) => p.type === type)?.value ?? 0);
  return {
    year: part("year"),
    month: part("month") - 1,
    day: part("day"),
    hour: part("hour"),
    minute: part("minute"),
    second: part("second"),
    ms: instant.getMilliseconds(),
  };
}

/** the zone the browser itself reads clocks in */
export function browserTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

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
const STAMP_DATE: Intl.DateTimeFormatOptions = { year: "numeric", month: "short", day: "numeric" };

const STAMP_CLOCK: Intl.DateTimeFormatOptions = { ...CLOCK, timeZoneName: "short" };

const STAMP_CLOCK_MS = {
  ...STAMP_CLOCK,
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
  /**
   * A calendar day written `YYYY-MM-DD` (a custom range's start or end), as a
   * date. It names a day, not an instant, so the chart zone does not move it
   * to its neighbour; an unreadable value comes back as it was
   */
  calendarDay: (value: string) => string;
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
  // the date and the clock are formatted apart and joined here: one formatter
  // takes its connector from the runtime's ICU data (`,` in older CLDR, `at` in
  // newer), so the same instant would read differently per browser (#2953)
  const joined = (value: Date | string | number, clock: Intl.DateTimeFormatOptions) => {
    const day = stamp(value, STAMP_DATE);
    return day === "" ? "" : `${day}, ${stamp(value, clock)}`;
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
    dateTime: (value) => joined(value, STAMP_CLOCK),
    dateTimeMs: (value) => joined(value, STAMP_CLOCK_MS),
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
    calendarDay: (value) => {
      const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
      if (!match) return value;
      // noon UTC, formatted in UTC: the day reads the same in every zone
      const noon = Date.UTC(Number(match[1]), Number(match[2]) - 1, Number(match[3]), 12);
      return dateFormat(locale, { ...DATE, timeZone: "UTC" }).format(noon);
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
