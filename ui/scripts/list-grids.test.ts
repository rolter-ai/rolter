import { describe, expect, it } from "bun:test";
import { Glob } from "bun";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { PRIMARY_COLUMN_FLOOR, primaryColumn } from "../src/components/screen";

// the identifying column of a list table keeps a floor (#2812).
//
// A list's grid is a string of tracks, shared by its header and its rows. A
// first column written as a bare `1.2fr` is a share of what the other columns
// leave, and a table at its floor leaves little, so the name a row is found by
// was drawn at 70-100px, cut to "vllm-a10…" and "Pl…". `primaryColumn()` is the
// same share with a floor under it. Nothing in the markup says a grid's first
// column is the one that matters, so this reads the screens and fails the next
// list that forgets, the way `check:primitives` fails a bare `<select>`.

/** The first track of a grid string, with `primaryColumn(…)` kept whole. */
export function firstTrack(grid: string): string {
  const call = grid.match(/^\$\{\s*primaryColumn\([^)]*\)\s*\}/);
  if (call) return call[0];
  return grid.trim().split(/\s+/)[0] ?? "";
}

/** a track that takes a share of the free space and nothing more */
const BARE_FR = /^\d*\.?\d+fr$/;

export interface GridDeclaration {
  name: string;
  grid: string;
  line: number;
}

/** every `const …GRID = "…"` or template literal in `source` */
export function gridDeclarations(source: string): GridDeclaration[] {
  const found: GridDeclaration[] = [];
  const re = /\b(?:const|let)\s+(\w*GRID)\s*=\s*(["'`])((?:\\.|(?!\2)[^\\])*)\2/g;
  for (const match of source.matchAll(re)) {
    found.push({
      name: match[1],
      grid: match[3],
      line: source.slice(0, match.index).split("\n").length,
    });
  }
  return found;
}

/** the grids of a list table whose first column is a bare share of the free space */
export function unfloored(source: string): GridDeclaration[] {
  if (!/<ListTable\b/.test(source)) return [];
  return gridDeclarations(source).filter((decl) => BARE_FR.test(firstTrack(decl.grid)));
}

describe("primaryColumn", () => {
  it("takes its share of the free space, never less than the floor", () => {
    expect(primaryColumn(1.2)).toBe(`minmax(${PRIMARY_COLUMN_FLOOR}, 1.2fr)`);
    expect(primaryColumn("1.5fr")).toBe(`minmax(${PRIMARY_COLUMN_FLOOR}, 1.5fr)`);
  });
});

/** `${inner}` as source text, built so it is not mistaken for an unexpanded template */
const interpolation = (inner: string) => "$" + "{" + inner + "}";

describe("the rule", () => {
  it("reads the first track of a plain grid", () => {
    expect(firstTrack("1.2fr 1fr 96px")).toBe("1.2fr");
    expect(firstTrack("150px 1.1fr")).toBe("150px");
  });

  it("keeps a primaryColumn call whole, spaces and all", () => {
    const tight = interpolation("primaryColumn(1.2)");
    expect(firstTrack(`${tight} 1fr 96px`)).toBe(tight);
    const spaced = interpolation(" primaryColumn(1) ");
    expect(firstTrack(`${spaced} 2fr`)).toBe(spaced);
  });

  it("fails a list whose first column is a bare fr", () => {
    const source = `<ListTable>\nconst GRID = "1.2fr 1fr 96px";`;
    expect(unfloored(source)).toEqual([{ name: "GRID", grid: "1.2fr 1fr 96px", line: 2 }]);
  });

  it("passes a floored first column and a fixed one", () => {
    const floored = `<ListTable>\nconst GRID = \`${interpolation("primaryColumn(1.2)")} 1fr\`;`;
    expect(unfloored(floored)).toEqual([]);
    const fixed = '<ListTable>\nconst GRID = "150px 1.1fr 1.3fr";';
    expect(unfloored(fixed)).toEqual([]);
  });

  it("leaves a grid in a file with no list table alone", () => {
    expect(unfloored('const DETAIL_GRID = "1fr 2fr";')).toEqual([]);
  });
});

describe("the screens", () => {
  it("floor the first column of every list table", () => {
    const root = join(import.meta.dir, "..");
    const offenders: string[] = [];
    let seen = 0;
    for (const path of new Glob("src/**/*.tsx").scanSync({ cwd: root })) {
      if (path.endsWith(".stories.tsx")) continue;
      const source = readFileSync(join(root, path), "utf8");
      if (/<ListTable\b/.test(source)) seen += gridDeclarations(source).length;
      for (const decl of unfloored(source)) {
        offenders.push(`${path}:${decl.line} ${decl.name} = "${decl.grid}"`);
      }
    }
    // the scan has to be looking at the lists, or an empty answer means nothing
    expect(seen).toBeGreaterThan(10);
    // a list's first column is the one a row is found by: write it as
    // `${primaryColumn(<weight>)}` (docs/dev-docs/development/list-tables.md)
    expect(offenders).toEqual([]);
  });
});
