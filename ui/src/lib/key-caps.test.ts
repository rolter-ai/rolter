import { describe, expect, test } from "bun:test";

import {
  EMPTY_CAPS,
  capProblems,
  capsToInput,
  capsValid,
  limitProblem,
  rateCapProblem,
} from "./key-caps";

describe("limitProblem", () => {
  test("accepts what numeric(12,4) holds, zero included", () => {
    for (const ok of ["0", "0.0001", "500", " 750.5 ", "99999999.9999", "99999999.99994"]) {
      expect(limitProblem(ok)).toBeNull();
    }
  });

  test("refuses what the control plane refuses", () => {
    expect(limitProblem("")).toBe("blank");
    for (const bad of ["-1", "NaN", "inf", "1e9", "lots", "1,5"]) {
      expect(limitProblem(bad)).toBe("invalid");
    }
    for (const big of ["99999999.99995", "100000000"]) expect(limitProblem(big)).toBe("range");
  });
});

describe("rateCapProblem", () => {
  test("an empty box is no cap on that axis", () => {
    expect(rateCapProblem("")).toBeNull();
    expect(rateCapProblem("60")).toBeNull();
  });

  test("whole numbers from 1 only", () => {
    expect(rateCapProblem("0")).toBe("range");
    expect(rateCapProblem("2147483648")).toBe("range");
    expect(rateCapProblem("1.5")).toBe("invalid");
    expect(rateCapProblem("-3")).toBe("invalid");
  });
});

describe("capsToInput", () => {
  test("no cap by default", () => {
    expect(capsToInput(EMPTY_CAPS)).toEqual({});
    expect(capsValid(EMPTY_CAPS)).toBe(true);
  });

  test("sends only what is switched on, with the window spelled out", () => {
    const draft = { ...EMPTY_CAPS, budgetOn: true, limit: " 25.5 ", period: "daily" as const };
    expect(capsToInput(draft)).toEqual({ budget: { limit_usd: "25.5", period: "daily" } });
  });

  test("a rate cap carries only the boxes that are filled", () => {
    const draft = { ...EMPTY_CAPS, rateOn: true, rpm: "120" };
    expect(capsToInput(draft)).toEqual({ rate_limit: { rpm: 120 } });
    expect(capsValid(draft)).toBe(true);
  });

  test("a switched-on cap with nothing in it is not valid", () => {
    expect(capsValid({ ...EMPTY_CAPS, budgetOn: true })).toBe(false);
    const empty = { ...EMPTY_CAPS, rateOn: true };
    expect(capProblems(empty).rateEmpty).toBe(true);
    expect(capsValid(empty)).toBe(false);
  });
});
