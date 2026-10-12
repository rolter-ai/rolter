import { describe, expect, it } from "bun:test";

import { STRATEGIES } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import {
  strategyDescriptionKey,
  strategyDocsPage,
  strategyGroup,
  strategyHintKey,
  strategyOptions,
  strategyPolicyPath,
  usesWeights,
} from "@/lib/strategies";

describe("strategyOptions", () => {
  it("offers every strategy the backend accepts, adaptive included", () => {
    const options = strategyOptions();
    for (const s of STRATEGIES) expect(options).toContain(s);
    expect(options).toContain("adaptive");
  });

  it("lists each strategy once, the simple group first", () => {
    const options = strategyOptions();
    expect(new Set(options).size).toBe(options.length);
    expect(options[0]).toBe("round_robin");
  });

  // the five #897 named: they work over the API and in rolter.toml, so a
  // dashboard that cannot produce them cannot express its own backend
  it("offers the strategies the dashboard used to be unable to select", () => {
    const options = strategyOptions();
    for (const s of ["cheapest", "fastest", "precise_cache_aware", "lmcache_aware"]) {
      expect(options).toContain(s);
    }
  });

  // the load-bearing guarantee: a picker that cannot show a value must not be
  // able to destroy it either
  it("keeps a current value it has never heard of", () => {
    const options = strategyOptions("strategy_from_the_future");
    expect(options[0]).toBe("strategy_from_the_future");
    // and does not lose the real ones in the process
    expect(options).toContain("round_robin");
  });

  it("does not duplicate a current value that is already offered", () => {
    const options = strategyOptions("weighted");
    expect(options.filter((s) => s === "weighted")).toHaveLength(1);
  });
});

describe("strategyHintKey", () => {
  it("flags the strategies that silently degrade without telemetry", () => {
    expect(strategyHintKey("precise_cache_aware")).toBe(
      "pages.routing.strategyHints.needsTelemetry",
    );
    expect(strategyHintKey("lmcache_aware")).toBe("pages.routing.strategyHints.needsTelemetry");
  });

  it("explains that adaptive is controlled elsewhere", () => {
    expect(strategyHintKey("adaptive")).toBe("pages.routing.strategyHints.deploymentWide");
  });

  it("says nothing about a strategy that needs no caveat", () => {
    expect(strategyHintKey("round_robin")).toBeNull();
    expect(strategyHintKey("cheapest")).toBeNull();
  });
});

describe("strategyDocsPage", () => {
  // the telemetry caveat is the one that sends an operator somewhere else to
  // finish the job, so it is the one that links the page explaining it (#2137)
  it("points the telemetry strategies at the cache-aware routing page", () => {
    expect(strategyDocsPage("precise_cache_aware")).toBe("cacheAwareRouting");
    expect(strategyDocsPage("lmcache_aware")).toBe("cacheAwareRouting");
  });

  it("links nothing for a caveat the hint already finishes", () => {
    expect(strategyDocsPage("adaptive")).toBeNull();
    expect(strategyDocsPage("round_robin")).toBeNull();
  });
});

describe("usesWeights", () => {
  // the strategies `build_with_stats` hands the weight slice to; the rest are
  // built from the target count and never read a weight (#1979)
  it("is true only where the balancer reads the weights", () => {
    expect(STRATEGIES.filter((s) => usesWeights(s))).toEqual(["weighted", "pipeline", "adaptive"]);
  });

  it("does not claim a weighted split for a strategy it has never heard of", () => {
    expect(usesWeights("strategy_from_the_future")).toBe(false);
  });
});

describe("strategy descriptions (#2136)", () => {
  const catalog = en as unknown as {
    strategies: { descriptions: Record<string, string>; groups: Record<string, string> };
  };

  it("places every strategy in a group and gives it a one-line description", () => {
    for (const s of STRATEGIES) {
      const group = strategyGroup(s);
      expect(group).not.toBeNull();
      expect(catalog.strategies.groups[group as string]).toBeTruthy();
      const key = strategyDescriptionKey(s) as string;
      expect(catalog.strategies.descriptions[key.split(".").pop() as string]).toBeTruthy();
    }
  });

  it("has no description for a value it does not know", () => {
    expect(strategyGroup("strategy_from_the_future")).toBeNull();
    expect(strategyDescriptionKey("strategy_from_the_future")).toBeNull();
  });

  it("sends adaptive to its settings and nothing else", () => {
    expect(strategyPolicyPath("adaptive")).toBe("/adaptive-settings");
    expect(strategyPolicyPath("round_robin")).toBeNull();
  });
});
