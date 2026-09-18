import { beforeEach, describe, expect, it } from "bun:test";

import {
  RECENT_LIMIT,
  RECENT_STORAGE_KEY,
  fuzzyScore,
  isNavSearchShortcut,
  isPaletteShortcut,
  isTextEntry,
  rankEntries,
  readRecentScreens,
  rememberScreen,
  type PaletteEntry,
  type RecentStore,
} from "./command-palette";

// the test runner has no DOM, so storage is handed in rather than reached for
function fakeStore(): RecentStore {
  const cells = new Map<string, string>();
  return {
    getItem: (key) => cells.get(key) ?? null,
    setItem: (key, value) => {
      cells.set(key, value);
    },
  };
}

/** the shape the shortcut rules read off an event target */
const field = (tagName: string) => ({ tagName }) as unknown as EventTarget;

const entry = (id: string, label: string, hint?: string): PaletteEntry => ({
  id,
  screen: id,
  label,
  hint,
});

describe("fuzzyScore", () => {
  it("matches a subsequence, not only a substring", () => {
    expect(fuzzyScore("Routing Rules", "rr")).not.toBeNull();
    expect(fuzzyScore("Audit Logs", "audlog")).not.toBeNull();
  });

  it("refuses a query the label does not contain in order", () => {
    expect(fuzzyScore("Providers", "zz")).toBeNull();
    expect(fuzzyScore("Providers", "srp")).toBeNull();
  });

  it("scores an initialism above an incidental match", () => {
    const initials = fuzzyScore("Routing Rules", "rr")!;
    const scattered = fuzzyScore("Providers", "rr")!;
    expect(initials).toBeGreaterThan(scattered);
  });

  it("scores a match on a word boundary above one inside a word", () => {
    // both start at the head, so only the boundary rule separates them
    expect(fuzzyScore("Audit Logs", "al")!).toBeGreaterThan(fuzzyScore("Analytics", "al")!);
  });

  it("scores an adjacent run above the same letters scattered", () => {
    expect(fuzzyScore("Logs", "lo")!).toBeGreaterThan(fuzzyScore("Latency Overview", "lo")!);
  });

  it("scores the shorter of two labels higher", () => {
    expect(fuzzyScore("Logs", "logs")!).toBeGreaterThan(fuzzyScore("Logs Settings", "logs")!);
  });

  it("treats an empty query as a match on everything", () => {
    expect(fuzzyScore("Providers", "")).toBe(0);
    expect(fuzzyScore("Providers", "   ")).toBe(0);
  });
});

describe("rankEntries", () => {
  it("keeps the caller's order for an empty query", () => {
    const all = [entry("a", "Playground"), entry("b", "Dashboard")];
    expect(rankEntries(all, "").map((e) => e.id)).toEqual(["a", "b"]);
  });

  it("drops what does not match and puts the best first", () => {
    const all = [entry("p", "Providers"), entry("r", "Routing Rules"), entry("x", "Cluster")];
    expect(rankEntries(all, "rr").map((e) => e.id)).toEqual(["r", "p"]);
  });

  it("matches the hint too, but never above a label match", () => {
    const all = [entry("record", "acme-key", "Virtual keys"), entry("screen", "Virtual keys")];
    expect(rankEntries(all, "virtual").map((e) => e.id)).toEqual(["screen", "record"]);
  });

  it("breaks a tie on the caller's order", () => {
    const all = [entry("first", "Same"), entry("second", "Same")];
    expect(rankEntries(all, "same").map((e) => e.id)).toEqual(["first", "second"]);
  });
});

describe("recent screens", () => {
  let store: RecentStore;
  beforeEach(() => {
    store = fakeStore();
  });

  it("starts empty and remembers most-recent first", () => {
    expect(readRecentScreens(store)).toEqual([]);
    rememberScreen("dashboard", store);
    rememberScreen("providers", store);
    expect(readRecentScreens(store)).toEqual(["providers", "dashboard"]);
  });

  it("moves a repeat visit to the front instead of duplicating it", () => {
    rememberScreen("dashboard", store);
    rememberScreen("providers", store);
    expect(rememberScreen("dashboard", store)).toEqual(["dashboard", "providers"]);
  });

  it("keeps at most RECENT_LIMIT screens", () => {
    let last: string[] = [];
    for (let i = 0; i < RECENT_LIMIT + 3; i += 1) last = rememberScreen(`screen-${i}`, store);
    expect(last).toHaveLength(RECENT_LIMIT);
    expect(readRecentScreens(store)).toHaveLength(RECENT_LIMIT);
  });

  it("caps a list an older version left over the limit", () => {
    const many = Array.from({ length: RECENT_LIMIT + 4 }, (_, i) => `screen-${i}`);
    store.setItem(RECENT_STORAGE_KEY, JSON.stringify(many));
    expect(readRecentScreens(store)).toHaveLength(RECENT_LIMIT);
  });

  it("reads garbage in storage as nothing visited", () => {
    store.setItem(RECENT_STORAGE_KEY, "{not json");
    expect(readRecentScreens(store)).toEqual([]);
    store.setItem(RECENT_STORAGE_KEY, JSON.stringify({ dashboard: true }));
    expect(readRecentScreens(store)).toEqual([]);
    store.setItem(RECENT_STORAGE_KEY, JSON.stringify(["dashboard", 7]));
    expect(readRecentScreens(store)).toEqual(["dashboard"]);
  });

  it("still answers, and forgets, with no storage at all", () => {
    expect(readRecentScreens(null)).toEqual([]);
    expect(rememberScreen("dashboard", null)).toEqual(["dashboard"]);
  });
});

describe("shortcuts", () => {
  it("opens the palette on ⌘K and Ctrl-K only", () => {
    expect(isPaletteShortcut({ key: "k", metaKey: true, ctrlKey: false })).toBe(true);
    expect(isPaletteShortcut({ key: "K", metaKey: false, ctrlKey: true })).toBe(true);
    expect(isPaletteShortcut({ key: "k", metaKey: false, ctrlKey: false })).toBe(false);
    expect(isPaletteShortcut({ key: "j", metaKey: true, ctrlKey: false })).toBe(false);
  });

  it("focuses the nav search on a bare slash", () => {
    const bare = { key: "/", metaKey: false, ctrlKey: false, altKey: false, target: null };
    expect(isNavSearchShortcut(bare)).toBe(true);
    expect(isNavSearchShortcut({ ...bare, metaKey: true })).toBe(false);
    expect(isNavSearchShortcut({ ...bare, key: "?" })).toBe(false);
  });

  it("leaves the slash alone while text is being typed", () => {
    expect(isTextEntry(field("INPUT"))).toBe(true);
    expect(isTextEntry(field("TEXTAREA"))).toBe(true);
    expect(
      isNavSearchShortcut({
        key: "/",
        metaKey: false,
        ctrlKey: false,
        altKey: false,
        target: field("INPUT"),
      }),
    ).toBe(false);
    expect(isTextEntry(field("DIV"))).toBe(false);
    expect(isTextEntry(null)).toBe(false);
  });
});
