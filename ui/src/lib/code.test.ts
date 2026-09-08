import { describe, expect, test } from "bun:test";

import {
  CODE_LANGUAGES,
  nodeText,
  resolveLanguage,
  splitLines,
  type CodeNode,
} from "./code";
import { highlight } from "./code-highlight";

describe("resolveLanguage", () => {
  test("passes a canonical name through", () => {
    for (const language of CODE_LANGUAGES) {
      expect(resolveLanguage(language)).toBe(language);
    }
  });

  test("maps the tags a model actually writes", () => {
    expect(resolveLanguage("js")).toBe("javascript");
    expect(resolveLanguage("TS")).toBe("typescript");
    expect(resolveLanguage("sh")).toBe("bash");
    expect(resolveLanguage("console")).toBe("bash");
    expect(resolveLanguage("yml")).toBe("yaml");
    expect(resolveLanguage("py")).toBe("python");
  });

  test("ignores metadata after the tag", () => {
    expect(resolveLanguage("ts title=\"server.ts\"")).toBe("typescript");
    expect(resolveLanguage("json,copy")).toBe("json");
  });

  // an unknown fence still has to render; falling back is the whole point
  test("falls back to text for anything unknown or absent", () => {
    expect(resolveLanguage(undefined)).toBe("text");
    expect(resolveLanguage(null)).toBe("text");
    expect(resolveLanguage("")).toBe("text");
    expect(resolveLanguage("brainfuck")).toBe("text");
  });
});

describe("splitLines", () => {
  const lines = (nodes: CodeNode[]) => splitLines(nodes).map((line) => nodeText(line));

  test("cuts plain text on newlines", () => {
    expect(lines(["a\nb\nc"])).toEqual(["a", "b", "c"]);
  });

  test("keeps a trailing blank line, so the gutter matches the source", () => {
    expect(lines(["a\n"])).toEqual(["a", ""]);
  });

  test("reopens a token that straddles a newline", () => {
    const tree: CodeNode[] = [
      { className: "token comment", children: ["/* one\ntwo */"] },
      " tail",
    ];
    const split = splitLines(tree);
    expect(split.map(nodeText)).toEqual(["/* one", "two */ tail"]);
    // both halves still carry the class, or the second line loses its colour
    expect(split[0][0]).toMatchObject({ className: "token comment" });
    expect(split[1][0]).toMatchObject({ className: "token comment" });
  });

  test("round-trips to the source text", () => {
    const source = "{\n  \"a\": 1\n}\n";
    const split = splitLines(highlight(source, "json"));
    expect(split.map(nodeText).join("\n")).toBe(source);
  });
});

describe("highlight", () => {
  test("tokenises json into the classes the stylesheet colours", () => {
    const classes = new Set<string>();
    const walk = (nodes: readonly CodeNode[]) => {
      for (const node of nodes) {
        if (typeof node === "string") continue;
        classes.add(node.className);
        walk(node.children);
      }
    };
    walk(highlight('{"model":"gpt-4o","n":2,"ok":true}', "json"));
    expect(classes).toContain("token property");
    expect(classes).toContain("token string");
    expect(classes).toContain("token number");
    expect(classes).toContain("token boolean");
  });

  test("marks log levels, which is what makes a log worth colouring", () => {
    const flat = JSON.stringify(highlight("2026-01-01 ERROR upstream timeout", "log"));
    expect(flat).toContain("token level error");
  });

  test("leaves plain text alone rather than loading a grammar for it", () => {
    expect(highlight("just words", "text")).toEqual(["just words"]);
  });

  // colour is the thing to give up when the alternative is a stalled main
  // thread; the payload itself is never truncated
  test("skips a payload past the size cap without losing it", () => {
    const huge = `{"a":"${"x".repeat(200_000)}"}`;
    expect(highlight(huge, "json")).toEqual([huge]);
  });

  test("survives every language it advertises", () => {
    for (const language of CODE_LANGUAGES) {
      expect(nodeText(highlight("a = 1\n# two\n", language))).toBe("a = 1\n# two\n");
    }
  });
});
