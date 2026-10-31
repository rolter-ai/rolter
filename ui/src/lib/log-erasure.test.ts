import { describe, expect, test } from "bun:test";

import { erasureRefetchInterval, erasureTimestamp } from "@/lib/log-erasure";

describe("erasureRefetchInterval", () => {
  test("keeps asking while the erasure is pending", () => {
    expect(erasureRefetchInterval({ status: "pending" })).toBeGreaterThan(0);
  });

  test("stops at every answer ClickHouse will not move, and before there is one", () => {
    expect(erasureRefetchInterval({ status: "completed" })).toBe(false);
    expect(erasureRefetchInterval({ status: "failed" })).toBe(false);
    expect(erasureRefetchInterval({ status: "unverified" })).toBe(false);
    expect(erasureRefetchInterval(undefined)).toBe(false);
  });
});

describe("erasureTimestamp", () => {
  test("reads a zone-less ClickHouse stamp as UTC", () => {
    expect(erasureTimestamp("2026-10-27 12:30:45.123")).toBe("2026-10-27T12:30:45.123Z");
  });

  test("keeps the instant of a stamp that names its zone", () => {
    expect(erasureTimestamp("2026-10-27T12:30:45.123Z")).toBe("2026-10-27T12:30:45.123Z");
    expect(erasureTimestamp("2026-10-27T14:30:45.000+02:00")).toBe("2026-10-27T12:30:45.000Z");
  });

  test("does not invent an instant for text that is not one", () => {
    expect(erasureTimestamp("yesterday")).toBeUndefined();
  });
});
