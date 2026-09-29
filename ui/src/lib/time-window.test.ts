import { afterEach, beforeEach, describe, expect, it } from "bun:test";

import {
  DEFAULT_TIME_WINDOW,
  isTimeWindow,
  readTimeWindow,
  TIME_WINDOWS,
  windowBounds,
  windowSpan,
} from "@/lib/time-window";

// month boundaries are the viewer's calendar months, so every case below runs
// under a named zone rather than whatever the machine is set to. bun re-reads
// TZ when it is assigned, which is what lets one file pin several zones. the
// zone is put back by name: once TZ is deleted, bun stops honouring later
// assignments, and every test after this file would run in the wrong zone
const originalTz = process.env.TZ || Intl.DateTimeFormat().resolvedOptions().timeZone;
const inZone = (tz: string) => {
  beforeEach(() => {
    process.env.TZ = tz;
  });
  afterEach(() => {
    process.env.TZ = originalTz;
  });
};

// a wall-clock instant in whatever zone is pinned, so a case reads as the
// viewer's own date rather than as a UTC offset worked out by hand
const local = (y: number, m: number, d: number, h = 0, min = 0) => new Date(y, m - 1, d, h, min);

describe("readTimeWindow", () => {
  it("accepts every window the picker offers", () => {
    for (const w of TIME_WINDOWS) expect(readTimeWindow(w)).toBe(w);
  });

  it("reads a missing or unknown value as the default window", () => {
    expect(readTimeWindow(null)).toBe(DEFAULT_TIME_WINDOW);
    expect(readTimeWindow(undefined)).toBe(DEFAULT_TIME_WINDOW);
    expect(readTimeWindow("fortnight")).toBe(DEFAULT_TIME_WINDOW);
    // the values are matched exactly, never loosely
    expect(readTimeWindow("7D")).toBe(DEFAULT_TIME_WINDOW);
    expect(readTimeWindow("")).toBe(DEFAULT_TIME_WINDOW);
  });

  it("defaults to the last 24 hours, the window the Dashboard reports", () => {
    expect(DEFAULT_TIME_WINDOW).toBe("24h");
    expect(isTimeWindow("24h")).toBe(true);
    expect(isTimeWindow(24)).toBe(false);
  });
});

describe("windowBounds: rolling windows", () => {
  inZone("UTC");
  const now = new Date("2026-09-30T14:25:00.000Z");

  it("reaches back from the moment it is asked and leaves the end open", () => {
    expect(windowBounds("24h", now)).toEqual({ since: "2026-09-29T14:25:00.000Z" });
    expect(windowBounds("7d", now)).toEqual({ since: "2026-09-23T14:25:00.000Z" });
    expect(windowBounds("30d", now)).toEqual({ since: "2026-08-31T14:25:00.000Z" });
  });

  it("rolls forward with the clock instead of keeping the first answer", () => {
    const later = new Date(now.getTime() + 3 * 3_600_000);
    expect(windowBounds("24h", later).since).toBe("2026-09-29T17:25:00.000Z");
  });
});

describe("windowBounds: calendar months in the viewer's zone", () => {
  describe("in New York", () => {
    inZone("America/New_York");

    it("starts month to date at local midnight on the first, open to now", () => {
      // 2026-09-01 00:00 EDT is 04:00 UTC
      expect(windowBounds("mtd", local(2026, 9, 15, 12))).toEqual({
        since: "2026-09-01T04:00:00.000Z",
      });
    });

    it("covers the whole previous month, closed at the first of this one", () => {
      expect(windowBounds("last-month", local(2026, 9, 15, 12))).toEqual({
        since: "2026-08-01T04:00:00.000Z",
        until: "2026-09-01T04:00:00.000Z",
      });
    });

    it("is the whole previous month even on the last minute of this one", () => {
      expect(windowBounds("last-month", local(2026, 9, 30, 23, 59))).toEqual({
        since: "2026-08-01T04:00:00.000Z",
        until: "2026-09-01T04:00:00.000Z",
      });
    });

    it("follows the local offset across a clock change rather than counting days", () => {
      // clocks went forward on 8 March 2026, so March opened at UTC-5 and April
      // at UTC-4: a month is a calendar month, not thirty days of milliseconds
      expect(windowBounds("last-month", local(2026, 4, 2, 9))).toEqual({
        since: "2026-03-01T05:00:00.000Z",
        until: "2026-04-01T04:00:00.000Z",
      });
      expect(windowBounds("mtd", local(2026, 4, 2, 9)).since).toBe("2026-04-01T04:00:00.000Z");
    });

    it("steps back into the previous year from January", () => {
      expect(windowBounds("last-month", local(2027, 1, 10, 8))).toEqual({
        since: "2026-12-01T05:00:00.000Z",
        until: "2027-01-01T05:00:00.000Z",
      });
    });

    it("counts the first half hour of a month as this month", () => {
      const now = local(2026, 10, 1, 0, 30);
      expect(windowBounds("mtd", now).since).toBe("2026-10-01T04:00:00.000Z");
      expect(windowBounds("last-month", now)).toEqual({
        since: "2026-09-01T04:00:00.000Z",
        until: "2026-10-01T04:00:00.000Z",
      });
    });
  });

  describe("in Tokyo", () => {
    inZone("Asia/Tokyo");

    it("opens the month at the viewer's midnight, which is the previous UTC day", () => {
      // 2026-09-01 00:00 JST is 2026-08-31 15:00 UTC: a UTC month boundary
      // would have filed the first nine hours of September under August
      expect(windowBounds("mtd", local(2026, 9, 1, 3))).toEqual({
        since: "2026-08-31T15:00:00.000Z",
      });
      expect(windowBounds("last-month", local(2026, 9, 1, 3))).toEqual({
        since: "2026-07-31T15:00:00.000Z",
        until: "2026-08-31T15:00:00.000Z",
      });
    });
  });
});

describe("windowSpan", () => {
  inZone("UTC");

  it("ends an open window at the moment it was sent", () => {
    const at = new Date("2026-09-30T14:25:00.000Z");
    const span = windowSpan(windowBounds("7d", at), at);
    expect(span.from.toISOString()).toBe("2026-09-23T14:25:00.000Z");
    expect(span.to).toBe(at);
  });

  it("ends a closed window on its own last day, not on the exclusive bound", () => {
    const at = new Date("2026-09-30T14:25:00.000Z");
    const span = windowSpan(windowBounds("last-month", at), at);
    expect(span.from.toISOString()).toBe("2026-08-01T00:00:00.000Z");
    expect(span.to.toISOString()).toBe("2026-08-31T23:59:59.999Z");
  });
});
