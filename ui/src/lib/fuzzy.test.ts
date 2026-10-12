import { describe, expect, test } from "bun:test";

import { fuzzyMatch, rankSearchable } from "@/lib/fuzzy";

describe("fuzzyMatch", () => {
  test("a subsequence finds what a substring cannot", () => {
    expect(fuzzyMatch("openai", "oai")?.indices).toEqual([0, 4, 5]);
    expect(fuzzyMatch("openai", "xyz")).toBeNull();
  });

  test("an empty query matches with nothing highlighted", () => {
    expect(fuzzyMatch("openai", "  ")).toEqual({ score: 0, indices: [] });
  });

  test("case and diacritics do not matter, and positions stay the original's", () => {
    expect(fuzzyMatch("Éclair", "ecl")?.indices).toEqual([0, 1, 2]);
  });

  test("a precomposed Hangul syllable keeps later positions aligned", () => {
    expect(fuzzyMatch("한글 abc", "b")?.indices).toEqual([4]);
  });

  test("underscores and slashes start a word", () => {
    const initials = fuzzyMatch("openai_compatible", "oc")!;
    const scattered = fuzzyMatch("ollama", "ol")!;
    expect(initials.indices).toEqual([0, 7]);
    expect(scattered.score).toBeGreaterThan(initials.score - 100);
  });
});

describe("rankSearchable", () => {
  const items = ["anthropic", "openai_compatible", "openai", "ollama"];
  const describe_ = (label: string) => ({ label });

  test("best match first, a shorter label breaking the tie", () => {
    expect(rankSearchable(items, "openai", describe_).map((r) => r.item)).toEqual([
      "openai",
      "openai_compatible",
    ]);
  });

  test("an empty query keeps the caller's order", () => {
    expect(rankSearchable(items, "", describe_).map((r) => r.item)).toEqual(items);
  });

  test("a secondary text finds a row but never outranks the label", () => {
    const rows = [{ label: "alpha", also: ["beta"] }, { label: "beta" }];
    expect(rankSearchable(rows, "beta", (r) => r).map((r) => r.item.label)).toEqual([
      "beta",
      "alpha",
    ]);
  });
});
