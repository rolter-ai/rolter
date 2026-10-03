import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

// date-times render through `useFormat()` so they carry the chart time zone and
// an unambiguous date (#2219). a bare `toLocale*String` follows the browser's
// locale and zone instead, so none may appear outside the formatter itself
const ROOT = join(import.meta.dir, "../../..");
const RAW = /\.toLocale(Date|Time)?String\(|new Intl\.DateTimeFormat\(/;
const ALLOWED = new Set([
  "src/lib/i18n/format.ts",
  // reads the browser's own zone name for the preference's placeholder
  "src/pages/Preferences.tsx",
]);

test("no screen formats a date with a raw toLocale*String", () => {
  const offenders: string[] = [];
  for (const file of new Glob("src/**/*.{ts,tsx}").scanSync({ cwd: ROOT })) {
    if (ALLOWED.has(file) || /\.(test|stories)\.tsx?$/.test(file)) continue;
    if (RAW.test(readFileSync(join(ROOT, file), "utf8"))) offenders.push(file);
  }
  expect(offenders).toEqual([]);
});
