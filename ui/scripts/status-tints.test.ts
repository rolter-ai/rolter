import { describe, expect, it } from "bun:test";
import { Glob } from "bun";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// a colour in a component is a token, never an `rgba(…)` literal (#1989).
//
// `Pill` used to take its tint as a raw `rgba(22,163,74,.14)` through a prop,
// so retuning `--status-success` moved every `Badge` and none of the pills,
// and `check:primitives` could not see it: the colour travelled in
// `style={{}}`, not in a class. A status is a `tone` now, resolved from the
// same tokens as `Badge`; this fails the next literal that brings the second
// system back. A shadow or a scrim is a token in `index.css` too.

const SRC = join(import.meta.dir, "..", "src");

/** the `rgba(` / `rgb(` / `hsla(` literals in `source`, with their line numbers */
export function colourLiterals(source: string): { line: number; text: string }[] {
  const found: { line: number; text: string }[] = [];
  source.split("\n").forEach((text, i) => {
    if (/\b(?:rgba?|hsla?)\(\s*\d/.test(text)) found.push({ line: i + 1, text: text.trim() });
  });
  return found;
}

describe("colourLiterals", () => {
  it("finds a tint passed as a literal", () => {
    expect(colourLiterals(`<Pill tint="rgba(22,163,74,.14)">ok</Pill>`)).toHaveLength(1);
    expect(colourLiterals(`const t = ["x", "rgb(1, 2, 3)"];`)).toHaveLength(1);
  });

  it("lets a token and the shadcn hsl contract through", () => {
    expect(colourLiterals(`<Pill tone="success">ok</Pill>`)).toEqual([]);
    expect(colourLiterals(`color: "hsl(var(--border))"`)).toEqual([]);
  });
});

describe("dashboard sources", () => {
  it("carry no colour literal outside index.css", () => {
    const offenders: string[] = [];
    for (const file of new Glob("**/*.{ts,tsx}").scanSync(SRC)) {
      if (/\.(stories|test)\.tsx?$/.test(file)) continue;
      for (const hit of colourLiterals(readFileSync(join(SRC, file), "utf8"))) {
        offenders.push(`src/${file}:${hit.line}: ${hit.text}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
