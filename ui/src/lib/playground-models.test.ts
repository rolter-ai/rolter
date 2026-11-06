import { describe, expect, it } from "bun:test";

import type { EffectiveModelDto, UptimeRow } from "@/lib/api";
import { pickModel, routeVerdicts } from "./playground-models";

const route = (model: string, targets: [string, string][], org?: string): EffectiveModelDto => ({
  model,
  strategy: "round_robin",
  targets: targets.length,
  route_targets: targets.map(([provider, upstream]) => ({ provider, model: upstream, weight: 1 })),
  source: "db",
  ...(org ? { org_id: org } : {}),
});

const health = (provider: string, target: string, uptime: number): UptimeRow => ({
  provider,
  target_id: target,
  grain: "target",
  sources: ["passive"],
  events: 10,
  ok: Math.round(uptime * 10),
  errors: 10 - Math.round(uptime * 10),
  timeouts: 0,
  uptime,
  failure_rate: 1 - uptime,
  error_budget_burn: (1 - uptime) / 0.01,
  sla_breached: uptime < 0.99 ? 1 : 0,
  last_event: "2026-11-05T10:00:00Z",
});

describe("routeVerdicts", () => {
  it("marks a route whose every target is below the SLA as failing", () => {
    const v = routeVerdicts(
      [route("claude", [["anthropic", "claude-sonnet-4"]])],
      [health("anthropic", "claude-sonnet-4", 0)],
    );
    expect(v.get("claude")).toBe("failing");
  });

  it("calls a route working when one target answers", () => {
    const v = routeVerdicts(
      [
        route("qwen", [
          ["neuraldeep", "qwen3.6"],
          ["yandex", "gpt://f/qwen"],
        ]),
      ],
      [health("neuraldeep", "qwen3.6", 0), health("yandex", "gpt://f/qwen", 1)],
    );
    expect(v.get("qwen")).toBe("working");
  });

  it("leaves a route with no health unjudged", () => {
    const v = routeVerdicts([route("kimi", [["openrouter", "kimi-k2"]])], []);
    expect(v.has("kimi")).toBe(false);
  });

  it("judges nothing without a rollup", () => {
    expect(routeVerdicts([route("kimi", [["openrouter", "kimi-k2"]])], undefined).size).toBe(0);
  });

  it("fails a name only when every project's route of it fails", () => {
    const v = routeVerdicts(
      [route("gpt-4o", [["openai", "gpt-4o"]]), route("gpt-4o", [["azure", "gpt-4o"]])],
      [health("openai", "gpt-4o", 0)],
    );
    // the azure route has no health of its own, so the name is not failing
    expect(v.has("gpt-4o")).toBe(false);
  });
});

describe("pickModel", () => {
  const verdicts = new Map([
    ["claude", "failing"],
    ["qwen", "working"],
  ] as const);

  it("prefers a route that answers its traffic", () => {
    expect(pickModel(["claude", "kimi", "qwen"], new Map(verdicts))).toBe("qwen");
  });

  it("falls back to a route nothing is known against, skipping a failing one", () => {
    expect(pickModel(["claude", "kimi"], new Map(verdicts))).toBe("kimi");
  });

  it("picks nothing when every route is failing", () => {
    expect(pickModel(["claude"], new Map(verdicts))).toBeNull();
  });
});
