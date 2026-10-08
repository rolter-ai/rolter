import { describe, expect, it } from "bun:test";

import { SLUG_MAX_LEN, slugify } from "./slug";

// the cases of `slugify_normalizes` and its siblings in crates/rolter-core/src/slug.rs
describe("slugify", () => {
  it("derives what the control plane derives", () => {
    expect(slugify("OpenAI")).toBe("openai");
    expect(slugify("vLLM MSK!")).toBe("vllm-msk");
    expect(slugify("  multi  space ")).toBe("multi-space");
    expect(slugify("trailing-")).toBe("trailing");
    expect(slugify("非ascii")).toBe("ascii");
    expect(slugify("Kelvin")).toBe("kelvin");
    expect(slugify("İProvider")).toBe("i-provider");
  });

  it("derives nothing from a name with no ascii letter or digit", () => {
    expect(slugify("非")).toBe("");
    expect(slugify("")).toBe("");
    expect(slugify(" - ")).toBe("");
  });

  it("stops at the longest slug the control plane accepts", () => {
    expect(slugify("x".repeat(100))).toHaveLength(SLUG_MAX_LEN);
  });
});
