// Axe fixtures for the stories that mount a whole page (#1353).
//
// `.storybook/test-runner.ts` disables three page-level rules by default —
// `region`, `landmark-one-main` and `page-has-heading-one` — because a story
// is normally one component rendered into a bare iframe with no app shell
// around it, and all three describe a page rather than a component.
//
// A story that *does* mount a page (`Shell/App`, `Screens/Login`) spreads the
// fragment below into its `parameters` to turn them back on; the runner merges
// `parameters.a11y.rules` over its own map, so this is the whole opt-in.
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
// `expectRules` marker below, which the runner verifies arrived, and
// `scripts/check-story-parameters.ts`, which fails a spread of this fixture
// placed anywhere but inside a `parameters` object.

/** The three page-level axe rules `.storybook/test-runner.ts` turns off by default. */
export const PAGE_A11Y_RULE_IDS = [
  "region",
  "landmark-one-main",
  "page-has-heading-one",
] as const;

// the runner checks these two story families by id rather than trusting the
// parameter it is handed: a fixture that carries `parameters` can be dropped
// without a word, so "the story says nothing" and "the story mounts no page"
// have to be told apart from outside the story (#1373). adding a third page
// story means adding it here, and `story-a11y.test.ts` checks the pattern
// against the titles that actually spread `withPageA11y`
/** Story-id prefixes whose stories mount a whole page (`Shell/App`, `Screens/Login`). */
export const PAGE_A11Y_STORY_ID = /^(shell-app|screens-login)--/;

/** `parameters` fields that gate the page-level axe rules the runner defaults off. */
export const withPageA11y = {
  a11y: {
    // the marker the runner verifies: `expectRules` says "these rule ids must
    // be enabled by the time postVisit runs". it travels with the rules, so a
    // spread that lands in the wrong place takes the claim with it and the
    // runner fails the story instead of passing it unchecked (#1373)
    expectRules: PAGE_A11Y_RULE_IDS,
    rules: Object.fromEntries(PAGE_A11Y_RULE_IDS.map((id) => [id, { enabled: true }])),
  },
};
