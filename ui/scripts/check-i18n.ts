#!/usr/bin/env bun
// catalog parity gate (#489). `en` is the base: every other locale must define
// exactly its key set — no missing keys (which silently fall back to english
// mid-sentence) and no orphans (copy that outlived the screen that used it).
// plural forms and interpolation placeholders are checked too; the rules live
// in src/lib/i18n/parity.ts so `bun test` enforces the same standard.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

import { compare, flatten, isClean, type Catalog } from "../src/lib/i18n/parity";

const LOCALES_DIR = join(import.meta.dir, "..", "src", "lib", "i18n", "locales");
const BASE = "en";

function load(locale: string): Map<string, string> {
  const raw = readFileSync(join(LOCALES_DIR, `${locale}.json`), "utf8");
  return flatten(JSON.parse(raw) as Catalog);
}

const locales = readdirSync(LOCALES_DIR)
  .filter((f) => f.endsWith(".json"))
  .map((f) => f.replace(/\.json$/, ""))
  .filter((l) => l !== BASE)
  .sort();

const base = load(BASE);
const problems: string[] = [];

console.log(`base catalog ${BASE}: ${base.size} keys`);

for (const locale of locales) {
  const catalog = load(locale);
  const d = compare(base, catalog, locale);

  for (const key of d.missing) problems.push(`${locale}: missing key    ${key}`);
  for (const key of d.orphaned) problems.push(`${locale}: orphaned key   ${key}`);
  for (const key of d.empty) problems.push(`${locale}: empty value    ${key}`);
  for (const p of d.plural) problems.push(`${locale}: plural forms   ${p}`);
  for (const p of d.placeholder) problems.push(`${locale}: placeholder    ${p}`);

  console.log(
    `  ${locale}: ${catalog.size} keys — ${isClean(d) ? "ok" : "FAIL"}` +
      (isClean(d)
        ? ""
        : ` (${d.missing.length} missing, ${d.orphaned.length} orphaned, ` +
          `${d.empty.length} empty, ${d.plural.length} plural, ${d.placeholder.length} placeholder)`),
  );
}

if (problems.length) {
  console.error(`\n${problems.length} catalog problem(s):`);
  for (const p of problems) console.error(`  ${p}`);
  console.error(
    "\nadd the missing keys to the locale catalog, or remove them from " +
      "src/lib/i18n/locales/en.json if the copy is gone for good.",
  );
  process.exit(1);
}

console.log("\nall catalogs match the base key set");
