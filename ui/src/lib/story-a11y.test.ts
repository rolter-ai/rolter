import { describe, it, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

import { PAGE_A11Y_RULE_IDS, PAGE_A11Y_STORY_ID, withPageA11y } from "./story-a11y";

const UI_ROOT = join(import.meta.dir, "..", "..");

/** Storybook's story-id derivation, enough of it for a `Group/Screen` title. */
function titleToId(title: string): string {
  return title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

/** Story files that spread `withPageA11y`, with the meta title they declare. */
function pageStoryTitles(): string[] {
  const titles: string[] = [];
  for (const path of new Glob("src/**/*.stories.tsx").scanSync(UI_ROOT)) {
    const source = readFileSync(join(UI_ROOT, path), "utf8");
    if (!source.includes("...withPageA11y")) continue;
    const title = /title:\s*"([^"]+)"/.exec(source)?.[1];
    if (title) titles.push(title);
  }
  return titles.sort();
}

describe("withPageA11y", () => {
  it("claims exactly the rules it enables", () => {
    // `expectRules` is the marker the test runner verifies arrived (#1373). If
    // it ever listed a rule the fragment does not actually enable, every page
    // story would fail; if it listed fewer, a dropped rule would pass.
    expect(withPageA11y.a11y.expectRules).toEqual(PAGE_A11Y_RULE_IDS);
    expect(Object.keys(withPageA11y.a11y.rules).sort()).toEqual([...PAGE_A11Y_RULE_IDS].sort());
    for (const rule of PAGE_A11Y_RULE_IDS) {
      expect(withPageA11y.a11y.rules[rule]).toEqual({ enabled: true });
    }
  });

  it("carries no `parameters` key of its own", () => {
    // it is a fragment *of* `parameters`; the moment it grows one it becomes a
    // story object and every spread of it is in the wrong place
    expect(Object.keys(withPageA11y)).toEqual(["a11y"]);
  });
});

describe("PAGE_A11Y_STORY_ID", () => {
  it("matches every story file that spreads the fixture", () => {
    // the runner falls back to the story id so a page story that lost the
    // fixture — and with it its own `expectRules` claim — still fails. That
    // only holds while the pattern names every such file
    const titles = pageStoryTitles();
    expect(titles).toEqual(["Screens/Login", "Shell/App"]);
    for (const title of titles) {
      expect(PAGE_A11Y_STORY_ID.test(`${titleToId(title)}--default`)).toBe(true);
    }
  });

  it("matches the real ids and nothing else", () => {
    expect(PAGE_A11Y_STORY_ID.test("shell-app--mobile")).toBe(true);
    expect(PAGE_A11Y_STORY_ID.test("screens-login--wrong-password")).toBe(true);
    expect(PAGE_A11Y_STORY_ID.test("screens-keys--loaded")).toBe(false);
    expect(PAGE_A11Y_STORY_ID.test("components-shell-app-bar--default")).toBe(false);
  });
});
