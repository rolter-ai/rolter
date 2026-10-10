import { describe, expect, test } from "bun:test";
import { lockedPlaywrightVersions, playwrightPinProblems } from "./check-playwright-pin";

const pkg = { devDependencies: { "@playwright/test": "1.64.0" } };

/** A `bun.lock` fragment in its real shape: one line per package, resolved name first. */
const lock = (...entries: [key: string, spec: string][]) =>
  entries.map(([key, spec]) => `    "${key}": ["${spec}", "", {}, "sha512-x"],`).join("\n");

describe("lockedPlaywrightVersions", () => {
  test("reads the resolved name and version out of each entry", () => {
    expect(
      lockedPlaywrightVersions(
        lock(["playwright", "playwright@1.64.0"], ["playwright-core", "playwright-core@1.64.0"]),
      ),
    ).toEqual([
      { name: "playwright", version: "1.64.0" },
      { name: "playwright-core", version: "1.64.0" },
    ]);
  });

  test("reads a copy nested under another package, which has a key of its own", () => {
    expect(
      lockedPlaywrightVersions(
        lock(["@vitest/browser-playwright/playwright", "playwright@1.65.0"]),
      ),
    ).toEqual([{ name: "playwright", version: "1.65.0" }]);
  });

  test("ignores a package that merely mentions playwright", () => {
    expect(
      lockedPlaywrightVersions(
        lock(
          ["@playwright/test", "@playwright/test@1.64.0"],
          ["@vitest/browser-playwright", "@vitest/browser-playwright@5.0.3"],
        ),
      ),
    ).toEqual([]);
  });
});

describe("playwrightPinProblems", () => {
  const one = lock(
    ["playwright", "playwright@1.64.0"],
    ["playwright-core", "playwright-core@1.64.0"],
  );

  test("passes when one playwright serves both runners", () => {
    expect(playwrightPinProblems(pkg, one)).toEqual([]);
  });

  test("fails when the lockfile lags a bump of @playwright/test", () => {
    const problems = playwrightPinProblems(
      { devDependencies: { "@playwright/test": "1.65.0" } },
      one,
    ).join("\n");
    expect(problems).toContain("playwright@1.64.0, not 1.65.0");
    expect(problems).toContain("playwright-core@1.64.0, not 1.65.0");
  });

  test("fails on a second copy of playwright beside the right one", () => {
    const two = `${one}\n${lock(["@vitest/browser-playwright/playwright", "playwright@1.65.0"])}`;
    expect(playwrightPinProblems(pkg, two).join("\n")).toContain("playwright@1.65.0, not 1.64.0");
  });

  test("fails when the lockfile has no playwright at all", () => {
    expect(playwrightPinProblems(pkg, "").join("\n")).toContain("bun.lock resolves no playwright");
  });

  test("fails when @playwright/test is missing", () => {
    expect(playwrightPinProblems({ devDependencies: {} }, one)).toEqual([
      'devDependencies["@playwright/test"] is missing',
    ]);
  });
});
