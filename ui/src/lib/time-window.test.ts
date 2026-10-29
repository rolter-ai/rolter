import { afterEach, beforeEach, describe, expect, it } from "bun:test";

import { setChartTimeZone } from "@/lib/i18n/format";
import {
  chartBucket,
  customRange,
  customWindow,
  dayRangeProblem,
  DEFAULT_TIME_WINDOW,
  formatDay,
  isTimeWindow,
  isWindowSelection,
  previousWindowBounds,
  readTimeWindow,
  readWindowSelection,
  TIME_WINDOWS,
  windowAddress,
  windowBounds,
  windowFilters,
  windowFromFilters,
  windowQueryKey,
  countsCalendarDays,
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

describe("previousWindowBounds (#2010)", () => {
  inZone("America/New_York");
  const iso = (d: Date) => d.toISOString();

  it("steps a rolling window back by its own length", () => {
    const now = new Date("2026-03-10T12:00:00Z");
    expect(previousWindowBounds("24h", now)).toEqual({
      since: "2026-03-08T12:00:00.000Z",
      until: "2026-03-09T12:00:00.000Z",
    });
    expect(previousWindowBounds("7d", now)).toEqual({
      since: "2026-02-24T12:00:00.000Z",
      until: "2026-03-03T12:00:00.000Z",
    });
  });

  it("ends where the current rolling window begins", () => {
    const now = new Date("2026-03-10T12:00:00Z");
    for (const w of ["24h", "7d", "30d"] as const) {
      expect(previousWindowBounds(w, now).until).toBe(windowBounds(w, now).since);
    }
  });

  it("compares the month so far with the same stretch of last month", () => {
    const now = local(2026, 3, 10, 6);
    expect(previousWindowBounds("mtd", now)).toEqual({
      since: iso(local(2026, 2, 1)),
      until: iso(local(2026, 2, 10, 6)),
    });
  });

  it("stops at last month's end when this month has more days", () => {
    expect(previousWindowBounds("mtd", local(2026, 3, 31, 12))).toEqual({
      since: iso(local(2026, 2, 1)),
      until: iso(local(2026, 3, 1)),
    });
  });

  it("compares last month with the month before it, across a year", () => {
    expect(previousWindowBounds("last-month", local(2026, 2, 15))).toEqual({
      since: iso(local(2025, 12, 1)),
      until: iso(local(2026, 1, 1)),
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

describe("custom range (#3407)", () => {
  describe("in New York", () => {
    inZone("America/New_York");

    it("covers whole calendar days, the last one included", () => {
      // the 1st to the 7th is seven days: from midnight on the 1st to midnight on the 8th
      const w = customWindow("2026-10-01", "2026-10-07");
      expect(w).not.toBeNull();
      expect(windowBounds(w!)).toEqual({
        since: local(2026, 10, 1).toISOString(),
        until: local(2026, 10, 8).toISOString(),
      });
    });

    it("is a day long when the two days are the same", () => {
      const bounds = windowBounds(customWindow("2026-10-05", "2026-10-05")!);
      expect(bounds).toEqual({
        since: local(2026, 10, 5).toISOString(),
        until: local(2026, 10, 6).toISOString(),
      });
    });

    it("keeps its midnights across the autumn clock change", () => {
      // 1 November 2026 is the 25-hour day: the end is still midnight on the 2nd
      const bounds = windowBounds(
        customWindow("2026-10-31", "2026-11-01")!,
        new Date("2026-12-01T00:00:00Z"),
      );
      expect(bounds).toEqual({
        since: local(2026, 10, 31).toISOString(),
        until: local(2026, 11, 2).toISOString(),
      });
    });

    it("compares a range with the same number of days straight before it", () => {
      const previous = previousWindowBounds(customWindow("2026-10-04", "2026-10-06")!);
      expect(previous).toEqual({
        since: local(2026, 10, 1).toISOString(),
        until: local(2026, 10, 4).toISOString(),
      });
    });

    it("runs a range that has not ended up to now, with no until (#3506)", () => {
      // the control plane refuses an until more than a day past its clock
      const now = local(2026, 10, 5, 12);
      expect(windowBounds(customWindow("2026-10-01", "2026-10-07")!, now)).toEqual({
        since: local(2026, 10, 1).toISOString(),
      });
      // one that ends today too: its end, tomorrow's midnight, is still ahead
      expect(windowBounds(customWindow("2026-10-01", "2026-10-05")!, now)).toEqual({
        since: local(2026, 10, 1).toISOString(),
      });
    });

    it("compares a range from early 2000 with nothing before 2000 (#3506)", () => {
      const previous = previousWindowBounds(customWindow("2000-01-02", "2000-01-10")!);
      expect(previous).toEqual({
        since: "2000-01-01T00:00:00.000Z",
        until: local(2000, 1, 2).toISOString(),
      });
    });

    it("names the first and last day it covers in a caption", () => {
      const bounds = windowBounds(customWindow("2026-10-01", "2026-10-07")!);
      const { from, to } = windowSpan(bounds, new Date("2026-10-30T00:00:00Z"));
      expect(from.getTime()).toBe(local(2026, 10, 1).getTime());
      // an exclusive end closes a millisecond early: still the 7th
      expect(to.getTime()).toBe(local(2026, 10, 8).getTime() - 1);
    });
  });

  it("refuses an end before the start", () => {
    expect(dayRangeProblem({ since: "2026-10-07", until: "2026-10-01" })).toBe("order");
    expect(customWindow("2026-10-07", "2026-10-01")).toBeNull();
  });

  it("refuses a day that is not on the calendar", () => {
    expect(dayRangeProblem({ since: "2026-02-31", until: "2026-03-02" })).toBe("invalid");
    expect(dayRangeProblem({ since: "2026-10-01", until: "" })).toBe("invalid");
    expect(dayRangeProblem({ since: "10/01/2026", until: "2026-10-02" })).toBe("invalid");
    // before the year 2000 is not a day, as the server holds a saved view to
    expect(dayRangeProblem({ since: "0001-01-01", until: "2026-10-02" })).toBe("invalid");
    expect(dayRangeProblem({ since: "1999-12-31", until: "2026-10-02" })).toBe("invalid");
    expect(dayRangeProblem({ since: "2000-01-01", until: "2000-01-02" })).toBeNull();
    expect(customWindow(null, "2026-10-02")).toBeNull();
  });

  it("round-trips through its string form", () => {
    const w = customWindow("2026-10-01", "2026-10-07")!;
    expect(customRange(w)).toEqual({ since: "2026-10-01", until: "2026-10-07" });
    expect(customRange("7d")).toBeNull();
    expect(isWindowSelection(w)).toBe(true);
    expect(isWindowSelection("custom:2026-10-07:2026-10-01")).toBe(false);
    expect(isWindowSelection("custom")).toBe(false);
    expect(isWindowSelection("7d")).toBe(true);
  });

  it("is read from the address, and a broken range reads as the default", () => {
    const read = (query: string, fallback?: "7d") =>
      readWindowSelection(new URLSearchParams(query), fallback);
    expect(read("window=custom&since=2026-10-01&until=2026-10-07")).toBe(
      "custom:2026-10-01:2026-10-07",
    );
    expect(read("window=custom&since=2026-10-07&until=2026-10-01")).toBe(DEFAULT_TIME_WINDOW);
    expect(read("window=custom&since=2026-10-01")).toBe(DEFAULT_TIME_WINDOW);
    expect(read("window=30d&since=2026-10-01&until=2026-10-07")).toBe("30d");
    // no window at all is the screen's own default, an unknown one is the global default
    expect(read("", "7d")).toBe("7d");
    expect(read("window=fortnight", "7d")).toBe(DEFAULT_TIME_WINDOW);
  });

  it("writes the three address parameters together", () => {
    expect(windowAddress("custom:2026-10-01:2026-10-07")).toEqual({
      window: "custom",
      since: "2026-10-01",
      until: "2026-10-07",
    });
    // a named window drops the days a range left behind
    expect(windowAddress("7d")).toEqual({ window: "7d", since: "", until: "" });
    expect(windowAddress(DEFAULT_TIME_WINDOW)).toEqual({ window: "", since: "", until: "" });
    expect(windowAddress("7d", "7d")).toEqual({ window: "", since: "", until: "" });
  });

  it("is saved as its days, so a view reads the same whenever it is applied", () => {
    const w = customWindow("2026-10-01", "2026-10-07")!;
    expect(windowFilters(w)).toEqual({
      window: "custom",
      since: "2026-10-01",
      until: "2026-10-07",
    });
    expect(windowFilters("30d")).toEqual({ window: "30d" });
    expect(windowFromFilters(windowFilters(w))).toBe(w);
    expect(windowFromFilters({ window: "custom", since: "2026-10-07", until: "2026-10-01" })).toBe(
      DEFAULT_TIME_WINDOW,
    );
    expect(windowFromFilters({})).toBe(DEFAULT_TIME_WINDOW);
  });
});

describe("chartBucket (#3407)", () => {
  it("keeps the named windows hourly, as they always were", () => {
    for (const w of TIME_WINDOWS) expect(chartBucket(w)).toBe("hour");
  });

  it("draws up to three days an hour at a time and anything longer a day at a time", () => {
    expect(chartBucket(customWindow("2026-10-05", "2026-10-05")!)).toBe("hour");
    expect(chartBucket(customWindow("2026-10-01", "2026-10-03")!)).toBe("hour");
    expect(chartBucket(customWindow("2026-10-01", "2026-10-04")!)).toBe("day");
    expect(chartBucket(customWindow("2026-01-01", "2026-10-29")!)).toBe("day");
  });
});

describe("calendar days in the chart zone (#3500)", () => {
  // the browser sits in one zone and the account draws its charts in another:
  // every calendar day is the chart zone's
  describe("charts in New York, browser in Tokyo", () => {
    inZone("Asia/Tokyo");
    const zone = "America/New_York";

    it("starts the month at New York's midnight", () => {
      const now = new Date("2026-11-10T12:00:00Z");
      expect(windowBounds("mtd", now, zone)).toEqual({ since: "2026-11-01T04:00:00.000Z" });
      expect(windowBounds("last-month", now, zone)).toEqual({
        since: "2026-10-01T04:00:00.000Z",
        until: "2026-11-01T04:00:00.000Z",
      });
    });

    it("keeps a custom range's midnights across the clock change inside it", () => {
      // 1 November 2026 is New York's 25-hour day
      const range = customWindow("2026-10-31", "2026-11-01")!;
      expect(windowBounds(range, new Date("2026-12-01T00:00:00Z"), zone)).toEqual({
        since: "2026-10-31T04:00:00.000Z",
        until: "2026-11-02T05:00:00.000Z",
      });
      // and the window before it is the two days before, the change included
      expect(
        previousWindowBounds(
          customWindow("2026-11-02", "2026-11-03")!,
          new Date("2026-12-01T00:00:00Z"),
          zone,
        ),
      ).toEqual({
        since: "2026-10-31T04:00:00.000Z",
        until: "2026-11-02T05:00:00.000Z",
      });
    });

    it("compares the month so far with the same stretch of last month, on New York's clock", () => {
      // 07:00 on the 10th in New York, after its clocks went back
      const now = new Date("2026-11-10T12:00:00Z");
      expect(previousWindowBounds("mtd", now, zone)).toEqual({
        since: "2026-10-01T04:00:00.000Z",
        // 07:00 on 10 October, still daylight time
        until: "2026-10-10T11:00:00.000Z",
      });
      expect(previousWindowBounds("last-month", now, zone)).toEqual({
        since: "2026-09-01T04:00:00.000Z",
        until: "2026-10-01T04:00:00.000Z",
      });
    });

    it("names the day an instant falls on in New York, not in Tokyo", () => {
      const instant = new Date("2026-11-10T02:00:00Z");
      expect(formatDay(instant, zone)).toBe("2026-11-09");
      expect(formatDay(instant, undefined)).toBe("2026-11-10");
    });
  });

  describe("charts in Berlin, browser in Los Angeles", () => {
    inZone("America/Los_Angeles");
    const zone = "Europe/Berlin";

    it("counts a custom range in Berlin across its clock change", () => {
      // 25 October 2026 is Berlin's 25-hour day, a week before Los Angeles's
      const range = customWindow("2026-10-24", "2026-10-26")!;
      expect(windowBounds(range, new Date("2026-12-01T00:00:00Z"), zone)).toEqual({
        since: "2026-10-23T22:00:00.000Z",
        until: "2026-10-26T23:00:00.000Z",
      });
      expect(previousWindowBounds(range, new Date("2026-12-01T00:00:00Z"), zone)).toEqual({
        since: "2026-10-20T22:00:00.000Z",
        until: "2026-10-23T22:00:00.000Z",
      });
    });

    it("ends last month at Berlin's midnight after the change", () => {
      const now = new Date("2026-11-05T10:00:00Z");
      expect(windowBounds("last-month", now, zone)).toEqual({
        since: "2026-09-30T22:00:00.000Z",
        until: "2026-10-31T23:00:00.000Z",
      });
      expect(windowBounds("mtd", now, zone)).toEqual({ since: "2026-10-31T23:00:00.000Z" });
    });
  });

  describe("the chart zone is the default", () => {
    inZone("Asia/Tokyo");
    afterEach(() => setChartTimeZone(null));

    it("reads the account's chart zone when none is passed", () => {
      const now = new Date("2026-11-10T12:00:00Z");
      // no chart zone: Tokyo's midnight on the 1st
      expect(windowBounds("mtd", now)).toEqual({ since: "2026-10-31T15:00:00.000Z" });
      setChartTimeZone("America/New_York");
      expect(windowBounds("mtd", now)).toEqual({ since: "2026-11-01T04:00:00.000Z" });
      expect(formatDay(new Date("2026-11-10T02:00:00Z"))).toBe("2026-11-09");
    });
  });
});

describe("a custom range is always a window the control plane reads (#3506)", () => {
  const now = new Date("2026-10-29T12:00:00Z");

  it("starts no earlier than 2000 in a zone ahead of UTC", () => {
    // midnight on 1 January 2000 in Berlin is 1999 in UTC
    const range = customWindow("2000-01-01", "2000-01-03")!;
    expect(windowBounds(range, now, "Europe/Berlin")).toEqual({
      since: "2000-01-01T00:00:00.000Z",
      until: "2000-01-03T23:00:00.000Z",
    });
    expect(windowBounds(range, now, "Asia/Tokyo").since).toBe("2000-01-01T00:00:00.000Z");
  });

  it("starts no later than now when its first day is in the future", () => {
    const range = customWindow("2026-11-10", "2026-11-12")!;
    expect(windowBounds(range, now, "UTC")).toEqual({ since: now.toISOString() });
    // and the window before it ends now, not on the future first day
    expect(previousWindowBounds(range, now, "UTC")).toEqual({
      since: now.toISOString(),
      until: now.toISOString(),
    });
  });
});

describe("windowQueryKey: a calendar-day read is keyed on the chart zone (#3519)", () => {
  afterEach(() => setChartTimeZone(null));

  it("carries the zone for the windows counted in calendar days", () => {
    const custom = customWindow("2026-10-01", "2026-10-07")!;
    for (const window of ["mtd", "last-month", custom] as const) {
      expect(countsCalendarDays(window)).toBe(true);
      expect(windowQueryKey(window, "Europe/Warsaw")).toEqual([window, "Europe/Warsaw"]);
      // the browser's own zone is a zone too, told apart from a named one
      expect(windowQueryKey(window, undefined)).toEqual([window, ""]);
    }
  });

  it("leaves a rolling window unkeyed on the zone", () => {
    for (const window of ["24h", "7d", "30d"] as const) {
      expect(countsCalendarDays(window)).toBe(false);
      expect(windowQueryKey(window, "Europe/Warsaw")).toEqual([window, null]);
    }
  });

  it("reads the chart zone by default", () => {
    setChartTimeZone("Pacific/Kiritimati");
    expect(windowQueryKey("mtd")).toEqual(["mtd", "Pacific/Kiritimati"]);
  });
});
