#!/usr/bin/env bun
// playwright pin gate (#2028).
//
//   bun run check:playwright-pin
//
// `overrides.playwright` and `overrides.playwright-core` force the one copy of
// playwright the storybook test-runner shares with `@playwright/test` (#737).
// An override wins over the dependency range, so a dependabot bump of
// `@playwright/test` that skips the override installs nothing new: the pin
// silently froze playwright at 1.62.0 while the manifest said 1.63.0. This
// fails when the three disagree, so such a bump goes red instead of landing as
// a no-op.
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Manifest = {
  overrides?: Record<string, string>;
  devDependencies?: Record<string, string>;
};

/** One line per mismatch; empty when the three versions agree. */
export function playwrightPinProblems(pkg: Manifest): string[] {
  const entries: [string, string | undefined][] = [
    ['devDependencies["@playwright/test"]', pkg.devDependencies?.["@playwright/test"]],
    ["overrides.playwright", pkg.overrides?.["playwright"]],
    ["overrides.playwright-core", pkg.overrides?.["playwright-core"]],
  ];
  const missing = entries.filter(([, v]) => v === undefined).map(([k]) => `${k} is missing`);
  if (missing.length > 0) return missing;
  if (new Set(entries.map(([, v]) => v)).size === 1) return [];
  return [
    "playwright versions differ, set all three to one exact version:",
    ...entries.map(([k, v]) => `  ${k} = ${v}`),
  ];
}

if (import.meta.main) {
  const pkg = JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8"));
  const problems = playwrightPinProblems(pkg);
  if (problems.length > 0) {
    console.error(problems.join("\n"));
    process.exit(1);
  }
  console.log("playwright pin: @playwright/test, playwright and playwright-core agree");
}
