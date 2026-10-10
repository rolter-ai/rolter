import { describe, it, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";

import {
  A11Y_CONTEXT,
  A11Y_TAGS,
  PAGE_A11Y_RULE_IDS,
  PAGE_A11Y_STORY_ID,
  a11yGate,
  pageGateProblem,
  withPageA11y,
} from "./story-a11y";

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
    // `expectRules` is the marker the preview verifies arrived (#1373). If it
    // ever listed a rule the fragment does not actually enable, every page
    // story would fail; if it listed fewer, a dropped rule would pass.
    const { expectRules, options } = withPageA11y.a11y;
    expect(expectRules).toEqual(PAGE_A11Y_RULE_IDS);
    expect(Object.keys(options.rules).sort()).toEqual([...PAGE_A11Y_RULE_IDS].sort());
    for (const rule of PAGE_A11Y_RULE_IDS) {
      expect(options.rules[rule]).toEqual({ enabled: true });
    }
  });

  it("carries no `parameters` key of its own", () => {
    // it is a fragment *of* `parameters`; the moment it grows one it becomes a
    // story object and every spread of it is in the wrong place
    expect(Object.keys(withPageA11y)).toEqual(["a11y"]);
  });

  it("turns back on exactly the rules the gate turns off by default", () => {
    // the opt-in is only an opt-in while the gate disables what it enables
    const disabled = a11yGate()
      .config.rules.filter((r) => !r.enabled)
      .map((r) => r.id);
    for (const rule of PAGE_A11Y_RULE_IDS) expect(disabled).toContain(rule);
  });
});

describe("a11yGate", () => {
  it("fails a story on a violation, at every impact, over the whole document", () => {
    const gate = a11yGate();
    // `error` fails the story; `todo` would only report it
    expect(gate.test).toBe("error");
    // the document, because axe skips the page-level rules on anything narrower
    expect(gate.context).toBe(A11Y_CONTEXT);
    expect(gate.context).toBe("html");
    expect(gate.options.runOnly).toEqual({ type: "tag", values: [...A11Y_TAGS] });
    expect(A11Y_TAGS).toEqual(["wcag2a", "wcag2aa", "best-practice"]);
  });

  it("names every rule it excludes, and nothing else", () => {
    const rules = a11yGate().config.rules;
    expect(rules.map((r) => r.id).sort()).toEqual([
      "document-title",
      "html-has-lang",
      "landmark-one-main",
      "page-has-heading-one",
      "region",
    ]);
    expect(rules.every((r) => r.enabled === false)).toBe(true);
  });

  it("measures without failing in a tally, and keeps only storybook's own two rules off", () => {
    const gate = a11yGate(true);
    expect(gate.test).toBe("todo");
    expect(gate.config.rules.map((r) => r.id).sort()).toEqual(["document-title", "html-has-lang"]);
    // addon-a11y always switches `region` off itself; the tally counts it
    expect(gate.options.rules).toEqual({ region: { enabled: true } });
  });
});

describe("pageGateProblem", () => {
  const arrived = {
    expectRules: PAGE_A11Y_RULE_IDS,
    context: A11Y_CONTEXT,
    options: { rules: withPageA11y.a11y.options.rules },
  };

  it("says nothing about an ordinary component story", () => {
    expect(pageGateProblem("screens-keys--loaded", { context: A11Y_CONTEXT })).toBeNull();
    expect(pageGateProblem("screens-keys--loaded", undefined)).toBeNull();
  });

  it("passes a page story whose fixture arrived", () => {
    expect(pageGateProblem("shell-app--desktop", arrived)).toBeNull();
  });

  it("fails a story that claims the rules but did not get them", () => {
    // a fixture spread one level too high: the claim is there, the rules are not
    const problem = pageGateProblem("some-page--story", { ...arrived, options: { rules: {} } });
    expect(problem).toContain("axe rules region, landmark-one-main, page-has-heading-one");
    expect(problem).toContain("#1373");
  });

  it("fails a page story that lost the fixture entirely — claim and all", () => {
    expect(pageGateProblem("screens-login--wrong-password", { context: A11Y_CONTEXT })).toContain(
      "should be enabled for this story but are not",
    );
    expect(pageGateProblem("shell-app--mobile", undefined)).toContain("#1373");
  });

  it("names only the rules that are missing", () => {
    const problem = pageGateProblem("shell-app--desktop", {
      ...arrived,
      options: { rules: { region: { enabled: true } } },
    });
    expect(problem).toContain("axe rules landmark-one-main, page-has-heading-one should");
  });

  it("fails rules that are enabled but cannot run, on a context narrower than the page", () => {
    // axe skips landmark-one-main and page-has-heading-one on <body>, so
    // enabling them there is the silent green the context check exists for
    const problem = pageGateProblem("shell-app--desktop", { ...arrived, context: "body" });
    expect(problem).toContain("not the whole document");
    expect(pageGateProblem("shell-app--desktop", { ...arrived, context: undefined })).toContain(
      "not the whole document",
    );
  });

  it("lets a story opt out of the gate, the check included", () => {
    expect(pageGateProblem("shell-app--desktop", { disable: true })).toBeNull();
    expect(pageGateProblem("shell-app--desktop", { test: "off" })).toBeNull();
  });
});

describe("PAGE_A11Y_STORY_ID", () => {
  it("matches every story file that spreads the fixture", () => {
    // the preview falls back to the story id so a page story that lost the
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
