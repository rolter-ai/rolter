import * as React from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import {
  chartTimeZone,
  useChartTimeZone,
  useFormat,
  wallClock,
  type WallClock,
} from "@/lib/i18n/format";

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
 *
 * A calendar day is a day in the chart zone (#3500): the zone the charts,
 * tables and clocks are drawn in (`chart_time_zone`, `chartTimeZone()`), or the
 * browser's when the account sets none. "Month to date", "last month" and a
 * custom range therefore start and end at the midnights the charts label. Each
 * function takes the zone as its last argument, the chart zone by default.
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

/** the first year a custom range may name; `me_saved_views.rs` holds the server to it too */
const MIN_DAY_YEAR = 2000;

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** a calendar day, with no zone: `month` counts from 0 */
type CalendarDay = Pick<WallClock, "year" | "month" | "day">;

/**
 * The instant the clock in `zone` reads `at`, the browser's zone when
 * `undefined`. A field past its range carries over the way `Date`'s
 * constructor does (a month of -1 is December before), and a reading a
 * daylight-saving change skips lands just after the gap, as `Date` does too.
 */
function zonedInstant(at: Partial<WallClock> & CalendarDay, zone: string | undefined): Date {
  const { year, month, day, hour = 0, minute = 0, second = 0, ms = 0 } = at;
  if (!zone) return new Date(year, month, day, hour, minute, second, ms);
  const wall = Date.UTC(year, month, day, hour, minute, second, ms);
  // the zone's offset at an instant: the wall clock there, read as UTC, minus it
  const offsetAt = (instant: number) => {
    const c = wallClock(new Date(instant), zone);
    return Date.UTC(c.year, c.month, c.day, c.hour, c.minute, c.second, c.ms) - instant;
  };
  // the offset at a first guess may not be the offset at the answer when a
  // change falls between them, so a second guess uses the first's. the earlier
  // of the two that reads `at` back wins, as `Date` picks the first of an hour
  // that happens twice; when neither does, `at` is in a gap and the later one
  // is just past it
  const first = wall - offsetAt(wall);
  const second_ = wall - offsetAt(first);
  const reads = (instant: number) => instant + offsetAt(instant) === wall;
  const [early, late] = first < second_ ? [first, second_] : [second_, first];
  if (reads(early)) return new Date(early);
  return new Date(late);
}

/** `YYYY-MM-DD` as the day it names, or null when it is not a real day */
function parseDay(raw: string | null | undefined): CalendarDay | null {
  const match = raw ? DAY.exec(raw) : null;
  if (!match) return null;
  const [year, month, day] = [Number(match[1]), Number(match[2]) - 1, Number(match[3])];
  // the server holds a saved range to the same years (`MIN_DAY_YEAR`), and
  // `Date.UTC(99, …)` is 1999 besides, so an earlier year is not a day here
  if (year < MIN_DAY_YEAR) return null;
  const date = new Date(Date.UTC(year, month, day));
  // the 31st of February is the 3rd of March, which is not the day that was typed
  return date.getUTCFullYear() === year && date.getUTCMonth() === month && date.getUTCDate() === day
    ? { year, month, day }
    : null;
}

/** the day `instant` falls on in `zone` (the chart zone by default), as `YYYY-MM-DD` */
export function formatDay(instant: Date, zone: string | undefined = chartTimeZone()): string {
  const { year, month, day } = wallClock(instant, zone);
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(year, 4)}-${pad(month + 1)}-${pad(day)}`;
}

/** the day `days` after `day`, on the calendar */
const addDays = (day: CalendarDay, days: number): CalendarDay => {
  const date = new Date(Date.UTC(day.year, day.month, day.day + days));
  return { year: date.getUTCFullYear(), month: date.getUTCMonth(), day: date.getUTCDate() };
};

/** midnight at the start of `day` in `zone` */
const dayStart = (day: CalendarDay, zone: string | undefined) => zonedInstant(day, zone);

/** whole days from `a` to `b`, counted on the calendar so a daylight-saving change is not one off */
const daysBetween = (a: CalendarDay, b: CalendarDay) =>
  Math.round((Date.UTC(b.year, b.month, b.day) - Date.UTC(a.year, a.month, a.day)) / (24 * HOUR));

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
  return daysBetween(since, until) < 0 ? "order" : null;
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

// midnight in `zone` on the first of the month `offset` months from the one
// `now` falls in there. a month of -1 or 12 carries into the neighbouring year,
// and a midnight in the chart zone is what makes the boundary the month the
// charts label rather than UTC's or the browser's (#3500)
function monthStart(now: Date, offset: number, zone: string | undefined): Date {
  const { year, month } = wallClock(now, zone);
  return zonedInstant({ year, month: month + offset, day: 1 }, zone);
}

/**
 * The first instant the control plane reads a window from (#3506): it refuses a
 * `since` or `until` before the first of January 2000, UTC, and one more than a
 * day past its own clock.
 */
const EARLIEST_BOUND = Date.UTC(MIN_DAY_YEAR, 0, 1);

/**
 * `instant`, held to what the control plane reads: no earlier than
 * {@link EARLIEST_BOUND}, which a first day of 2000 is in a zone ahead of UTC,
 * and no later than `now`, which a first day typed in the future is. A window
 * clamped that way may be empty, but it is never a `400` (#3506)
 */
const readableBound = (instant: Date, now: Date) =>
  new Date(Math.min(Math.max(instant.getTime(), EARLIEST_BOUND), now.getTime())).toISOString();

/**
 * What a custom range covers: from the start of its first day to the start of
 * the day after its last, in the chart zone, since `until` is exclusive.
 * A range that has not ended by `now` runs up to now and sends no `until`, the
 * way a named window does, and its start is held to what the control plane
 * reads ({@link readableBound}), so no day a person can type is a bound it
 * refuses (#3506).
 */
function customBounds(range: DayRange, now: Date, zone: string | undefined): WindowBounds {
  const since = readableBound(dayStart(parseDay(range.since) as CalendarDay, zone), now);
  const end = dayStart(addDays(parseDay(range.until) as CalendarDay, 1), zone);
  if (end.getTime() > now.getTime()) return { since };
  return { since, until: end.toISOString() };
}

/** What `window` covers at the instant `now`, its days counted in `zone`. */
export function windowBounds(
  window: WindowSelection,
  now: Date = new Date(),
  zone: string | undefined = chartTimeZone(),
): WindowBounds {
  const range = customRange(window);
  if (range) return customBounds(range, now, zone);
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
      return { since: monthStart(now, 0, zone).toISOString() };
    case "last-month":
      // the whole previous month, however far into this one `now` is
      return {
        since: monthStart(now, -1, zone).toISOString(),
        until: monthStart(now, 0, zone).toISOString(),
      };
    default:
      // a custom range that is not one (cannot happen for a value read through
      // `readWindowSelection`) reads as the default rather than as no bounds
      return windowBounds(DEFAULT_TIME_WINDOW, now, zone);
  }
}

/**
 * Whether `window` is counted in calendar days (the month so far, last month,
 * a custom range), whose bounds are the chart zone's midnights and so move
 * when the zone does. A rolling window is the same hours in every zone.
 */
export function countsCalendarDays(window: WindowSelection): boolean {
  return window === "mtd" || window === "last-month" || customRange(window) !== null;
}

/**
 * What a read over `window` is keyed on (#3519): the window's name and, for a
 * window of calendar days, the chart zone its days are counted in (`""` for
 * the browser's own). Spread it into a query key where the window's name went,
 * so a change of **Time zone** in Preferences is a new read over the new
 * zone's midnights rather than a cached answer over the old one's. A rolling
 * window carries `null`, so a zone change does not read it again.
 */
export function windowQueryKey(
  window: WindowSelection,
  zone: string | undefined = chartTimeZone(),
): readonly [WindowSelection, string | null] {
  return [window, countsCalendarDays(window) ? (zone ?? "") : null];
}

/** {@link windowQueryKey} for a component, which re-renders when the chart zone changes */
export function useWindowQueryKey(
  window: WindowSelection,
): readonly [WindowSelection, string | null] {
  return windowQueryKey(window, useChartTimeZone());
}

/** the longest custom range, in days, that is still drawn an hour at a time */
const HOURLY_CUSTOM_DAYS = 3;

/**
 * How finely a chart over `window` is cut (#3407). The named windows are drawn
 * an hour at a time as they always were. A custom range is as long as the
 * person made it, and a month of hours is thousands of points, so one longer
 * than three days is drawn a day at a time.
 */
export function chartBucket(window: WindowSelection): "hour" | "day" {
  const range = customRange(window);
  if (!range) return "hour";
  const days =
    daysBetween(parseDay(range.since) as CalendarDay, parseDay(range.until) as CalendarDay) + 1;
  return days > HOURLY_CUSTOM_DAYS ? "day" : "hour";
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
  zone: string | undefined = chartTimeZone(),
): Required<WindowBounds> {
  const range = customRange(window);
  if (range) {
    // the same number of days straight before the first: a range of three days
    // is compared with the three days that precede it
    const first = parseDay(range.since) as CalendarDay;
    const days = daysBetween(first, parseDay(range.until) as CalendarDay) + 1;
    // a range that starts in the first days of 2000 has a shorter window
    // before it, since the control plane reads nothing earlier (#3506)
    return {
      since: readableBound(dayStart(addDays(first, -days), zone), now),
      until: readableBound(dayStart(first, zone), now),
    };
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
      const since = monthStart(now, -1, zone);
      // the same wall-clock moment a month back, in the chart zone, so a
      // daylight-saving change between the two does not shift it by an hour.
      // a 31st has no counterpart in a 30-day month and would carry into this
      // one, so the comparison stops at last month's end
      const wall = wallClock(now, zone);
      const sameMoment = zonedInstant({ ...wall, month: wall.month - 1 }, zone);
      const until = Math.min(sameMoment.getTime(), monthStart(now, 0, zone).getTime());
      return { since: since.toISOString(), until: new Date(until).toISOString() };
    }
    case "last-month":
      return {
        since: monthStart(now, -2, zone).toISOString(),
        until: monthStart(now, -1, zone).toISOString(),
      };
    default:
      return previousWindowBounds(DEFAULT_TIME_WINDOW, now, zone);
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
 * The window a screen reads, kept in the address and nowhere else (#3407).
 *
 * {@link useTimeWindow} also remembers the pick for the next screen opened from
 * the nav rail. LLM Logs, MCP Logs and the Dashboard do not: a saved view
 * (#2452) names its window, and a window carried in from another screen would
 * make the same address read two ways. The selection is read from `window`,
 * `since` and `until`, and a pick writes all three, so a named window drops the
 * days a range left in the address. An unknown value reads as `fallback`.
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
