import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// #3067: i18n is one instance for the whole browser session, so a story that
// follows a Russian one used to start in Russian and flip to English halfway
// through its play, after the decorator's effect ran. McpCatalog's "Register
// Sheet Is Sectioned" failed four runs in ten on it. The language has to be
// settled in `beforeEach`, which the runner awaits before the story mounts.
describe(".storybook/preview.ts", () => {
  const source = readFileSync(join(import.meta.dir, "..", ".storybook", "preview.ts"), "utf8");

  it("settles the story's locale before it renders", () => {
    const hook = /beforeEach:\s*async\s*\(\{\s*globals\s*\}\)\s*=>\s*\{[^}]*await setLocale\(/.exec(
      source,
    );
    expect(hook).not.toBeNull();
  });
});
