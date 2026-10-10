// Axe for every story (#1181), and the fixture for the stories that mount a whole
// page (#1353).
//
// Every story is an accessibility test: `@storybook/addon-a11y` runs axe in the
// preview's `afterEach` once the play function has finished, and `test: "error"`
// fails the story on **any violation at any impact** (#1244). `a11yGate` below
// is that configuration, applied to every story by `.storybook/preview.ts`.
//
// It disables five rules by name, each with its reason beside it. Two describe
// Storybook's iframe rather than the dashboard. The other three describe a
// *page*, and a story is normally one component or one screen body rendered into
// a bare iframe with no app shell around it, so asserting them on a component
// story would only ever fail.
//
// A story that *does* mount a page (`Shell/App`, `Screens/Login`) spreads the
// fragment at the bottom into its `parameters` to turn those three back on; its
// `options.rules` is merged over the gate's, so this is the whole opt-in.
// Without it the landmarks, the `<main>` and the `<h1>` of the assembled shell
// would be checked nowhere.
//
// It is a `parameters` fragment rather than a whole story object on purpose:
// the JSDoc docgen transform appends a `parameters: { docs: … }` of its own to
// every meta, and a spread that carried `parameters` would simply be replaced
// by it — silently, with the story still green.
//
// Not a `.stories.tsx` file: it is a fixture, like `story-viewport.ts`.
//
// Prose alone did not stop that (#1373), so two things now enforce it: the
// `expectRules` marker below, which the preview's `afterEach` verifies arrived,
// and `scripts/check-story-parameters.ts`, which fails a spread of this fixture
// placed anywhere but inside a `parameters` object.

/** The three page-level axe rules the gate turns off by default. */
export const PAGE_A11Y_RULE_IDS = ["region", "landmark-one-main", "page-has-heading-one"] as const;

// the preview checks these two story families by id rather than trusting the
// parameter it is handed: a fixture that carries `parameters` can be dropped
// without a word, so "the story says nothing" and "the story mounts no page"
// have to be told apart from outside the story (#1373). adding a third page
// story means adding it here, and `story-a11y.test.ts` checks the pattern
// against the titles that actually spread `withPageA11y`
/** Story-id prefixes whose stories mount a whole page (`Shell/App`, `Screens/Login`). */
export const PAGE_A11Y_STORY_ID = /^(shell-app|screens-login)--/;

/**
 * What axe looks at: the whole document, `<html>` included.
 *
 * The addon's own default is `<body>`, and axe skips the page-level rules —
 * `landmark-one-main` and `page-has-heading-one` are two of them — on any
 * context that is not the whole page. On the default, a page story that turned
 * them back on would assert nothing. The dialogs, sheets and toasts that portal
 * to `<body>` are inside this either way.
 */
export const A11Y_CONTEXT = "html";

/** The rule sets the gate runs: `wcag2a` + `wcag2aa` + `best-practice`. */
export const A11Y_TAGS = ["wcag2a", "wcag2aa", "best-practice"] as const;

/**
 * Rules the gate excludes, with the reason. `ROLTER_AXE_TALLY` keeps the first
 * two excluded and turns the rest back on, so the reason for each exclusion can
 * be re-checked rather than assumed (see docs/dev-docs/development/testing.md).
 */
const EXCLUDED_RULES: Record<string, { why: string; tally: boolean }> = {
  // storybook's iframe, not ours: the dashboard's index.html sets both
  "document-title": { why: "the iframe's title is Storybook's", tally: true },
  "html-has-lang": { why: "the iframe's lang is Storybook's", tally: true },
  // the next three describe a *page*: the landmarks, the <main> and the <h1> they
  // ask for live in App.tsx and components/screen.tsx. they are off *by default*,
  // not unchecked: the stories that do mount a whole page turn them back on
  // through `parameters.a11y.options.rules` (`withPageA11y`). Shell/App mounts
  // the assembled shell at all three widths and Screens/Login mounts the
  // signed-out page, so between them every landmark, the <main> and the <h1> are
  // gated on every PR (#1353). any other story is one component and keeps them off
  region: { why: "describes a page", tally: false },
  "landmark-one-main": { why: "describes a page", tally: false },
  "page-has-heading-one": { why: "describes a page", tally: false },
};

/**
 * The `parameters.a11y` every story runs under.
 *
 * With `tally` the gate stops failing (`todo` reports a violation without
 * failing the story) and keeps only the rules in `EXCLUDED_RULES` marked `tally`
 * excluded, which is how `ROLTER_AXE_TALLY` re-measures the band.
 */
export function a11yGate(tally = false) {
  return {
    test: tally ? ("todo" as const) : ("error" as const),
    context: A11Y_CONTEXT,
    options: {
      runOnly: { type: "tag" as const, values: [...A11Y_TAGS] },
      // the addon always disables `region` itself; in a tally it is a rule to count
      rules: tally ? { region: { enabled: true } } : {},
    },
    config: {
      rules: Object.entries(EXCLUDED_RULES)
        .filter(([, rule]) => !tally || rule.tally)
        .map(([id]) => ({ id, enabled: false })),
    },
  };
}

/** The parts of `parameters.a11y` the guard below reads. */
interface A11yClaims {
  disable?: boolean;
  test?: string;
  context?: unknown;
  expectRules?: readonly string[];
  options?: { rules?: Record<string, { enabled?: boolean } | undefined> };
}

/**
 * Why a page story's axe gate is not the one it claims, or null when it is.
 *
 * A dropped `parameters` spread is the one failure a gate cannot survive: the
 * story runs, the override is gone and the run is green while nothing is
 * asserted (#1373). Two claims are checked against the parameters the story
 * actually got — `parameters.a11y.expectRules`, which `withPageA11y` carries so
 * a fixture that arrived says which rules it bought, and the story id, so a page
 * story that lost the fixture entirely (and with it its own claim) still fails.
 * The context is checked beside them: a rule that is enabled but never run, as
 * on the addon's `<body>` default, is the same silent green.
 */
export function pageGateProblem(id: string, a11y: A11yClaims | undefined): string | null {
  if (a11y?.disable || a11y?.test === "off") return null;
  const expected = PAGE_A11Y_STORY_ID.test(id) ? PAGE_A11Y_RULE_IDS : (a11y?.expectRules ?? []);
  if (expected.length === 0) return null;
  const missing = expected.filter((rule) => a11y?.options?.rules?.[rule]?.enabled !== true);
  if (missing.length > 0) {
    return (
      `${id}: axe rules ${missing.join(", ")} should be enabled for this story but are not. ` +
      "spread `withPageA11y` from src/lib/story-a11y.ts *inside* the meta's `parameters` object " +
      "(`parameters: { ...withPageA11y }`) — spread as a bare story or meta field it is replaced " +
      "by the docgen transform and the story passes asserting nothing (#1373)."
    );
  }
  if (a11y?.context !== A11Y_CONTEXT) {
    return (
      `${id}: axe rules ${expected.join(", ")} are enabled but axe is looking at ` +
      `${JSON.stringify(a11y?.context)}, not the whole document. axe skips the page-level rules on ` +
      `anything narrower, so they would assert nothing: leave \`parameters.a11y.context\` at ` +
      `"${A11Y_CONTEXT}".`
    );
  }
  return null;
}

/** `parameters` fields that gate the page-level axe rules the gate defaults off. */
export const withPageA11y = {
  a11y: {
    // the marker the preview verifies: `expectRules` says "these rule ids must
    // be enabled by the time afterEach runs". it travels with the rules, so a
    // spread that lands in the wrong place takes the claim with it and the
    // preview fails the story instead of passing it unchecked (#1373)
    expectRules: PAGE_A11Y_RULE_IDS,
    // `options.rules` wins over the gate's `config.rules` exclusions
    options: {
      rules: Object.fromEntries(PAGE_A11Y_RULE_IDS.map((id) => [id, { enabled: true }])) as Record<
        (typeof PAGE_A11Y_RULE_IDS)[number],
        { enabled: true }
      >,
    },
  },
};
