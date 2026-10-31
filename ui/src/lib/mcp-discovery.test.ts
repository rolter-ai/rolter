import { describe, expect, test } from "bun:test";

import {
  DISCOVERY_POLL_MS,
  discoveryOutstanding,
  discoveryReasonKey,
  discoveryRefetchInterval,
  discoveryView,
} from "@/lib/mcp-discovery";

describe("discoveryView", () => {
  test("a never-answered server is pending even with fallback tools", () => {
    expect(discoveryView({ tools_discovery_status: "pending", tools: ["a"] })).toBe("pending");
  });
  test("an answered empty manifest declares no tools, not pending", () => {
    expect(discoveryView({ tools_discovery_status: "discovered", tools: [] })).toBe("declaresNone");
  });
  test("an answered manifest with tools is discovered", () => {
    expect(discoveryView({ tools_discovery_status: "discovered", tools: ["a"] })).toBe(
      "discovered",
    );
  });
  test("failed wins whatever the kept manifest holds", () => {
    expect(discoveryView({ tools_discovery_status: "failed", tools: [] })).toBe("failed");
  });
});

describe("discoveryReasonKey", () => {
  test("maps a known code", () => {
    expect(discoveryReasonKey("needs_session")).toBe(
      "pages.mcpCatalog.discovery.reasons.needs_session",
    );
  });
  test("falls back for a code this build does not know", () => {
    expect(discoveryReasonKey("brand_new")).toBe("pages.mcpCatalog.discovery.reasons.unknown");
  });
  test("is null without a code", () => {
    expect(discoveryReasonKey(null)).toBeNull();
  });
});

describe("polling", () => {
  const at = (v: string | null) => ({ tools_discovery_attempted_at: v });
  test("polls while any row has not been attempted", () => {
    expect(discoveryOutstanding([at("2026-01-01T00:00:00Z"), at(null)])).toBe(true);
    expect(discoveryRefetchInterval([at(null)])).toBe(DISCOVERY_POLL_MS);
  });
  test("stops once every row has an attempt", () => {
    expect(discoveryRefetchInterval([at("2026-01-01T00:00:00Z")])).toBe(false);
  });
  test("does not poll an unread or empty list", () => {
    expect(discoveryRefetchInterval(undefined)).toBe(false);
    expect(discoveryRefetchInterval([])).toBe(false);
  });
});
