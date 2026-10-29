import { describe, expect, it } from "bun:test";

import type { TimelineRow, UptimeRow } from "@/lib/api";
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
    expect(targetHealth(rows, "sim-a", "llama-70b")).toEqual({
      uptime: 0.95,
      breached: true,
      state: "breaching",
    });
  });

  it("falls back to the provider's probe row for a target with no traffic yet", () => {
    expect(targetHealth([row({ uptime: 0.999 })], "sim-a", "llama-70b")).toEqual({
      uptime: 0.999,
      breached: false,
      state: "within",
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

  it("reads the last two hours: at risk when they burn faster than the SLA allows", () => {
    const bucket = (hour: string, errors: number): TimelineRow => ({
      bucket: `2026-08-06T${hour}:00:00Z`,
      provider: "sim-a",
      target_id: "sim-a",
      grain: "provider",
      events: 100,
      ok: 100 - errors,
      errors,
      timeouts: 0,
    });
    const rows = [row({ uptime: 0.995 })];
    const burning = [bucket("08", 0), bucket("10", 3), bucket("11", 3)];
    expect(targetHealth(rows, "sim-a", "llama-70b", burning)?.state).toBe("atRisk");
    const early = [bucket("03", 5), bucket("10", 0), bucket("11", 0)];
    expect(targetHealth(rows, "sim-a", "llama-70b", early)?.state).toBe("within");
    expect(targetHealth(rows, "sim-a", "llama-70b")?.state).toBe("within");
    const breached = [row({ uptime: 0.9, sla_breached: 1 })];
    expect(targetHealth(breached, "sim-a", "llama-70b", early)?.state).toBe("breaching");
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

// two organizations may each have a provider and a route of one name (#3460)
describe("two organizations sharing a name", () => {
  const rows = [
    row({ org_id: "org-a", uptime: 0.5, sla_breached: 1 }),
    row({ org_id: "org-b", uptime: 0.999 }),
    row({ org_id: "", provider: "global", target_id: "global", uptime: 0.97, sla_breached: 1 }),
  ];

  it("reads each organization's own provider", () => {
    expect(targetHealth(rows, "sim-a", "m", undefined, "org-a")?.uptime).toBe(0.5);
    expect(targetHealth(rows, "sim-a", "m", undefined, "org-b")?.uptime).toBe(0.999);
  });

  it("falls back to a config-file provider, never another organization's", () => {
    expect(targetHealth(rows, "sim-a", "m", undefined, "org-c")).toBeNull();
    expect(targetHealth(rows, "global", "m", undefined, "org-a")?.uptime).toBe(0.97);
  });

  it("matches on the name alone for a control plane that predates org_id", () => {
    const old = [row({ uptime: 0.9 })];
    expect(targetHealth(old, "sim-a", "m", undefined, "org-a")?.uptime).toBe(0.9);
  });

  it("gives each route its own breaker", () => {
    const breaker = (org: string, state: "open" | "half_open") => ({
      org_id: org,
      model: "gpt-4o",
      target_index: 0,
      provider: "sim-a",
      upstream_model: "gpt-4o",
      state,
      nodes_open: state === "open" ? 1 : 0,
      nodes_half_open: state === "half_open" ? 1 : 0,
      nodes: [],
    });
    const breakers = { targets: [breaker("org-a", "open"), breaker("org-b", "half_open")] };
    const route = { model: "gpt-4o", targets: [{ provider: "sim-a", model: null, weight: 1 }] };
    expect(targetViews(route, rows, undefined, breakers, "org-a")[0].breaker?.state).toBe("open");
    expect(targetViews(route, rows, undefined, breakers, "org-b")[0].breaker?.state).toBe(
      "half_open",
    );
    expect(targetViews(route, rows, undefined, breakers, "")[0].breaker).toBeUndefined();
  });
});
