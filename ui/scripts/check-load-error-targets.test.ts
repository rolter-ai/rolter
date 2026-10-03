import { describe, it, expect } from "bun:test";

import { checkAll, checkSource } from "./check-load-error-targets";

const alert = (target: string) =>
  `<LoadError error={e} resource="x" onRetry={r} target=${target} />`;
const empty = (target: string) => `<EmptyState title="t" uxTarget=${target} />`;

describe("the pairing rule", () => {
  it("accepts a LoadError named like its EmptyState", () => {
    const { violations } = checkSource([empty('"keys"'), alert('"keys"')].join("\n"), "a.tsx");
    expect(violations).toEqual([]);
  });

  it("flags a target that matches none of the file's uxTargets", () => {
    const { violations } = checkSource(
      ["const a = 1;", empty('"keys"'), alert('"key-list"')].join("\n"),
      "a.tsx",
    );
    expect(violations).toEqual([{ file: "a.tsx", line: 3, target: "key-list" }]);
  });

  it("is satisfied by any one of several uxTargets", () => {
    const source = [empty('"a"'), empty('"b"'), alert('"b"')].join("\n");
    expect(checkSource(source, "a.tsx").violations).toEqual([]);
  });

  it("reads every branch of a ternary on either side", () => {
    const source = [
      empty('{n === 0 ? "collector" : "collector-all-off"}'),
      alert('{n === 0 ? "collector-all-off" : "other"}'),
    ].join("\n");
    expect(checkSource(source, "a.tsx").violations).toEqual([]);
  });

  it("reads a multi-line element", () => {
    const source = [
      empty('"keys"'),
      "<LoadError",
      "  error={e}",
      "  onRetry={() => void q.refetch()}",
      '  target="nope"',
      "/>",
    ].join("\n");
    expect(checkSource(source, "a.tsx").violations).toHaveLength(1);
  });

  it("ignores a forwarded target it cannot read", () => {
    const { violations } = checkSource([empty('"keys"'), alert("{target}")].join("\n"), "a.tsx");
    expect(violations).toEqual([]);
  });

  it("ignores a file with no EmptyState — there is no list to pair with", () => {
    expect(checkSource(alert('"profile"'), "a.tsx").violations).toEqual([]);
  });

  it("ignores a uxTarget that is only in a comment", () => {
    const source = ['// <EmptyState uxTarget="keys" />', empty('"other"'), alert('"keys"')];
    expect(checkSource(source.join("\n"), "a.tsx").violations).toHaveLength(1);
  });
});

describe("the waiver", () => {
  it("honours load-error-allow with its reason, and reports it", () => {
    const source = [
      empty('"keys"'),
      "{/* load-error-allow: a figure beside the list, no empty state of its own */}",
      alert('"usage"'),
    ].join("\n");
    const { violations, waivers } = checkSource(source, "a.tsx");
    expect(violations).toEqual([]);
    expect(waivers).toEqual([
      {
        file: "a.tsx",
        line: 3,
        target: "usage",
        reason: "a figure beside the list, no empty state of its own",
      },
    ]);
  });

  it("reads the marker anywhere in the comment block above", () => {
    const source = [
      empty('"keys"'),
      "// load-error-allow: a figure",
      "// that wraps to a second line",
      alert('"usage"'),
    ].join("\n");
    expect(checkSource(source, "a.tsx").violations).toEqual([]);
  });

  it("does not carry a waiver across code", () => {
    const source = [
      empty('"keys"'),
      "// load-error-allow: stale",
      "const x = 1;",
      alert('"usage"'),
    ].join("\n");
    expect(checkSource(source, "a.tsx").violations).toHaveLength(1);
  });
});

describe("the tree", () => {
  it("passes", () => {
    expect(checkAll().violations).toEqual([]);
  });
});
