import type { TestRunnerConfig } from "@storybook/test-runner";
import { getStoryContext } from "@storybook/test-runner";
import { appendFileSync } from "node:fs";
import { checkA11y, getViolations, injectAxe } from "axe-playwright";

import { PAGE_A11Y_RULE_IDS, PAGE_A11Y_STORY_ID } from "../src/lib/story-a11y.ts";

// The viewport addon sizes the preview iframe inside the Storybook UI. The test
// runner drives the iframe directly, where nothing does, so a "fits at 375px"
// story would otherwise be measured at the browser default and assert nothing.
//
// `parameters.viewportSize` — set by `src/lib/story-viewport.ts` — is the width
// the story means; every other story is pinned to a desktop width so the one
// before it cannot leave the page narrow.
const DESKTOP = { width: 1280, height: 800 };

// every story is an accessibility test (#1181): after the play function has
// put the screen into its state, axe runs over the rendered root and fails
// the story on any violation at any impact. a story that needs an exception
// opts out with `parameters.a11y.disable = true` and says why beside it
//
// the gate started at serious+critical only. #1244 measured the rest of the
// band over all 695 stories and it came to six rules; the three small ones
// are fixed, and the three below are excluded by name with a reason, so
// neither half of the band can drift unnoticed. re-measure with
// ROLTER_AXE_TALLY (see docs/development/testing.md)
const DISABLED_RULES: Record<string, { enabled: boolean }> = {
  // storybook's iframe, not ours: the dashboard's index.html sets both
  "document-title": { enabled: false },
  "html-has-lang": { enabled: false },
  // the next three describe a *page*, and a story is one component (or one
  // screen body) rendered into a bare iframe with no app shell around it.
  // the landmarks, the <main> and the <h1> they ask for live in App.tsx and
  // components/screen.tsx, so asserting them on a component story would only
  // ever fail, and passing them would mean every story grew a fake shell.
  //
  // they are off *by default*, not unchecked: the stories that do mount a
  // whole page turn them back on through `parameters.a11y.rules`, which is
  // merged over this map below. Shell/App mounts the assembled shell at all
  // three widths and Screens/Login mounts the signed-out page, so between them
  // every landmark, the <main> and the <h1> are gated on every PR (#1353).
  // any other story is one component and keeps them off
  "region": { enabled: false },
  "landmark-one-main": { enabled: false },
  "page-has-heading-one": { enabled: false },
};

const config: TestRunnerConfig = {
  async preVisit(page, context) {
    const story = await getStoryContext(page, context);
    const size = story.parameters?.viewportSize as
      | { width: number; height: number }
      | undefined;
    await page.setViewportSize(size ?? DESKTOP);
    await injectAxe(page);
  },
  async postVisit(page, context) {
    const story = await getStoryContext(page, context);
    const a11y = story.parameters?.a11y as
      | {
          disable?: boolean;
          expectRules?: readonly string[];
          rules?: Record<string, { enabled: boolean }>;
        }
      | undefined;
    if (a11y?.disable) return;
    // a story that mounts a whole page re-enables the page-level rules above
    // by name; its map wins over the defaults, so an override is additive and
    // never silently loosens the gate for everything else
    const rules = { ...DISABLED_RULES, ...(a11y?.rules ?? {}) };
    // a dropped `parameters` spread is the one failure a gate cannot survive:
    // the story runs, the override is gone and the run is green while nothing
    // is asserted (#1373). two claims are checked against the merged map above
    // — `parameters.a11y.expectRules`, which `withPageA11y` carries so a
    // fixture that arrived says which rules it bought, and the story id, so a
    // page story that lost the fixture entirely (and with it its own claim)
    // still fails
    const expectRules = PAGE_A11Y_STORY_ID.test(context.id)
      ? PAGE_A11Y_RULE_IDS
      : (a11y?.expectRules ?? []);
    const missing = expectRules.filter((rule) => !rules[rule]?.enabled);
    if (missing.length > 0) {
      throw new Error(
        `${context.id}: axe rules ${missing.join(", ")} should be enabled for this story but are not. ` +
          "spread `withPageA11y` from src/lib/story-a11y.ts *inside* the meta's `parameters` object " +
          "(`parameters: { ...withPageA11y }`) — spread as a bare story or meta field it is replaced " +
          "by the docgen transform and the story passes asserting nothing (#1373).",
      );
    }
    // ROLTER_AXE_TALLY=<path> re-measures the whole band (#1244): every
    // violation at every impact is appended as one JSON line per story so the
    // per-rule table in docs/development/testing.md can be regenerated. it
    // never fails a story — the gate below still does that
    if (process.env.ROLTER_AXE_TALLY) {
      const found = await getViolations(page, undefined, {
        runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "best-practice"] },
        // the tally deliberately keeps the rules the gate excludes, so the
        // reason for each exclusion can be re-checked rather than assumed
        rules: { "document-title": { enabled: false }, "html-has-lang": { enabled: false } },
      });
      appendFileSync(
        process.env.ROLTER_AXE_TALLY,
        `${JSON.stringify({
          story: context.id,
          violations: found.map((v) => ({ id: v.id, impact: v.impact, nodes: v.nodes.length })),
        })}\n`,
      );
    }
    // the whole document rather than #storybook-root: dialogs, sheets and
    // toasts portal to <body>, and they are exactly what needs checking
    await checkA11y(page, undefined, {
      detailedReport: true,
      detailedReportOptions: { html: true },
      axeOptions: {
        runOnly: { type: "tag", values: ["wcag2a", "wcag2aa", "best-practice"] },
        rules,
      },
      includedImpacts: ["minor", "moderate", "serious", "critical"],
    });
  },
};

export default config;
