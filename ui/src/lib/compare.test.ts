import { describe, expect, it } from "bun:test";

import { compare, toneOfChange } from "@/lib/compare";

describe("compare (#2010)", () => {
  it("states the change as a share of the earlier value", () => {
    expect(compare(120, 100)).toEqual({ trend: "up", ratio: 0.2 });
    expect(compare(75, 100)).toEqual({ trend: "down", ratio: -0.25 });
  });

  it("reads a change inside the dead band as flat", () => {
    expect(compare(100.5, 100)?.trend).toBe("flat");
    expect(compare(99.2, 100)?.trend).toBe("flat");
    expect(compare(101, 100)?.trend).toBe("up");
  });

  it("has nothing to say without an earlier value, or from zero", () => {
    expect(compare(10, undefined)).toBeNull();
    expect(compare(undefined, 10)).toBeNull();
    expect(compare(10, 0)).toBeNull();
    expect(compare(10, Number.NaN)).toBeNull();
  });
});

describe("toneOfChange", () => {
  it("colours a move by what it means for the figure", () => {
    expect(toneOfChange("up", "good")).toBe("good");
    expect(toneOfChange("down", "good")).toBe("bad");
    expect(toneOfChange("up", "bad")).toBe("bad");
    expect(toneOfChange("down", "bad")).toBe("good");
    expect(toneOfChange("up", "neutral")).toBe("neutral");
    expect(toneOfChange("flat", "bad")).toBe("neutral");
  });
});
