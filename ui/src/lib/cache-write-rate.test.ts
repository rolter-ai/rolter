import { describe, expect, test } from "bun:test";

import { cacheWritePatch } from "@/lib/cache-write-rate";

describe("cacheWritePatch", () => {
  test("sends nothing for an input nobody touched, so the stored rate is kept", () => {
    expect(cacheWritePatch("", "")).toEqual({});
    expect(cacheWritePatch("3.750000", "3.750000")).toEqual({});
    expect(cacheWritePatch(" 3.75 ", "3.75")).toEqual({});
  });

  test("sends the rate that was typed", () => {
    expect(cacheWritePatch("3.75", "")).toEqual({ cache_write_per_mtok: "3.75" });
    expect(cacheWritePatch("4.5", "3.750000")).toEqual({ cache_write_per_mtok: "4.5" });
    expect(cacheWritePatch(" 4.5 ", "3.750000")).toEqual({ cache_write_per_mtok: "4.5" });
  });

  test("sends null for a rate that was emptied, which clears it", () => {
    expect(cacheWritePatch("", "3.750000")).toEqual({ cache_write_per_mtok: null });
    expect(cacheWritePatch("  ", "3.750000")).toEqual({ cache_write_per_mtok: null });
  });

  test("keeps null on the wire, where an undefined key would be dropped", () => {
    expect(JSON.stringify(cacheWritePatch("", "3.75"))).toBe('{"cache_write_per_mtok":null}');
    expect(JSON.stringify(cacheWritePatch("", ""))).toBe("{}");
  });
});
