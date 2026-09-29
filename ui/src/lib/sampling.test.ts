import { describe, expect, it } from "bun:test";

import { sampleShare } from "@/lib/sampling";

const share = (rate: number) => {
  const s = sampleShare(rate);
  return s && `${s.numerator} in ${s.denominator}`;
};

describe("sampleShare", () => {
  it("has nothing to say when every request or none is logged", () => {
    expect(sampleShare(1)).toBeNull();
    expect(sampleShare(0)).toBeNull();
    expect(sampleShare(1.5)).toBeNull();
    expect(sampleShare(-0.1)).toBeNull();
    expect(sampleShare(Number.NaN)).toBeNull();
  });

  it("reads the round rates as the fraction an operator would say", () => {
    expect(share(0.25)).toBe("1 in 4");
    expect(share(0.5)).toBe("1 in 2");
    expect(share(0.75)).toBe("3 in 4");
    expect(share(0.1)).toBe("1 in 10");
    expect(share(0.9)).toBe("9 in 10");
    expect(share(0.2)).toBe("1 in 5");
    expect(share(0.125)).toBe("1 in 8");
  });

  it("picks the nearest small fraction for a rate that is not one", () => {
    expect(share(0.33)).toBe("1 in 3");
    expect(share(0.15)).toBe("1 in 7");
    expect(share(0.6)).toBe("3 in 5");
  });

  it("counts one in however many below 10 percent", () => {
    expect(share(0.05)).toBe("1 in 20");
    expect(share(0.01)).toBe("1 in 100");
    expect(share(0.001)).toBe("1 in 1000");
    expect(share(0.08)).toBe("1 in 13");
  });

  it("counts all but one in however many above 90 percent", () => {
    expect(share(0.95)).toBe("19 in 20");
    expect(share(0.99)).toBe("99 in 100");
    expect(share(0.999)).toBe("999 in 1000");
  });
});
