import { describe, expect, it } from "bun:test";

import { parseSamplingPercent, sampleShare, samplingPercentText } from "@/lib/sampling";

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

describe("parseSamplingPercent", () => {
  it("keeps 0 as a rate of 0 instead of falling back to 100 percent", () => {
    expect(parseSamplingPercent("0")).toEqual({ ok: true, rate: 0 });
    expect(parseSamplingPercent("0.0")).toEqual({ ok: true, rate: 0 });
    expect(parseSamplingPercent(" 0 ")).toEqual({ ok: true, rate: 0 });
    // the sign of a zero is not a rate
    expect(Object.is((parseSamplingPercent("-0") as { rate: number }).rate, 0)).toBe(true);
  });

  it("reads a percentage as the rate the control plane stores", () => {
    expect(parseSamplingPercent("100")).toEqual({ ok: true, rate: 1 });
    expect(parseSamplingPercent("25")).toEqual({ ok: true, rate: 0.25 });
    expect(parseSamplingPercent("10")).toEqual({ ok: true, rate: 0.1 });
    expect(parseSamplingPercent("0.5")).toEqual({ ok: true, rate: 0.005 });
    expect(parseSamplingPercent(".5")).toEqual({ ok: true, rate: 0.005 });
    expect(parseSamplingPercent("1e1")).toEqual({ ok: true, rate: 0.1 });
  });

  it("refuses a blank field and text rather than reading them as a rate", () => {
    for (const typed of [
      "",
      "   ",
      "abc",
      "10%",
      "1,5",
      "--1",
      "0x10",
      "Infinity",
      "NaN",
      "1e999",
    ]) {
      expect(parseSamplingPercent(typed)).toEqual({ ok: false, problem: "invalid" });
    }
  });

  it("refuses a number outside 0 to 100 rather than clamping it", () => {
    for (const typed of ["150", "100.01", "-1", "-0.5", "1e3"]) {
      expect(parseSamplingPercent(typed)).toEqual({ ok: false, problem: "range" });
    }
  });
});

describe("samplingPercentText", () => {
  it("reads a stored rate as the percentage an operator would type", () => {
    expect(samplingPercentText(1)).toBe("100");
    expect(samplingPercentText(0)).toBe("0");
    expect(samplingPercentText(0.25)).toBe("25");
    expect(samplingPercentText(0.004)).toBe("0.4");
  });

  it("drops the floating-point tail a product leaves behind", () => {
    // 0.07 * 100 is 7.000000000000001
    expect(samplingPercentText(0.07)).toBe("7");
    expect(samplingPercentText(0.29)).toBe("29");
    expect(samplingPercentText(0.57)).toBe("57");
  });

  it("opens on text the percentage field accepts", () => {
    for (const rate of [0, 1e-7, 0.004, 0.07, 0.5, 0.123456789012345, 1]) {
      expect(parseSamplingPercent(samplingPercentText(rate)).ok).toBe(true);
    }
  });

  it("does not promise to round-trip a rate with more digits than it shows", () => {
    // which is why an untouched edit sends the stored rate rather than this text
    const text = samplingPercentText(0.123456789012345);
    expect(parseSamplingPercent(text)).not.toEqual({ ok: true, rate: 0.123456789012345 });
  });
});
