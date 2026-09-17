import { describe, expect, it } from "bun:test";

import { highlight } from "@/lib/code-highlight";
import { HIGHLIGHT_CHAR_LIMIT, nodeText, type CodeNode } from "@/lib/code";

/** every token, however deep, as one string — what the reader ends up seeing */
const text = (nodes: CodeNode[]): string => nodeText(nodes);

const classNames = (nodes: CodeNode[]): string[] =>
  nodes.flatMap((n) =>
    typeof n === "string" ? [] : [n.className, ...classNames(n.children)],
  );

describe("highlight", () => {
  it("tokenises a known grammar into classed elements", () => {
    const nodes = highlight('{"a": 1}', "json");
    expect(nodes.length).toBeGreaterThan(1);
    expect(classNames(nodes).some((c) => c.includes("token"))).toBe(true);
  });

  it("keeps the source byte-for-byte, so colour is all that is added", () => {
    const source = "select id from providers where kind = 'openai';";
    expect(text(highlight(source, "sql"))).toBe(source);
  });

  it("returns plain text unhighlighted", () => {
    expect(highlight("just words", "text")).toEqual(["just words"]);
  });

  it("falls back to one text node past the size cap", () => {
    // tokenisable on purpose: a payload of filler would come back as a single
    // node either way, and would not notice the cap going missing
    const line = '{"a": 1}\n';
    const huge = line.repeat(Math.ceil((HIGHLIGHT_CHAR_LIMIT + 1) / line.length));
    expect(huge.length).toBeGreaterThan(HIGHLIGHT_CHAR_LIMIT);
    expect(highlight(huge, "json")).toEqual([huge]);
  });

  it("highlights right up to the cap", () => {
    const line = '{"a": 1}\n';
    const atLimit = line
      .repeat(Math.ceil(HIGHLIGHT_CHAR_LIMIT / line.length))
      .slice(0, HIGHLIGHT_CHAR_LIMIT);
    const nodes = highlight(atLimit, "json");
    expect(text(nodes)).toBe(atLimit);
    expect(nodes.length).toBeGreaterThan(1);
  });

  it("does not throw on input the grammar cannot parse", () => {
    const broken = "{{{ not json at all ]]]";
    expect(text(highlight(broken, "json"))).toBe(broken);
  });

  it("tokenises each language the code block offers", () => {
    const samples: [Parameters<typeof highlight>[1], string][] = [
      ["bash", "echo hi"],
      ["javascript", "const a = 1;"],
      ["typescript", "const a: number = 1;"],
      ["python", "def a(): pass"],
      ["yaml", "a: 1"],
      ["toml", "a = 1"],
      ["markdown", "# title"],
      ["csv", "a,b"],
      ["log", "ERROR boom"],
    ];
    for (const [language, source] of samples) {
      const nodes = highlight(source, language);
      expect(text(nodes)).toBe(source);
      expect(nodes.length).toBeGreaterThan(0);
    }
  });
});
