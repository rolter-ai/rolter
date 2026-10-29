import * as React from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import { useFormat } from "@/lib/i18n/format";

/**
 * The reporting windows a spend screen can be read over (#2107).
 *
 * A window is a name, not a pair of timestamps: "the last 7 days" means the
 * seven days before the request leaves, so the bounds are worked out by
 * {@link windowBounds} each time one is sent. Bounds fixed when a screen first
 * renders keep widening on a tab left open, so "the last 24 hours" would mean
 * every hour since the screen was opened (#1975).
 *
 * The values are what the address carries (`?window=last-month`).
 */
export const TIME_WINDOWS = ["24h", "7d", "30d", "mtd", "last-month"] as const;

export type TimeWindow = (typeof TIME_WINDOWS)[number];

export const DEFAULT_TIME_WINDOW: TimeWindow = "24h";

/** the query parameter a picked window is kept in */
export const TIME_WINDOW_PARAM = "window";

/** the value `window` takes in the address for a custom range, whose days ride in the next two */
export const CUSTOM_WINDOW = "custom";
/** the first day of a custom range, `YYYY-MM-DD` in the viewer's calendar */
export const WINDOW_SINCE_PARAM = "since";
/** the last day of a custom range, `YYYY-MM-DD`, itself included */
export const WINDOW_UNTIL_PARAM = "until";

/**
 * A range of whole calendar days the viewer typed (#3407): `custom:since:until`,
 * both `YYYY-MM-DD`, the last day included. Written as one string so it can be
 * a query key, compared and held wherever a window's name is, while the address
 * and a saved view keep the two days in their own fields.
 */
export type CustomWindow = `${typeof CUSTOM_WINDOW}:${string}:${string}`;

/** what a screen reports over: a named window, or a custom range of days */
export type WindowSelection = TimeWindow | CustomWindow;

/**
 * Where the last window picked in this tab is remembered, so it carries over
 * to a screen opened from the nav rail, whose links drop the query string.
 */
export const TIME_WINDOW_STORAGE_KEY = "rolter.time-window";

export function isTimeWindow(value: unknown): value is TimeWindow {
  return typeof value === "string" && (TIME_WINDOWS as readonly string[]).includes(value);
}

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** `YYYY-MM-DD` as local midnight, or null when it is not a real day */
function parseDay(raw: string | null | undefined): Date | null {
  const match = raw ? DAY.exec(raw) : null;
  if (!match) return null;
  const [year, month, day] = [Number(match[1]), Number(match[2]) - 1, Number(match[3])];
  const date = new Date(year, month, day);
  // `new Date(2026, 1, 31)` is the 3rd of March, which is not the day that was typed
  return date.getFullYear() === year && date.getMonth() === month && date.getDate() === day
    ? date
    : null;
}

/** a date as `YYYY-MM-DD` in the viewer's calendar */
export function formatDay(date: Date): string {
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(date.getFullYear(), 4)}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

const addDays = (date: Date, days: number) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);

/** whole days from `a` to `b`, counted on the calendar so a daylight-saving change is not one off */
const daysBetween = (a: Date, b: Date) =>
  Math.round(
    (Date.UTC(b.getFullYear(), b.getMonth(), b.getDate()) -
      Date.UTC(a.getFullYear(), a.getMonth(), a.getDate())) /
      (24 * HOUR),
  );

/** the two days of a custom range, as typed */
export interface DayRange {
  since: string;
  until: string;
}

/**
 * Why a pair of days is not a range: a day that is not a real date, or an end
 * before the start. `null` for one that is.
 */
export function dayRangeProblem(range: Partial<DayRange>): "invalid" | "order" | null {
  const since = parseDay(range.since);
  const until = parseDay(range.until);
  if (!since || !until) return "invalid";
  return until.getTime() < since.getTime() ? "order" : null;
}

/** The custom window over `since` to `until`, or null when they are not a range. */
export function customWindow(since: string | null, until: string | null): CustomWindow | null {
  if (!since || !until || dayRangeProblem({ since, until })) return null;
  return `${CUSTOM_WINDOW}:${since}:${until}`;
}

/** the days of a custom window, or null for a named one */
export function customRange(window: WindowSelection): DayRange | null {
  const [kind, since, until] = window.split(":");
  if (kind !== CUSTOM_WINDOW || !since || !until) return null;
  return dayRangeProblem({ since, until }) ? null : { since, until };
}

export function isCustomWindow(value: unknown): value is CustomWindow {
  return typeof value === "string" && customRange(value as WindowSelection) !== null;
}

/** a named window or a well-formed custom one */
export function isWindowSelection(value: unknown): value is WindowSelection {
  return isTimeWindow(value) || isCustomWindow(value);
}

/**
 * The selection an address holds. A custom range needs both days and an end on
 * or after the start; one that fails that reads as the default, the way an
 * unknown name does, rather than as a failed screen. With no `window` at all,
 * `fallback` applies.
 */
export function readWindowSelection(
  params: URLSearchParams,
  fallback: WindowSelection = DEFAULT_TIME_WINDOW,
): WindowSelection {
  const raw = params.get(TIME_WINDOW_PARAM);
  if (raw === null) return fallback;
  if (raw === CUSTOM_WINDOW) {
    return (
      customWindow(params.get(WINDOW_SINCE_PARAM), params.get(WINDOW_UNTIL_PARAM)) ??
      DEFAULT_TIME_WINDOW
    );
  }
  return readTimeWindow(raw);
}

/**
 * The three address parameters that say `window`, an empty string for one to
 * leave out: nothing for the default, the name for a named window, and the name
 * `custom` with its two days for a range. Written together so that picking a
 * named window drops the days a range left behind.
 */
export function windowAddress(
  window: WindowSelection,
  fallback: WindowSelection = DEFAULT_TIME_WINDOW,
): Record<
  typeof TIME_WINDOW_PARAM | typeof WINDOW_SINCE_PARAM | typeof WINDOW_UNTIL_PARAM,
  string
> {
  const range = customRange(window);
  if (range) {
    return { window: CUSTOM_WINDOW, since: range.since, until: range.until };
  }
  return { window: window === fallback ? "" : window, since: "", until: "" };
}

/**
 * The filters a saved view holds for `window` (#3407): the name, and for a
 * custom range the two days beside it. A saved range is the days, not "the
 * last N days", so it reads the same whenever the view is applied.
 */
export function windowFilters(
  window: WindowSelection,
): { window: string } | { window: typeof CUSTOM_WINDOW; since: string; until: string } {
  const range = customRange(window);
  return range ? { window: CUSTOM_WINDOW, ...range } : { window };
}

/** The selection a saved view's filters name, the default when they name none. */
export function windowFromFilters(filters: {
  window?: string;
  since?: string;
  until?: string;
}): WindowSelection {
  if (filters.window === CUSTOM_WINDOW) {
    return customWindow(filters.since ?? null, filters.until ?? null) ?? DEFAULT_TIME_WINDOW;
  }
  return readTimeWindow(filters.window);
}

/** an unknown value reads as the default window rather than as a failed screen */
export function readTimeWindow(raw: string | null | undefined): TimeWindow {
  return isTimeWindow(raw) ? raw : DEFAULT_TIME_WINDOW;
}

/**
 * The bounds as the control plane reads them: `since` inclusive, `until`
 * exclusive, and no `until` at all for a window that runs up to now, so the
 * server's clock closes it rather than the browser's.
 */
export interface WindowBounds {
  since: string;
  until?: string;
}

const HOUR = 3_600_000;

// local midnight on the first of the month `offset` months from `now`'s. the
// Date constructor carries a month of -1 or 12 into the neighbouring year, and
// a local midnight is what makes the boundary the viewer's calendar month
// rather than UTC's
function monthStart(now: Date, offset = 0): Date {
  return new Date(now.getFullYear(), now.getMonth() + offset, 1);
}

/**
 * What a custom range covers: from the start of its first day to the start of
 * the day after its last, in the viewer's calendar, since `until` is exclusive.
 */
function customBounds(range: DayRange): Required<WindowBounds> {
  const first = parseDay(range.since) as Date;
  const last = parseDay(range.until) as Date;
  return { since: first.toISOString(), until: addDays(last, 1).toISOString() };
}

/** What `window` covers at the instant `now`. */
export function windowBounds(window: WindowSelection, now: Date = new Date()): WindowBounds {
  const range = customRange(window);
  if (range) return customBounds(range);
  const hoursBack = (hours: number) => ({
    since: new Date(now.getTime() - hours * HOUR).toISOString(),
  });
  switch (window) {
    case "24h":
      return hoursBack(24);
    case "7d":
      return hoursBack(24 * 7);
    case "30d":
      return hoursBack(24 * 30);
    case "mtd":
      return { since: monthStart(now).toISOString() };
    case "last-month":
      // the whole previous month, however far into this one `now` is
      return { since: monthStart(now, -1).toISOString(), until: monthStart(now).toISOString() };
    default:
      // a custom range that is not one (cannot happen for a value read through
      // `readWindowSelection`) reads as the default rather than as no bounds
      return windowBounds(DEFAULT_TIME_WINDOW, now);
  }
}

/**
 * The window just before `window` at `now`, the same length, for a figure to
 * be compared with (#2010). A rolling window steps back by its own length; the
 * month so far is compared with the same stretch of last month, so the 10th is
 * measured against the first ten days of the month before and not against a
 * whole month; last month is compared with the month before it. The previous
 * window always has an `until`, since it never runs up to now.
 */
export function previousWindowBounds(
  window: WindowSelection,
  now: Date = new Date(),
): Required<WindowBounds> {
  const range = customRange(window);
  if (range) {
    // the same number of days straight before the first: a range of three days
    // is compared with the three days that precede it
    const first = parseDay(range.since) as Date;
    const days = daysBetween(first, parseDay(range.until) as Date) + 1;
    return { since: addDays(first, -days).toISOString(), until: first.toISOString() };
  }
  const hoursBack = (hours: number) => ({
    since: new Date(now.getTime() - 2 * hours * HOUR).toISOString(),
    until: new Date(now.getTime() - hours * HOUR).toISOString(),
  });
  switch (window) {
    case "24h":
      return hoursBack(24);
    case "7d":
      return hoursBack(24 * 7);
    case "30d":
      return hoursBack(24 * 30);
    case "mtd": {
      const since = monthStart(now, -1);
      // the same wall-clock moment a month back, so a daylight-saving change
      // between the two does not shift it by an hour. a 31st has no
      // counterpart in a 30-day month and the Date constructor would carry it
      // into this one, so the comparison stops at last month's end
      const sameMoment = new Date(
        now.getFullYear(),
        now.getMonth() - 1,
        now.getDate(),
        now.getHours(),
        now.getMinutes(),
        now.getSeconds(),
        now.getMilliseconds(),
      );
      const until = Math.min(sameMoment.getTime(), monthStart(now).getTime());
      return { since: since.toISOString(), until: new Date(until).toISOString() };
    }
    case "last-month":
      return {
        since: monthStart(now, -2).toISOString(),
        until: monthStart(now, -1).toISOString(),
      };
    default:
      return previousWindowBounds(DEFAULT_TIME_WINDOW, now);
  }
}

/**
 * The first and the last instant `bounds` covered, for a caption. A window
 * with no `until` ran up to `at`, the moment it was sent; an exclusive `until`
 * ends a millisecond before itself, so last month closes on its last day
 * rather than on the first of this one.
 */
export function windowSpan(bounds: WindowBounds, at: Date): { from: Date; to: Date } {
  return {
    from: new Date(bounds.since),
    to: bounds.until ? new Date(Date.parse(bounds.until) - 1) : at,
  };
}

// session storage rather than local: two tabs can read two different months,
// and a fresh tab starts from the default. either store can be unavailable
// (private mode, a sandboxed frame), and then the window simply is not carried
function remembered(): WindowSelection | null {
  try {
    const raw = sessionStorage.getItem(TIME_WINDOW_STORAGE_KEY);
    return isWindowSelection(raw) ? raw : null;
  } catch {
    return null;
  }
}

function remember(window: WindowSelection): void {
  try {
    sessionStorage.setItem(TIME_WINDOW_STORAGE_KEY, window);
  } catch {
    // not carried over; the address still holds it
  }
}

/**
 * The window a screen reports over, kept in the address and carried between
 * screens.
 *
 * The address wins when it names one, so a link or a reload comes back to the
 * same window. With no `window` parameter the last pick in this tab applies and
 * is written back, so the address names what the screen shows. Every write
 * replaces the history entry, the way the LLM Logs filters do, so the back
 * button leaves the screen instead of stepping through each pick.
 */
export function useTimeWindow(): [WindowSelection, (next: WindowSelection) => void] {
  const [params, setParams] = useSearchParams();
  const raw = params.get(TIME_WINDOW_PARAM);
  const window = raw === null ? (remembered() ?? DEFAULT_TIME_WINDOW) : readWindowSelection(params);

  const write = React.useCallback(
    (next: WindowSelection) =>
      setParams(
        (prev) => {
          // the other parameters are left alone, since the address is not only the picker's
          const out = new URLSearchParams(prev);
          for (const [name, value] of Object.entries(windowAddress(next))) {
            if (value) out.set(name, value);
            else out.delete(name);
          }
          return out;
        },
        { replace: true },
      ),
    [setParams],
  );

  React.useEffect(() => {
    remember(window);
    if (raw === null && window !== DEFAULT_TIME_WINDOW) write(window);
  }, [raw, window, write]);

  const pick = React.useCallback(
    (next: WindowSelection) => {
      // remembered before the address changes: picking the default empties the
      // parameter, and an address with none reads the remembered window, which
      // would otherwise still be the one just replaced
      remember(next);
      write(next);
    },
    [write],
  );
  return [window, pick];
}

/** The picker's options, in the order {@link TIME_WINDOWS} lists them. */
export function useTimeWindowOptions(): { value: TimeWindow; label: string }[] {
  const { t } = useTranslation();
  const labels: Record<TimeWindow, string> = {
    "24h": t("common.timeWindow.last24h"),
    "7d": t("common.timeWindow.last7d"),
    "30d": t("common.timeWindow.last30d"),
    mtd: t("common.timeWindow.monthToDate"),
    "last-month": t("common.timeWindow.lastMonth"),
  };
  return TIME_WINDOWS.map((value) => ({ value, label: labels[value] }));
}

/**
 * The picker's options with the custom range last, for a screen that draws
 * {@link TimeWindowPicker}. The plain {@link useTimeWindowOptions} stays the
 * named windows, for a screen that has not adopted a range (#3407).
 */
export function useWindowSelectionOptions(): {
  value: TimeWindow | typeof CUSTOM_WINDOW;
  label: string;
}[] {
  const { t } = useTranslation();
  return [
    ...useTimeWindowOptions(),
    { value: CUSTOM_WINDOW, label: t("common.timeWindow.custom") },
  ];
}

/**
 * A selection as a phrase: a named window's label, or a custom range's two
 * days (`Oct 1, 2026 – Oct 7, 2026`), for a caption or a chart's name.
 */
export function useWindowLabel(): (window: WindowSelection) => string {
  const { t } = useTranslation();
  const fmt = useFormat();
  const named = useTimeWindowOptions();
  return (window) => {
    const range = customRange(window);
    if (range) {
      return t("common.timeWindow.rangeLabel", {
        from: fmt.calendarDay(range.since),
        to: fmt.calendarDay(range.until),
      });
    }
    return named.find((o) => o.value === window)?.label ?? window;
  };
}

/**
 * The window a screen reads, kept in the address and nowhere else.
 *
 * {@link useTimeWindow} also remembers the pick for the next screen opened from
 * the nav rail. LLM Logs and the Dashboard do not: a saved view (#2452) names
 * its window, and a window carried in from another screen would make the same
 * address read two ways. An unknown value reads as the default.
 */
export function useAddressWindow(): [TimeWindow, (next: TimeWindow) => void] {
  const [params, setParams] = useSearchParams();
  const window = readTimeWindow(params.get(TIME_WINDOW_PARAM));
  const pick = React.useCallback(
    (next: TimeWindow) =>
      setParams(
        (prev) => {
          const out = new URLSearchParams(prev);
          if (next === DEFAULT_TIME_WINDOW) out.delete(TIME_WINDOW_PARAM);
          else out.set(TIME_WINDOW_PARAM, next);
          return out;
        },
        { replace: true },
      ),
    [setParams],
  );
  return [window, pick];
}

/**
 * {@link useAddressWindow} for a screen that takes a custom range (#3407): the
 * selection is read from `window`, `since` and `until`, and a pick writes all
 * three, so a named window drops the days a range left in the address.
 */
export function useAddressSelection(
  fallback: WindowSelection = DEFAULT_TIME_WINDOW,
): [WindowSelection, (next: WindowSelection) => void] {
  const [params, setParams] = useSearchParams();
  const window = readWindowSelection(params, fallback);
  const pick = React.useCallback(
    (next: WindowSelection) =>
      setParams(
        (prev) => {
          const out = new URLSearchParams(prev);
          for (const [name, value] of Object.entries(windowAddress(next, fallback))) {
            if (value) out.set(name, value);
            else out.delete(name);
          }
          return out;
        },
        { replace: true },
      ),
    [setParams, fallback],
  );
  return [window, pick];
}
