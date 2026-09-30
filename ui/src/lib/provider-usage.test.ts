import { describe, expect, test } from "bun:test";

import type { GatewayConfigDto, ProviderGroupRow } from "@/lib/api";
import { providerUsage } from "@/lib/provider-usage";

const provider = { id: "p-1", name: "openai-prod" };

const config = (routes: GatewayConfigDto["routes"]) => ({ routes });

const target = (name: string) => ({ provider: name, weight: 1 });

const group = (name: string, members: string[]): ProviderGroupRow => ({
  id: `g-${name}`,
  org_id: "org-1",
  name,
  slug: name,
  strategy: "round_robin",
  created_at: "2026-01-01T00:00:00Z",
  members: members.map((id, position) => ({
    group_id: `g-${name}`,
    provider_id: id,
    provider_name: id,
    weight: 1,
    position,
  })),
});

describe("providerUsage", () => {
  test("names the routes that target the provider and flags the one it is the only target of", () => {
    const usage = providerUsage(
      provider,
      config([
        { model: "gpt-4o", strategy: "weighted", targets: [target("openai-prod")] },
        {
          model: "chat",
          strategy: "weighted",
          targets: [target("openai-prod"), target("anthropic-eu")],
        },
        { model: "claude", strategy: "weighted", targets: [target("anthropic-eu")] },
      ]),
      [],
    );
    expect(usage.routes).toEqual([
      { name: "chat", only: false },
      { name: "gpt-4o", only: true },
    ]);
    expect(usage.groups).toEqual([]);
  });

  test("names the groups the provider is a member of", () => {
    const usage = providerUsage(provider, config([]), [
      group("eu-fleet", ["p-1", "p-2"]),
      group("solo", ["p-1"]),
      group("other", ["p-2"]),
    ]);
    expect(usage.groups).toEqual([
      { name: "eu-fleet", only: false },
      { name: "solo", only: true },
    ]);
  });

  test("one line for a model that repeats across projects, flagged if any would be left empty", () => {
    const usage = providerUsage(
      provider,
      config([
        {
          model: "gpt-4o",
          strategy: "weighted",
          targets: [target("openai-prod"), target("anthropic-eu")],
        },
        { model: "gpt-4o", strategy: "weighted", targets: [target("openai-prod")] },
      ]),
      [],
    );
    expect(usage.routes).toEqual([{ name: "gpt-4o", only: true }]);
  });

  test("reads an answer with no routes or groups as no use, not as a failure", () => {
    expect(providerUsage(provider, undefined, undefined)).toEqual({ routes: [], groups: [] });
    // a stub or an older control plane may answer the config with a bare array
    expect(providerUsage(provider, [] as unknown as GatewayConfigDto, [])).toEqual({
      routes: [],
      groups: [],
    });
  });
});
