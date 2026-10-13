import { describe, expect, it } from "bun:test";

import { misplacedWildcards } from "./policy-patterns";

describe("misplacedWildcards", () => {
  it("accepts exact names, a trailing glob and a bare star", () => {
    expect(misplacedWildcards("gpt-4o\no1-*\n*\n\n  chat-*  ")).toEqual([]);
  });

  it("names a star in the middle or at the start", () => {
    expect(misplacedWildcards("o1-*-mini\n*-preview\ngpt-*")).toEqual(["o1-*-mini", "*-preview"]);
  });

  it("names a star before the trailing one", () => {
    expect(misplacedWildcards("a*b*")).toEqual(["a*b*"]);
  });
});
