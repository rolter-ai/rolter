import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import { PERIOD_KINDS, PERIOD_SPELLINGS, periodKind } from "./budget-period";

const CONFIG = fileURLToPath(new URL("../../../crates/rolter-core/src/config.rs", import.meta.url));

/** the `(spelling, window)` pairs of `BudgetPeriod::SPELLINGS`, in order */
function rustSpellings(source: string): [string, string][] {
  const start = source.indexOf("pub const SPELLINGS");
  if (start < 0) throw new Error("BudgetPeriod::SPELLINGS not found in config.rs");
  const end = source.indexOf("];", start);
  const table = source.slice(start, end < 0 ? undefined : end);
  return [...table.matchAll(/\("([^"]+)",\s*BudgetPeriod::(\w+)\)/g)].map((m) => [
    m[1],
    m[2].toLowerCase(),
  ]);
}

// the card names the window a stored period is enforced as, and the sheet
// refuses to save one the control plane would, so the dashboard's copy of the
// spellings has to be the gateway's (#1902)
describe("the period vocabulary", () => {
  it("mirrors BudgetPeriod::SPELLINGS in rolter-core", () => {
    const rust = rustSpellings(readFileSync(CONFIG, "utf8"));
    expect(rust.length).toBeGreaterThan(0);
    const mirrored: [string, string][] = [...PERIOD_SPELLINGS];
    expect(mirrored).toEqual(rust);
  });

  it("offers each window once, under the name the gateway reads it by", () => {
    expect(new Set(PERIOD_KINDS).size).toBe(PERIOD_KINDS.length);
    for (const kind of PERIOD_KINDS) expect(periodKind(kind)).toBe(kind);
    expect(new Set(PERIOD_SPELLINGS.values())).toEqual(new Set(PERIOD_KINDS));
  });
});

describe("periodKind", () => {
  it("names the spellings the gateway reads as a day", () => {
    for (const period of ["daily", "1d", "24h"]) expect(periodKind(period)).toBe("daily");
  });

  it("names the spellings the gateway reads as a lifetime cap", () => {
    for (const period of ["total", "lifetime", "all"]) expect(periodKind(period)).toBe("total");
  });

  it("names a month, including the dashboard's own 30d default", () => {
    expect(periodKind("monthly")).toBe("monthly");
    expect(periodKind("30d")).toBe("monthly");
  });

  it("reads the way the gateway does: trimmed and case-insensitive", () => {
    expect(periodKind("  Daily ")).toBe("daily");
    expect(periodKind("30D")).toBe("monthly");
  });

  it("leaves an unknown period to be printed as stored", () => {
    // enforced as monthly by the gateway, but not something the dashboard can name
    expect(periodKind("7d")).toBeNull();
    expect(periodKind("dialy")).toBeNull();
    expect(periodKind("")).toBeNull();
  });

  it("does not mistake an object property for a period", () => {
    expect(periodKind("constructor")).toBeNull();
    expect(periodKind("toString")).toBeNull();
  });
});
