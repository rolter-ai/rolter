import { describe, expect, test } from "bun:test";
import { playwrightPinProblems } from "./check-playwright-pin";

const pkg = (test: string, pw: string, core: string) => ({
  devDependencies: { "@playwright/test": test },
  overrides: { playwright: pw, "playwright-core": core },
});

describe("playwrightPinProblems", () => {
  test("passes when all three agree", () => {
    expect(playwrightPinProblems(pkg("1.63.0", "1.63.0", "1.63.0"))).toEqual([]);
  });
  test("fails when an override lags the test package", () => {
    expect(playwrightPinProblems(pkg("1.63.0", "1.62.0", "1.63.0")).join("\n")).toContain(
      "overrides.playwright = 1.62.0",
    );
  });
  test("fails when playwright-core differs", () => {
    expect(playwrightPinProblems(pkg("1.63.0", "1.63.0", "1.62.0"))).not.toEqual([]);
  });
  test("fails when an entry is missing", () => {
    expect(playwrightPinProblems({ devDependencies: {}, overrides: {} })).toHaveLength(3);
  });
});
