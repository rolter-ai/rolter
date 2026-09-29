import { describe, expect, it } from "bun:test";

import { STRATEGIES } from "@/lib/api";
import { strategyHintKey, strategyOptions, strategyTone, STRATEGY_TONE } from "@/lib/strategies";

describe("strategyOptions", () => {
  it("offers every strategy the backend accepts except the deployment-wide one", () => {
    const options = strategyOptions();
    for (const s of STRATEGIES) {
      if (s === "adaptive") continue;
      expect(options).toContain(s);
    }
    expect(options).not.toContain("adaptive");
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
  it("keeps a current value it would not otherwise offer", () => {
    const options = strategyOptions("adaptive");
    expect(options[0]).toBe("adaptive");
  });

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

describe("strategyTone", () => {
  // a missing tone silently rendered the round_robin pill, which made two
  // different strategies look identical
  it("has a tone for every strategy the backend accepts", () => {
    for (const s of STRATEGIES) {
      expect(STRATEGY_TONE[s]).toBeDefined();
    }
  });

  it("falls back rather than crashing on an unknown strategy", () => {
    expect(strategyTone("strategy_from_the_future")).toEqual(STRATEGY_TONE.round_robin);
  });
});
