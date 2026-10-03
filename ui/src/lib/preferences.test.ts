import { afterAll, afterEach, beforeEach, describe, expect, test } from "bun:test";

import { preferencesDocument, putPreferences, type UserPreferences } from "@/lib/api";
import { formattersFor, setChartTimeZone, validTimeZone } from "@/lib/i18n/format";
import { applyLanguagePreference, currentLocale } from "@/lib/i18n";
import { isEmptyDocument, legacyDocument, refusedField } from "@/lib/preferences";
import { ApiError } from "@/lib/api";

// the pure half of the preferences work (#2448): the document helpers, the
// time-zone fallback and the one-time localStorage migration's input. the
// provider and the screen are asserted in their stories

const originalStorage = globalThis.localStorage;
const originalFetch = globalThis.fetch;

beforeEach(() => {
  const store = new Map<string, string>();
  globalThis.localStorage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: () => null,
    get length() {
      return store.size;
    },
  } as unknown as Storage;
});
afterEach(() => {
  setChartTimeZone(null);
  globalThis.fetch = originalFetch;
});
afterAll(() => {
  globalThis.localStorage = originalStorage;
});

const EMPTY: UserPreferences = {
  language: null,
  default_org_id: null,
  default_team_id: null,
  default_project_id: null,
  default_playground_model: null,
  chart_time_zone: null,
};

describe("the document", () => {
  test("drops the computed scope and fills a missing key with null", () => {
    const doc = preferencesDocument({
      language: "ru",
      effective_default_scope: { org_id: "o", team_id: null, project_id: null },
    } as unknown as UserPreferences);
    expect(Object.keys(doc).sort()).toEqual(Object.keys(EMPTY).sort());
    expect(doc.language).toBe("ru");
    expect(doc.chart_time_zone).toBeNull();
  });

  test("a save sends exactly the six keys, never the computed scope", async () => {
    let sent = "";
    globalThis.fetch = (async (_url: unknown, init?: RequestInit) => {
      sent = String(init?.body);
      return new Response(JSON.stringify({ ...EMPTY, effective_default_scope: null }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }) as typeof fetch;
    await putPreferences({ ...EMPTY, effective_default_scope: null } as UserPreferences);
    expect(Object.keys(JSON.parse(sent)).sort()).toEqual(Object.keys(EMPTY).sort());
  });

  test("an empty document is one with no key set", () => {
    expect(isEmptyDocument(EMPTY)).toBe(true);
    expect(isEmptyDocument({ ...EMPTY, default_playground_model: "gpt-4o" })).toBe(false);
  });
});

describe("the localStorage migration", () => {
  test("has nothing to move when this browser kept nothing", () => {
    expect(
      legacyDocument(EMPTY, { language: null, scope: { org: null, team: null, project: null } }),
    ).toBeNull();
  });

  test("moves the language and the scope over the empty document", () => {
    const doc = legacyDocument(
      { ...EMPTY, chart_time_zone: "UTC" },
      { language: "ru", scope: { org: "o", team: "t", project: "p" } },
    );
    expect(doc).toEqual({
      ...EMPTY,
      chart_time_zone: "UTC",
      language: "ru",
      default_org_id: "o",
      default_team_id: "t",
      default_project_id: "p",
    });
  });
});

describe("a refused save", () => {
  test("is pinned to the field the 400 names", () => {
    const refusal = (message: string) => new ApiError(message, 400, "bad_request");
    expect(refusedField(refusal("language must be one of: en, ru"))).toBe("language");
    expect(refusedField(refusal("chart_time_zone must be an IANA time zone name"))).toBe(
      "chart_time_zone",
    );
    expect(refusedField(refusal("default_playground_model must be at most 200 characters"))).toBe(
      "default_playground_model",
    );
    expect(refusedField(refusal("something else"))).toBeNull();
    expect(refusedField(new ApiError("language", 500))).toBeNull();
  });
});

describe("the chart time zone", () => {
  test("a zone the engine knows is kept; an unknown one is the local zone", () => {
    expect(validTimeZone("Asia/Tokyo")).toBe("Asia/Tokyo");
    expect(validTimeZone("Mars/Olympus_Mons")).toBeUndefined();
    expect(validTimeZone("")).toBeUndefined();
    expect(validTimeZone(null)).toBeUndefined();
  });

  test("every clock and date follows it", () => {
    const fmt = formattersFor("en");
    setChartTimeZone("Asia/Tokyo");
    expect(fmt.timeShort("2026-01-01T00:30:00Z")).toBe("09:30");
    setChartTimeZone("UTC");
    expect(fmt.timeShort("2026-01-01T00:30:00Z")).toBe("00:30");
  });

  test("an unknown zone never throws out of a formatter", () => {
    setChartTimeZone("Mars/Olympus_Mons");
    expect(() => formattersFor("en").dateTime("2026-01-01T00:30:00Z")).not.toThrow();
  });

  test("a day is the day in the chart zone, not the browser's", () => {
    const fmt = formattersFor("en");
    // 23:30 UTC on the 1st is already the 2nd in Tokyo
    setChartTimeZone("Asia/Tokyo");
    expect(fmt.dayUnlessToday("2026-01-01T23:30:00Z", "2026-01-02T03:00:00Z")).toBe("");
    setChartTimeZone("UTC");
    expect(fmt.dayUnlessToday("2026-01-01T23:30:00Z", "2026-01-02T03:00:00Z")).not.toBe("");
  });
});

describe("the language preference", () => {
  test("null follows the browser and forgets the stored choice", async () => {
    localStorage.setItem("rolter.locale", "ru");
    await applyLanguagePreference(null);
    expect(localStorage.getItem("rolter.locale")).toBeNull();
    expect(currentLocale()).toBe("en");
  });
});
