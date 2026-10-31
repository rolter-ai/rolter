import { describe, expect, it } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { EXCLUDED_REASONS } from "../src/lib/savings-reasons";

// the control plane's list of the reasons the savings advisor leaves a request
// out under (#3423): written by a rolter-control unit test from
// `EXCLUSION_REASONS` in src/savings.rs, which another test holds to what
// `reason_expr` and `analyse` can emit. `ui/` builds on its own, but this test
// is a development check, so it reads the artifact where it lives
const UI = join(import.meta.dir, "..");
const ARTIFACT = join(UI, "..", "crates", "rolter-control", "savings-reasons.json");
const LOCALES = join(UI, "src", "lib", "i18n", "locales");

const published = (JSON.parse(readFileSync(ARTIFACT, "utf8")) as { reasons: string[] }).reasons;

type Catalog = { pages: { savings: { reasons: Record<string, unknown> } } };
const catalogs = readdirSync(LOCALES)
  .filter((file) => file.endsWith(".json"))
  .map((file) => ({
    locale: file.replace(/\.json$/, ""),
    reasons: (JSON.parse(readFileSync(join(LOCALES, file), "utf8")) as Catalog).pages.savings
      .reasons,
  }));

describe("the savings advisor's exclusion reasons", () => {
  it("are published by the control plane", () => {
    expect(published.length).toBeGreaterThan(0);
    expect(new Set(published).size).toBe(published.length);
  });

  it("are every reason the screen lists, and no other", () => {
    // a reason the server added fails here, not as raw snake_case in a table
    expect(
      [...(EXCLUDED_REASONS as readonly string[])].sort(),
      "ui/src/lib/savings-reasons.ts differs from crates/rolter-control/savings-reasons.json",
    ).toEqual([...published].sort());
  });

  it("are worded in every catalog", () => {
    expect(catalogs.length).toBeGreaterThan(1);
    for (const { locale, reasons } of catalogs) {
      const missing = published.filter((reason) => {
        const text = reasons[reason];
        return typeof text !== "string" || text.trim() === "";
      });
      expect(missing, `${locale}: pages.savings.reasons lacks`).toEqual([]);
    }
  });

  it("are the only reasons a catalog words", () => {
    for (const { locale, reasons } of catalogs) {
      const extra = Object.keys(reasons).filter((reason) => !published.includes(reason));
      expect(extra, `${locale}: pages.savings.reasons has a reason the server never emits`).toEqual(
        [],
      );
    }
  });
});
