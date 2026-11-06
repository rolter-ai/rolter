import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { BASE_EXAMPLES } from "../src/lib/provider-base-examples";

// the provider sheet's per-kind base placeholders are the example config's own
// (#3666), so a base corrected there is corrected in the sheet too. Commented
// rows count: most kinds are documented there as a commented-out provider
const EXAMPLE = readFileSync(join(import.meta.dir, "..", "..", "rolter.example.toml"), "utf8");

function documentedBases(toml: string): Record<string, string> {
  const bases: Record<string, string> = {};
  const pair = /^#? ?kind = "([a-z0-9_]+)"[^\n]*\n#? ?api_base = "([^"]+)"/gm;
  for (const [, kind, base] of toml.matchAll(pair)) {
    if (!(kind! in bases)) bases[kind!] = base!;
  }
  return bases;
}

describe("the provider sheet's base examples", () => {
  it("are the first base rolter.example.toml gives each kind", () => {
    const documented = documentedBases(EXAMPLE);
    expect(Object.keys(documented).length).toBeGreaterThan(10);
    expect(
      BASE_EXAMPLES,
      "ui/src/lib/provider-base-examples.ts differs from rolter.example.toml",
    ).toEqual(documented);
  });
});
