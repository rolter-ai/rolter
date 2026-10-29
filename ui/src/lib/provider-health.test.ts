import { describe, expect, it } from "bun:test";

import type { UptimeRow } from "@/lib/api";
import { groupByProvider, rollUpProviders } from "./provider-health";

const row = (provider: string, uptime: number, over: Partial<UptimeRow> = {}): UptimeRow => ({
  provider,
  target_id: provider,
  grain: "provider",
  sources: ["probe"],
  events: 100,
  ok: Math.round(uptime * 100),
  errors: 100 - Math.round(uptime * 100),
  timeouts: 0,
  uptime,
  failure_rate: 1 - uptime,
  error_budget_burn: 0,
  sla_breached: 0,
  last_event: "2026-08-06T10:00:00Z",
  ...over,
});

describe("rollUpProviders", () => {
  it("counts the providers meeting the SLA of those that reported, and names the lowest", () => {
    const roll = rollUpProviders([row("a", 0.9995), row("b", 0.999)]);
    expect(roll).toEqual({
      total: 2,
      up: 2,
      breaching: [],
      worst: { provider: "b", uptime: 0.999, breaching: false },
    });
  });

  it("does not count a breaching provider as up, and names the worst of them", () => {
    const roll = rollUpProviders([
      row("a", 0.9995),
      row("b", 0.96, { sla_breached: 1 }),
      row("c", 0.9, { sla_breached: 1 }),
    ]);
    expect(roll.up).toBe(1);
    expect(roll.total).toBe(3);
    expect(roll.breaching.sort()).toEqual(["b", "c"]);
    expect(roll.worst).toEqual({ provider: "c", uptime: 0.9, breaching: true });
  });

  it("leaves out a provider that reported nothing: no event is no data, not a perfect score", () => {
    const roll = rollUpProviders([row("a", 0.9995), row("silent", 1, { events: 0, ok: 0 })]);
    expect(roll.total).toBe(1);
  });

  it("rolls a provider with only target rows up from them", () => {
    const roll = rollUpProviders([
      row("a", 0.5, { grain: "target", target_id: "a/gpt-4o", events: 10, ok: 5, sla_breached: 1 }),
      row("a", 1, { grain: "target", target_id: "a/gpt-4o-mini", events: 10, ok: 10 }),
    ]);
    // one provider, breaching because one of its targets is
    expect(roll).toMatchObject({ total: 1, up: 0, breaching: ["a"] });
  });

  it("reports nothing for no rows", () => {
    expect(rollUpProviders([])).toEqual({ total: 0, up: 0, breaching: [], worst: undefined });
  });
});

// two organizations may each have an `openai`, and the rollups answer a series
// for each (#3441, #3460)
describe("two organizations sharing a provider name", () => {
  it("are two groups, each with its own rows", () => {
    const groups = groupByProvider([
      row("openai", 0.5, { org_id: "org-a", sla_breached: 1 }),
      row("openai", 0.999, { org_id: "org-b" }),
      row("openai", 0.98, { org_id: "org-b", grain: "target", target_id: "gpt-4o" }),
    ]);
    expect(groups.map((g) => [g.org_id, g.provider, g.targets.length])).toEqual([
      ["org-a", "openai", 0],
      ["org-b", "openai", 1],
    ]);
    expect(rollUpProviders(groups.flatMap((g) => (g.overall ? [g.overall] : []))).total).toBe(2);
  });
});
