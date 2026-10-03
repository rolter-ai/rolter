import { describe, expect, test } from "bun:test";

import { modelColor, rankedByRequests } from "./model-colors";

const row = (model: string, requests: number) => ({ model, requests });

describe("rankedByRequests", () => {
  test("orders busiest first whatever order the read arrived in", () => {
    // by cost, as the control plane answers: the cheap busy model comes last
    const byCost = [row("claude-sonnet-4", 48), row("gpt-4o", 84), row("haiku", 120)];
    expect(rankedByRequests(byCost).map((m) => m.model)).toEqual([
      "haiku",
      "gpt-4o",
      "claude-sonnet-4",
    ]);
  });

  test("breaks a tie by name, so the order does not depend on arrival", () => {
    const a = rankedByRequests([row("b", 10), row("a", 10), row("c", 10)]);
    const b = rankedByRequests([row("c", 10), row("b", 10), row("a", 10)]);
    expect(a.map((m) => m.model)).toEqual(["a", "b", "c"]);
    expect(b).toEqual(a);
  });

  test("does not reorder the list it was given", () => {
    const given = [row("b", 1), row("a", 2)];
    rankedByRequests(given);
    expect(given.map((m) => m.model)).toEqual(["b", "a"]);
  });
});

describe("modelColor", () => {
  test("walks the shared sequence in rank order", () => {
    expect(modelColor(0)).toBe("var(--chart-1)");
    expect(modelColor(1)).toBe("var(--chart-2)");
    expect(modelColor(7)).toBe("var(--chart-8)");
  });

  test("repeats the last colour once the sequence is spent, never the first", () => {
    expect(modelColor(8)).toBe("var(--chart-8)");
    expect(modelColor(40)).toBe("var(--chart-8)");
  });
});
