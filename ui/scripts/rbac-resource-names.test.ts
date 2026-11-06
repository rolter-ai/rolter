import { describe, expect, it } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

// the role matrix names each resource in the dashboard's language, with its id
// under the name (#3664). The ids are the dashboard's copy of the control
// plane's capability list, so a resource added there fails here until every
// catalog names it, rather than showing up as bare snake_case in a Russian table
const UI = join(import.meta.dir, "..");
const LOCALES = join(UI, "src", "lib", "i18n", "locales");
const capabilities = JSON.parse(
  readFileSync(join(UI, "src", "lib", "rbac-capabilities.json"), "utf8"),
) as { resources: { resource: string }[] };
const ids = capabilities.resources.map((r) => r.resource);

type Catalog = { pages: { rbac: { resources: Record<string, unknown> } } };
const catalogs = readdirSync(LOCALES)
  .filter((file) => file.endsWith(".json"))
  .map((file) => ({
    locale: file.replace(/\.json$/, ""),
    names: (JSON.parse(readFileSync(join(LOCALES, file), "utf8")) as Catalog).pages.rbac.resources,
  }));

describe("the role matrix's resource names", () => {
  it("cover every resource the control plane guards, in every catalog", () => {
    expect(ids.length).toBeGreaterThan(0);
    expect(catalogs.length).toBeGreaterThan(1);
    for (const { locale, names } of catalogs) {
      const missing = ids.filter((id) => {
        const name = names?.[id];
        return typeof name !== "string" || name.trim() === "";
      });
      expect(missing, `${locale}: pages.rbac.resources lacks`).toEqual([]);
    }
  });

  it("name no resource the control plane does not guard", () => {
    for (const { locale, names } of catalogs) {
      const extra = Object.keys(names ?? {}).filter((id) => !ids.includes(id));
      expect(extra, `${locale}: pages.rbac.resources names an unknown resource`).toEqual([]);
    }
  });

  it("are names, not the id again", () => {
    for (const { locale, names } of catalogs) {
      const echoed = ids.filter((id) => names?.[id] === id);
      expect(echoed, `${locale}: a resource is named by its own id`).toEqual([]);
    }
  });
});
