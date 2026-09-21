#!/usr/bin/env bun
// hardcoded-literal gate (#871). See src/lib/i18n/literals.ts for why this
// exists, and src/lib/i18n/literals-allowlist.ts for the notation it tolerates.
//
//   bun run check:literals              fail on any literal not in the catalogs
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

import { NOT_COPY } from "../src/lib/i18n/literals-allowlist";
import {
  findLiteralsInTree,
  newViolations,
  staleAllowed,
  unexplainedAllowed,
  type Literal,
} from "../src/lib/i18n/literals";

const ROOT = join(import.meta.dir, "..");

// stories and tests are not shipped copy: a story's job is to render a component
// with concrete sample text, and routing that through the catalogs would make
// the stories test the catalogs instead of the component. `story-harness.tsx`
// is the fixture those stories share — it sits under `pages/` without being a
// screen, and it says so in its own header comment, so it skips too, as does
// `shell-harness.tsx`, which mounts the whole shell for `App.stories.tsx` and
// stubs a wire note in English on purpose (#1546).
// `src/lib` is scanned too: `scope.ts` rendered English straight into the
// shell for months while the gate looked only at components and pages (#1200).
// the i18n machinery itself and the tests are the exceptions
const SCANNED = [
  "src/components/**/*.tsx",
  "src/pages/**/*.tsx",
  "src/lib/**/*.{ts,tsx}",
  "src/App.tsx",
];
const SKIP = /\.(stories|test)\.tsx?$|(^|\/)(story|shell)-harness\.tsx$|^src\/lib\/i18n\//;

// read as one tree, so a table or helper exported by one file and rendered by
// another is followed to where it is written (#1765)
const files: Record<string, string> = {};
for (const pattern of SCANNED) {
  for (const path of new Glob(pattern).scanSync(ROOT)) {
    if (SKIP.test(path)) continue;
    files[path.replace(/\\/g, "/")] = readFileSync(join(ROOT, path), "utf8");
  }
}
const found: Literal[] = findLiteralsInTree(files);

const allowedSize = Object.values(NOT_COPY).reduce((n, v) => n + Object.keys(v).length, 0);
const violations = newViolations(found, NOT_COPY);
const stale = staleAllowed(found, NOT_COPY);
const unexplained = unexplainedAllowed(NOT_COPY);

console.log(`scanned ${SCANNED.join(", ")}`);
console.log(`  ${found.length} hardcoded literal(s), ${allowedSize} allowed as notation`);

if (violations.length) {
  console.error(`\n${violations.length} hardcoded user-facing string(s):`);
  for (const v of violations) {
    console.error(`  ${v.file}:${v.line}  [${v.kind}]  ${v.text}`);
  }
  console.error(
    "\nevery user-facing string goes through the i18n catalogs: add the key to\n" +
      "src/lib/i18n/locales/en.json, translate it in every sibling catalog, and use\n" +
      't("...") here. only notation that is never translated goes in\n' +
      "src/lib/i18n/literals-allowlist.ts, with its reason. See docs/dev-docs/development/i18n.md.",
  );
  process.exit(1);
}

if (unexplained.length) {
  console.error(`\n${unexplained.length} allow-list entry/entries with no reason:`);
  for (const u of unexplained) console.error(`  ${u}`);
  console.error("\nsay why each string is not copy, or translate it instead.");
  process.exit(1);
}

if (stale.length) {
  console.error(`\n${stale.length} allow-list entry/entries no longer in the source:`);
  for (const s of stale) console.error(`  ${s}`);
  console.error(
    "\nthe string was translated, reworded or deleted. remove the entry from\n" +
      "src/lib/i18n/literals-allowlist.ts so the same literal cannot come back unnoticed.",
  );
  process.exit(1);
}

console.log("\nno hardcoded user-facing strings");
