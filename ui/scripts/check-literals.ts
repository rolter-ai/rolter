#!/usr/bin/env bun
// hardcoded-literal gate (#871). See src/lib/i18n/literals.ts for why this
// exists and why it carries a baseline.
//
//   bun run check:literals              fail on any literal not in the baseline
//   bun run check:literals --update     re-record the baseline (only to shrink it)
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

import {
  findLiterals,
  newViolations,
  staleBaseline,
  toBaseline,
  type Baseline,
  type Literal,
} from "../src/lib/i18n/literals";

const ROOT = join(import.meta.dir, "..");
const BASELINE_PATH = join(ROOT, "src", "lib", "i18n", "literals-baseline.json");

// stories and tests are not shipped copy: a story's job is to render a component
// with concrete sample text, and routing that through the catalogs would make
// the stories test the catalogs instead of the component. `story-harness.tsx`
// is the fixture those stories share — it sits under `pages/` without being a
// screen, and it says so in its own header comment, so it skips too.
// `src/lib` is scanned too: `scope.ts` rendered English straight into the
// shell for months while the gate looked only at components and pages (#1200).
// the i18n machinery itself and the tests are the exceptions
const SCANNED = ["src/components/**/*.tsx", "src/pages/**/*.tsx", "src/lib/**/*.{ts,tsx}", "src/App.tsx"];
const SKIP = /\.(stories|test)\.tsx?$|(^|\/)story-harness\.tsx$|^src\/lib\/i18n\//;

const found: Literal[] = [];
for (const pattern of SCANNED) {
  for (const path of new Glob(pattern).scanSync(ROOT)) {
    if (SKIP.test(path)) continue;
    const rel = path.replaceAll("\\", "/");
    found.push(...findLiterals(readFileSync(join(ROOT, path), "utf8"), rel));
  }
}
found.sort((a, b) => a.file.localeCompare(b.file) || a.line - b.line);

if (process.argv.includes("--update")) {
  writeFileSync(BASELINE_PATH, `${JSON.stringify(toBaseline(found), null, 2)}\n`);
  console.log(`recorded ${found.length} literal(s) to ${BASELINE_PATH}`);
  process.exit(0);
}

const baseline: Baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8"));
const baselineSize = Object.values(baseline).reduce((n, v) => n + v.length, 0);
const violations = newViolations(found, baseline);
const stale = staleBaseline(found, baseline);

console.log(`scanned ${SCANNED.join(", ")}`);
console.log(`  ${found.length} hardcoded literal(s), ${baselineSize} recorded as pre-existing`);

if (violations.length) {
  console.error(`\n${violations.length} new hardcoded user-facing string(s):`);
  for (const v of violations) {
    console.error(`  ${v.file}:${v.line}  [${v.kind}]  ${v.text}`);
  }
  console.error(
    "\nevery user-facing string goes through the i18n catalogs: add the key to\n" +
      "src/lib/i18n/locales/en.json, translate it in every sibling catalog, and use\n" +
      't("...") here. See docs/dev-docs/development/i18n.md.',
  );
  process.exit(1);
}

if (stale.length) {
  console.error(`\n${stale.length} baseline entry/entries no longer in the source:`);
  for (const s of stale) console.error(`  ${s}`);
  console.error(
    "\nthese were translated or deleted — good. run `bun run check:literals --update`\n" +
      "to drop them, so the same literal cannot come back unnoticed.",
  );
  process.exit(1);
}

console.log("\nno new hardcoded user-facing strings");
