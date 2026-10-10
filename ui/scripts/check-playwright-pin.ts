#!/usr/bin/env bun
// one-playwright gate (#2028, #2907).
//
//   bun run check:playwright-pin
//
// The e2e journeys (`@playwright/test`) and the story tests (vitest's
// `@vitest/browser-playwright`, which imports `playwright`) both launch a
// browser, and each launches the chromium revision *its* playwright wants. A
// second copy of playwright in the lockfile means a second revision, one
// `playwright install` never downloaded, and the run that uses it fails at
// launch (#737). That used to be two `overrides` that pinned the story runner's
// private copy to `@playwright/test`'s version; the runner's tree is gone
// (#2907), so the invariant is checked where it is true: in the lockfile.
//
// This fails when `bun.lock` resolves `playwright` or `playwright-core` to
// anything but the version `devDependencies["@playwright/test"]` names, so a
// bump of one that leaves the other behind goes red instead of landing as two
// browser revisions.
import { readFileSync } from "node:fs";
import { join } from "node:path";

type Manifest = { devDependencies?: Record<string, string> };

/**
 * Every version the lockfile resolves `playwright` and `playwright-core` to.
 *
 * `bun.lock` is JSONC, one line per package with the resolved `name@version` as
 * the first element of the entry; a copy nested under another package is a key
 * of its own (`"@vitest/browser-playwright/playwright"`) and shows up the same
 * way, which is the case this exists to catch.
 */
export function lockedPlaywrightVersions(lock: string): { name: string; version: string }[] {
  const found: { name: string; version: string }[] = [];
  for (const match of lock.matchAll(/\["(playwright|playwright-core)@([^"]+)"/g)) {
    found.push({ name: match[1], version: match[2] });
  }
  return found;
}

/** One line per mismatch; empty when one playwright serves both runners. */
export function playwrightPinProblems(pkg: Manifest, lock: string): string[] {
  const wanted = pkg.devDependencies?.["@playwright/test"];
  if (wanted === undefined) return ['devDependencies["@playwright/test"] is missing'];
  const locked = lockedPlaywrightVersions(lock);
  const problems: string[] = [];
  for (const name of ["playwright", "playwright-core"]) {
    const versions = locked.filter((entry) => entry.name === name).map((entry) => entry.version);
    if (versions.length === 0) problems.push(`bun.lock resolves no ${name}`);
    for (const version of versions) {
      if (version !== wanted) problems.push(`bun.lock resolves ${name}@${version}, not ${wanted}`);
    }
  }
  if (problems.length > 0) {
    problems.push(
      `@playwright/test is ${wanted}: bump it and whatever else asks for another playwright together`,
    );
  }
  return problems;
}

if (import.meta.main) {
  const root = join(import.meta.dir, "..");
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as Manifest;
  const problems = playwrightPinProblems(pkg, readFileSync(join(root, "bun.lock"), "utf8"));
  if (problems.length > 0) {
    console.error(problems.join("\n"));
    process.exit(1);
  }
  console.log("playwright pin: bun.lock holds one playwright, the version @playwright/test names");
}
