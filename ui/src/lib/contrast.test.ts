import { expect, test } from "bun:test";

import { contrastRatio } from "./contrast";

test("black on white is 21:1", () => {
  expect(contrastRatio("rgb(0, 0, 0)", "rgb(255, 255, 255)")).toBeCloseTo(21, 5);
});

test("the off switch track (--zinc-500) clears 3:1 on all four surfaces", () => {
  const track = "rgb(113, 113, 122)";
  for (const surface of ["#18181b", "#111113", "#1f1f23", "#27272a"]) {
    const n = parseInt(surface.slice(1), 16);
    const bg = `rgb(${n >> 16}, ${(n >> 8) & 255}, ${n & 255})`;
    expect(contrastRatio(track, bg)).toBeGreaterThanOrEqual(3);
  }
});
