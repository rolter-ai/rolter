import { describe, expect, test } from "bun:test";

import { countChanges, diffFields, diffLines, foldUnchanged, pairLines } from "@/lib/text-diff";

describe("diffLines", () => {
  test("identical texts are all unchanged, numbered on both sides", () => {
    const lines = diffLines("a\nb\n", "a\nb\n");
    expect(lines.map((line) => line.kind)).toEqual(["same", "same"]);
    expect(lines[1]).toMatchObject({ oldNo: 2, newNo: 2 });
  });

  test("a reworded line is one removal and one addition", () => {
    const lines = diffLines("one\ntwo\nthree", "one\n2\nthree");
    expect(lines.map((line) => [line.kind, line.text])).toEqual([
      ["same", "one"],
      ["del", "two"],
      ["add", "2"],
      ["same", "three"],
    ]);
    expect(lines[1]).toMatchObject({ oldNo: 2 });
    expect(lines[2]).toMatchObject({ newNo: 2 });
    expect(countChanges(lines)).toEqual({ added: 1, removed: 1 });
  });

  test("keeps the common lines of an interleaved edit", () => {
    const lines = diffLines("a\nb\nc\nd", "a\nx\nc\ny\nd");
    expect(lines.filter((line) => line.kind === "same").map((line) => line.text)).toEqual([
      "a",
      "c",
      "d",
    ]);
    expect(countChanges(lines)).toEqual({ added: 2, removed: 1 });
  });

  test("an empty side is all additions or all removals", () => {
    expect(countChanges(diffLines("", "x\ny"))).toEqual({ added: 2, removed: 0 });
    expect(countChanges(diffLines("x\ny", ""))).toEqual({ added: 0, removed: 2 });
    expect(diffLines("", "")).toEqual([]);
  });

  test("reads CRLF and a trailing newline as the same text", () => {
    expect(countChanges(diffLines("a\r\nb\r\n", "a\nb"))).toEqual({ added: 0, removed: 0 });
  });

  test("a pair too large for the table still reports every change", () => {
    const before = Array.from({ length: 2500 }, (_, i) => `old ${i}`).join("\n");
    const after = Array.from({ length: 2500 }, (_, i) => `new ${i}`).join("\n");
    expect(countChanges(diffLines(before, after))).toEqual({ added: 2500, removed: 2500 });
  });
});

describe("foldUnchanged", () => {
  test("folds a long unchanged run to the context beside each change", () => {
    const body = Array.from({ length: 20 }, (_, i) => `line ${i}`);
    const lines = diffLines(
      body.join("\n"),
      [...body.slice(0, 10), "edit", ...body.slice(11)].join("\n"),
    );
    const folded = foldUnchanged(lines, 2);
    expect(folded.filter((row) => row.kind === "skip")).toEqual([
      { kind: "skip", count: 8 },
      { kind: "skip", count: 7 },
    ]);
  });

  test("keeps everything when changes are close together", () => {
    const folded = foldUnchanged(diffLines("a\nb\nc", "a\nB\nc"), 3);
    expect(folded.some((row) => row.kind === "skip")).toBe(false);
  });
});

describe("pairLines", () => {
  test("sets a removal beside the addition that replaces it", () => {
    const rows = pairLines(diffLines("a\nb\nc", "a\nB\nc"));
    expect(rows.map((row) => [row.left?.text, row.right?.text])).toEqual([
      ["a", "a"],
      ["b", "B"],
      ["c", "c"],
    ]);
  });

  test("leaves the other side empty for a pure addition", () => {
    const rows = pairLines(diffLines("a", "a\nb"));
    expect(rows[1]).toMatchObject({ right: { text: "b" } });
    expect(rows[1].left).toBeUndefined();
  });
});

describe("diffFields", () => {
  test("names added, removed, changed and unchanged fields", () => {
    const changes = diffFields(
      { keep: "1", edit: "a", gone: "x" },
      { keep: "1", edit: "b", new: "y" },
    );
    expect(changes).toEqual([
      { field: "keep", kind: "same", before: "1", after: "1" },
      { field: "edit", kind: "changed", before: "a", after: "b" },
      { field: "gone", kind: "removed", before: "x" },
      { field: "new", kind: "added", after: "y" },
    ]);
  });
});
