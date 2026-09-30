import { describe, expect, it } from "bun:test";

import { periodKind } from "./budget-period";

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
