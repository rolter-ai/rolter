import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

// #2419: the dashboard painted selection in the UA colour. The pair is a token
// and must clear AA on every surface a selection can land on.

const css = readFileSync(fileURLToPath(new URL("../src/index.css", import.meta.url)), "utf8");

function token(name: string): string {
  const match = css.match(new RegExp(`--${name}:\\s*([^;]+);`));
  if (!match) throw new Error(`--${name} not found`);
  const value = match[1].trim();
  const ref = value.match(/^var\(--([\w-]+)\)$/);
  return ref ? token(ref[1]) : value;
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5]
    .map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
    .map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

describe("text selection", () => {
  it("has a ::selection rule built from the selection tokens", () => {
    expect(css).toMatch(/::selection\s*{[^}]*var\(--selection-bg\)[^}]*var\(--selection-text\)/);
  });

  it("clears 4.5:1 for the text; the pair is opaque so surfaces do not matter", () => {
    expect(ratio(token("selection-text"), token("selection-bg"))).toBeGreaterThanOrEqual(4.5);
  });
});
