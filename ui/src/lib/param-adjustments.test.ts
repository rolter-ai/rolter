import { describe, expect, test } from "bun:test";

import { parseAdjustment } from "./param-adjustments";

describe("parseAdjustment", () => {
  test("splits on the first colon", () => {
    expect(parseAdjustment("dropped:top_k")).toEqual({ disposition: "dropped", param: "top_k" });
    expect(parseAdjustment("translated:tool_choice")).toEqual({
      disposition: "translated",
      param: "tool_choice",
    });
  });

  test("an entry with no colon is a bare parameter", () => {
    expect(parseAdjustment("top_k")).toEqual({ disposition: "", param: "top_k" });
  });
});
