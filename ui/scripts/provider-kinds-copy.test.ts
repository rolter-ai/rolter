import { describe, expect, it } from "bun:test";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

import { PROVIDER_KINDS as BUNDLED } from "../src/lib/api";

const ROOT = join(import.meta.dir, "..", "..");
const LOCALES = join(ROOT, "ui", "src", "lib", "i18n", "locales");

/** the `PROVIDER_KINDS` allowlist the control plane validates a write against */
function controlPlaneKinds(): string[] {
  const source = readFileSync(join(ROOT, "crates", "rolter-control", "src", "crud.rs"), "utf8");
  const list = /const PROVIDER_KINDS: \[&str; \d+\] = \[([\s\S]*?)\];/.exec(source)?.[1] ?? "";
  return [...list.matchAll(/"([a-z0-9_]+)"/g)].map((m) => m[1]!);
}

const PROVIDER_KINDS: string[] = [...BUNDLED];

type Kinds = Record<string, { name?: string; description?: string }>;

function catalog(locale: string): Kinds {
  const raw = JSON.parse(readFileSync(join(LOCALES, `${locale}.json`), "utf8"));
  return raw.providerSheet.kinds as Kinds;
}

// the picker names every kind and says in a line what it is (#2811). a kind the
// backend gains without a name here would be listed by its raw id in the
// picker, so the bundled list and the catalogs are held to the backend's
describe("provider kind copy", () => {
  it("lists exactly the kinds the control plane accepts", () => {
    expect(controlPlaneKinds().length).toBeGreaterThan(0);
    expect(PROVIDER_KINDS).toEqual(controlPlaneKinds());
  });

  for (const locale of readdirSync(LOCALES).map((f) => f.replace(/\.json$/, ""))) {
    it(`${locale} names and describes every kind and nothing else`, () => {
      const kinds = catalog(locale);
      expect(Object.keys(kinds).sort()).toEqual([...PROVIDER_KINDS].sort());
      for (const kind of PROVIDER_KINDS) {
        expect(kinds[kind]?.name?.trim(), `${locale} name of ${kind}`).toBeTruthy();
        expect(kinds[kind]?.description?.trim(), `${locale} description of ${kind}`).toBeTruthy();
      }
    });
  }
});
