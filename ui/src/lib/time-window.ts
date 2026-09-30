import * as React from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

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

/**
 * Where the last window picked in this tab is remembered, so it carries over
 * to a screen opened from the nav rail, whose links drop the query string.
 */
export const TIME_WINDOW_STORAGE_KEY = "rolter.time-window";

export function isTimeWindow(value: unknown): value is TimeWindow {
  return typeof value === "string" && (TIME_WINDOWS as readonly string[]).includes(value);
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

/** What `window` covers at the instant `now`. */
export function windowBounds(window: TimeWindow, now: Date = new Date()): WindowBounds {
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
function remembered(): TimeWindow | null {
  try {
    const raw = sessionStorage.getItem(TIME_WINDOW_STORAGE_KEY);
    return isTimeWindow(raw) ? raw : null;
  } catch {
    return null;
  }
}

function remember(window: TimeWindow): void {
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
export function useTimeWindow(): [TimeWindow, (next: TimeWindow) => void] {
  const [params, setParams] = useSearchParams();
  const raw = params.get(TIME_WINDOW_PARAM);
  const window = raw === null ? (remembered() ?? DEFAULT_TIME_WINDOW) : readTimeWindow(raw);

  const write = React.useCallback(
    (next: TimeWindow) =>
      setParams(
        (prev) => {
          // the other parameters are left alone, since the address is not only the picker's
          const out = new URLSearchParams(prev);
          if (next === DEFAULT_TIME_WINDOW) out.delete(TIME_WINDOW_PARAM);
          else out.set(TIME_WINDOW_PARAM, next);
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
    (next: TimeWindow) => {
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
