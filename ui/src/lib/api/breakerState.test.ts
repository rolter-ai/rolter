import { describe, expect, test } from "bun:test";

import {
  breakerFor,
  breakerLabel,
  isTripped,
  worstBreaker,
  type BreakerTargetDto,
} from "./breakerState";

function target(over: Partial<BreakerTargetDto>): BreakerTargetDto {
  return {
    model: "gpt-4o",
    target_index: 0,
    provider: "openai",
    upstream_model: "gpt-4o",
    state: "open",
    nodes_open: 2,
    nodes_half_open: 0,
    nodes: [],
    ...over,
  };
}

describe("breakerFor", () => {
  const state = { targets: [target({}), target({ target_index: 1, state: "half_open" })] };

  test("matches on the model and the target's position", () => {
    expect(breakerFor(state, "gpt-4o", 1)?.state).toBe("half_open");
    expect(breakerFor(state, "gpt-4o", 0)?.state).toBe("open");
  });

  test("an unlisted target is closed everywhere, so there is nothing to return", () => {
    expect(breakerFor(state, "gpt-4o", 2)).toBeUndefined();
    expect(breakerFor(state, "other", 0)).toBeUndefined();
    expect(breakerFor(undefined, "gpt-4o", 0)).toBeUndefined();
  });
});

describe("isTripped", () => {
  test("only open and half-open count; a closed row is failing but untripped", () => {
    expect(isTripped(target({ state: "open" }))).toBe(true);
    expect(isTripped(target({ state: "half_open" }))).toBe(true);
    expect(isTripped(target({ state: "closed" }))).toBe(false);
    expect(isTripped(undefined)).toBe(false);
  });
});

describe("worstBreaker", () => {
  test("open beats half-open beats closed", () => {
    expect(worstBreaker([])).toBe("closed");
    expect(worstBreaker(["closed", "half_open"])).toBe("half_open");
    expect(worstBreaker(["half_open", "open", "closed"])).toBe("open");
  });
});

describe("breakerLabel", () => {
  test("counts the gateways the state holds on", () => {
    expect(breakerLabel(target({ state: "open", nodes_open: 2 }))).toEqual({
      key: "breaker.open",
      count: 2,
    });
    expect(breakerLabel(target({ state: "half_open", nodes_open: 0, nodes_half_open: 1 }))).toEqual(
      { key: "breaker.probing", count: 1 },
    );
  });

  test("a closed or absent breaker has no label", () => {
    expect(breakerLabel(target({ state: "closed" }))).toBeUndefined();
    expect(breakerLabel(undefined)).toBeUndefined();
  });
});
