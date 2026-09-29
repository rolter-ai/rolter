import { describe, expect, it } from "bun:test";

import type { UptimeRow } from "@/lib/api";
import { targetHealth, targetViews } from "@/lib/route-targets";

const row = (patch: Partial<UptimeRow>): UptimeRow => ({
  provider: "sim-a",
  target_id: "sim-a",
  grain: "provider",
  sources: ["probe"],
  events: 100,
  ok: 100,
  errors: 0,
  timeouts: 0,
  uptime: 1,
  failure_rate: 0,
  error_budget_burn: 0,
  sla_breached: 0,
  last_event: "2026-09-29T00:00:00Z",
  ...patch,
});

describe("targetHealth", () => {
  it("prefers the passive row for the exact provider and upstream model", () => {
    const rows = [
      row({ uptime: 0.999 }),
      row({ grain: "target", target_id: "llama-70b", uptime: 0.95, sla_breached: 1 }),
    ];
    expect(targetHealth(rows, "sim-a", "llama-70b")).toEqual({ uptime: 0.95, breached: true });
  });

  it("falls back to the provider's probe row for a target with no traffic yet", () => {
    expect(targetHealth([row({ uptime: 0.999 })], "sim-a", "llama-70b")).toEqual({
      uptime: 0.999,
      breached: false,
    });
  });

  it("does not borrow another model's traffic on the same provider", () => {
    const rows = [row({ grain: "target", target_id: "other-model", uptime: 0.5 })];
    expect(targetHealth(rows, "sim-a", "llama-70b")).toBeNull();
  });

  it("reads the grain from the id on a control plane without the column", () => {
    const legacy = row({ uptime: 0.9, sla_breached: 1 }) as Partial<UptimeRow>;
    delete legacy.grain;
    expect(targetHealth([legacy as UptimeRow], "sim-a", "llama-70b")?.breached).toBe(true);
  });

  it("knows nothing when the rollup was not read", () => {
    expect(targetHealth(undefined, "sim-a", "llama-70b")).toBeNull();
  });
});

describe("targetViews", () => {
  const route = {
    model: "llama-70b",
    strategy: "cache_aware",
    targets: [
      { provider: "sim-a", model: null, weight: 1 },
      { provider: "sim-b", model: "meta-llama/Llama-3.1-70B", weight: 2 },
    ],
  };

  it("sends the route's public name where a target names no model of its own", () => {
    expect(targetViews(route).map((v) => v.upstream)).toEqual([
      "llama-70b",
      "meta-llama/Llama-3.1-70B",
    ]);
  });

  it("leaves health off entirely when the rollup was not asked for", () => {
    expect(targetViews(route)[0]).not.toHaveProperty("health");
    expect(targetViews(route, [])[0].health).toBeNull();
  });
});
