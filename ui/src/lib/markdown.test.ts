import { describe, expect, test } from "bun:test";

import { parseMarkdown, safeHref, type MdBlock } from "./markdown";

const types = (blocks: MdBlock[]) => blocks.map((b) => b.type);

describe("safeHref", () => {
  test("allows the schemes a reply may legitimately link to", () => {
    expect(safeHref("https://example.test/x")).toBe("https://example.test/x");
    expect(safeHref("http://example.test")).toBe("http://example.test");
    expect(safeHref("mailto:ops@example.test")).toBe("mailto:ops@example.test");
  });

  test("allows a relative link, which cannot leave the dashboard", () => {
    expect(safeHref("/logs")).toBe("/logs");
    expect(safeHref("#section")).toBe("#section");
  });

  test("refuses everything that could execute or smuggle", () => {
    expect(safeHref("javascript:alert(1)")).toBeNull();
    expect(safeHref("JavaScript:alert(1)")).toBeNull();
    expect(safeHref("data:text/html;base64,PHNjcmlwdD4=")).toBeNull();
    expect(safeHref("vbscript:msgbox")).toBeNull();
    expect(safeHref("//evil.test/x")).toBeNull();
    expect(safeHref("")).toBeNull();
    expect(safeHref(undefined)).toBeNull();
  });
});

describe("parseMarkdown", () => {
  test("reads the blocks a model reply is made of", () => {
    const blocks = parseMarkdown(
      "# Title\n\ntext\n\n- one\n- two\n\n| a | b |\n| - | - |\n| 1 | 2 |\n",
    );
    expect(types(blocks)).toEqual(["heading", "paragraph", "list", "table"]);
  });

  test("resolves a fence tag to a language the highlighter knows", () => {
    const [block] = parseMarkdown("```js\nconst a = 1;\n```");
    expect(block).toEqual({ type: "code", language: "javascript", value: "const a = 1;" });
  });

  // streaming: the fence has not closed yet, and the column must not break
  test("treats an unterminated fence as the code block it is becoming", () => {
    const blocks = parseMarkdown("here you go:\n\n```json\n{\n  \"a\": 1");
    expect(types(blocks)).toEqual(["paragraph", "code"]);
    expect(blocks[1]).toMatchObject({ type: "code", language: "json" });
  });

  test("every prefix of a reply parses, so no chunk boundary breaks it", () => {
    const reply = "## Result\n\n```ts\nexport const x = 1;\n```\n\n| a |\n| - |\n| 1 |\n";
    for (let i = 1; i <= reply.length; i++) {
      expect(() => parseMarkdown(reply.slice(0, i))).not.toThrow();
    }
  });

  describe("untrusted markup", () => {
    test("keeps raw html as text rather than markup", () => {
      const blocks = parseMarkdown("<script>alert(1)</script>");
      expect(blocks).toEqual([{ type: "raw", value: "<script>alert(1)</script>" }]);
    });

    test("keeps an inline tag as text too", () => {
      const [block] = parseMarkdown("hello <img src=x onerror=alert(1)> world");
      expect(block.type).toBe("paragraph");
      const text = block.type === "paragraph" ? block.children.map((c) => JSON.stringify(c)).join("") : "";
      expect(text).toContain("onerror=alert(1)");
      // there is no node type that could become an element
      expect(text).not.toContain('"type":"image"');
    });

    test("strips the href off a javascript: link but keeps the label", () => {
      const [block] = parseMarkdown("[click me](javascript:alert(1))");
      expect(block).toMatchObject({
        type: "paragraph",
        children: [{ type: "link", href: null }],
      });
    });

    test("never emits an image as anything a browser would fetch", () => {
      const [block] = parseMarkdown("![alt text](https://evil.test/pixel.png)");
      expect(block).toMatchObject({
        type: "paragraph",
        children: [{ type: "image", alt: "alt text", href: "https://evil.test/pixel.png" }],
      });
    });
  });

  test("returns the source rather than nothing when handed junk", () => {
    expect(parseMarkdown("")).toEqual([]);
    expect(types(parseMarkdown("   "))).toEqual([]);
  });
});
