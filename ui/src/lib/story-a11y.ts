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

/** `parameters` fields that gate the page-level axe rules the runner defaults off. */
export const withPageA11y = {
  a11y: {
    rules: {
      region: { enabled: true },
      "landmark-one-main": { enabled: true },
      "page-has-heading-one": { enabled: true },
    },
  },
};
