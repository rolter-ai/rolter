import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

// index.css records a distinctness floor beside the --chart-* tokens: no two of
// the eight series colours closer than CIE76 delta-E 17, in plain vision and
// under protanopia, deuteranopia and tritanopia (Machado 2009, full severity).
// nothing enforced it, so a retune could quietly bring two series back to
// reading as one colour (#2436, #2543). there is no light theme, so the single
// :root block is the only place the tokens are defined
const CSS = readFileSync(fileURLToPath(new URL("../src/index.css", import.meta.url)), "utf8");

type Rgb = [number, number, number];
type Matrix = [Rgb, Rgb, Rgb];

// Machado, Oliveira & Fernandes 2009, severity 1.0, applied to linear sRGB
const DEFICIENCIES: Record<string, Matrix | null> = {
  "plain vision": null,
  protanopia: [
    [0.152286, 1.052583, -0.204868],
    [0.114503, 0.786281, 0.099216],
    [-0.003882, -0.048116, 1.051998],
  ],
  deuteranopia: [
    [0.367322, 0.860646, -0.227968],
    [0.280085, 0.672501, 0.047413],
    [-0.01182, 0.04294, 0.968881],
  ],
  tritanopia: [
    [1.255528, -0.076749, -0.178779],
    [-0.078411, 0.930809, 0.147602],
    [0.004733, 0.691367, 0.3039],
  ],
};

/** every `--name: value` declaration, last one wins */
function declarations(css: string): Map<string, string> {
  const out = new Map<string, string>();
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const m of stripped.matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) out.set(m[1]!, m[2]!.trim());
  return out;
}

function resolve(value: string, vars: Map<string, string>, seen = new Set<string>()): string {
  const ref = /^var\((--[\w-]+)\)$/.exec(value);
  if (!ref) return value;
  const name = ref[1]!;
  if (seen.has(name)) throw new Error(`${name} refers to itself`);
  const next = vars.get(name);
  if (next === undefined) throw new Error(`${name} is not defined in index.css`);
  return resolve(next, vars, new Set(seen).add(name));
}

function parseHex(hex: string): Rgb {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex);
  if (!m) throw new Error(`unsupported colour "${hex}": the chart tokens must resolve to hex`);
  const h = m[1]!.length === 3 ? [...m[1]!].map((c) => c + c).join("") : m[1]!;
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16) / 255) as Rgb;
}

const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);

function toLab(linear: Rgb): Rgb {
  const [r, g, b] = linear;
  // linear sRGB to XYZ, D65
  const x = 0.4124564 * r + 0.3575761 * g + 0.1804375 * b;
  const y = 0.2126729 * r + 0.7151522 * g + 0.072175 * b;
  const z = 0.0193339 * r + 0.119192 * g + 0.9503041 * b;
  const f = (t: number) => (t > 216 / 24389 ? Math.cbrt(t) : (24389 / 27 / 116) * t + 16 / 116);
  const [fx, fy, fz] = [f(x / 0.95047), f(y), f(z / 1.08883)];
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)];
}

function simulate(rgb: Rgb, m: Matrix | null): Rgb {
  const lin = rgb.map(toLinear) as Rgb;
  if (!m) return lin;
  return m.map((row) =>
    Math.min(1, Math.max(0, row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2])),
  ) as Rgb;
}

const deltaE76 = (a: Rgb, b: Rgb) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);

export interface PairDistance {
  vision: string;
  a: string;
  b: string;
  deltaE: number;
}

/** the closest pair of the series colours under each vision type */
export function worstPairs(css: string): PairDistance[] {
  const vars = declarations(css);
  const names = [...vars.keys()].filter((n) => /^--chart-\d+$/.test(n));
  const colours = names.map((n) => parseHex(resolve(vars.get(n)!, vars)));
  return Object.entries(DEFICIENCIES).map(([vision, m]) => {
    const labs = colours.map((c) => toLab(simulate(c, m)));
    let worst: PairDistance = { vision, a: "", b: "", deltaE: Infinity };
    for (let i = 0; i < names.length; i++)
      for (let j = i + 1; j < names.length; j++) {
        const deltaE = deltaE76(labs[i]!, labs[j]!);
        if (deltaE < worst.deltaE) worst = { vision, a: names[i]!, b: names[j]!, deltaE };
      }
    return worst;
  });
}

/** the floor, read from the comment that records it so the two cannot drift */
function recordedFloor(css: string): number {
  const m = /closer\s+than\s+CIE76\s+delta-E\s+(\d+(?:\.\d+)?)/.exec(css);
  if (!m) throw new Error("index.css no longer records the chart distinctness floor");
  return Number(m[1]);
}

describe("chart palette distinctness (#2543)", () => {
  const floor = recordedFloor(CSS);

  it("reads a floor and eight series", () => {
    expect(floor).toBeGreaterThan(0);
    const names = [...declarations(CSS).keys()].filter((n) => /^--chart-\d+$/.test(n));
    expect(names.length).toBe(8);
  });

  it("keeps every pair of --chart-N at or above the recorded CIE76 delta-E floor", () => {
    const failures = [];
    for (const w of worstPairs(CSS)) {
      if (w.deltaE < floor)
        failures.push(
          `${w.a} and ${w.b} are ${w.deltaE.toFixed(1)} apart under ${w.vision}, below the delta-E ${floor} floor`,
        );
    }
    expect(failures).toEqual([]);
  });
});
