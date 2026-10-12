import { describe, expect, it } from "bun:test";

import { checkAll, checkSource } from "./check-raw-errors";

describe("the raw error rule", () => {
  it("flags a cast message, a mutation error message and errorDetail", () => {
    const source = [
      "<p>{(save.error as Error).message}</p>",
      "<p>{query.error.message}</p>",
      "detail: errorDetail(error),",
    ].join("\n");
    expect(checkSource(source, "a.tsx").violations.map((v) => v.line)).toEqual([1, 2, 3]);
  });

  it("accepts errorText and describeError", () => {
    const source = "<p>{errorText(save.error, t)}</p>\n<p>{describeError(e, t).message}</p>";
    expect(checkSource(source, "a.tsx").violations).toEqual([]);
  });

  it("honours a waiver with a reason on the line above, and reports it", () => {
    const source = "// raw-error-allow: not a server answer\nappend((e as Error).message)";
    const result = checkSource(source, "a.tsx");
    expect(result.violations).toEqual([]);
    expect(result.waived).toEqual([{ file: "a.tsx", line: 2, reason: "not a server answer" }]);
  });

  it("does not read a comment", () => {
    expect(checkSource("// (x as Error).message is banned", "a.tsx").violations).toEqual([]);
  });

  it("holds for the tree", () => {
    expect(checkAll().violations).toEqual([]);
  });
});
