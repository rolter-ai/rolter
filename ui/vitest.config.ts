// The story tests: every story is a Vitest test in headless chromium (#2907).
//
//   bun run test:stories [files]     # the guarded run, see scripts/run-story-tests.ts
//   bun run test-storybook           # the bare vitest run, without the guard
//
// `@storybook/addon-vitest` reads `.storybook/main.ts`, takes the files its
// `stories` globs match as the test files and turns each story export into one
// test: the story renders, its play function runs, and `afterEach` in
// `.storybook/preview.ts` runs axe. Nothing here is a second list of stories.
//
// The unit tests (`bun test src scripts`) are a different runner and not part of
// this config; there is one project, `storybook`.
import { storybookTest } from "@storybook/addon-vitest/vitest-plugin";
import { playwright } from "@vitest/browser-playwright";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

// a chromium that is already installed, for a sandbox where the download the
// pinned playwright wants is blocked (#2678). playwright has no variable of its
// own for this: PLAYWRIGHT_BROWSERS_PATH only moves the cache and still looks for
// the exact revision. unset, nothing changes
const executablePath = process.env.ROLTER_CHROMIUM_PATH;

// `ROLTER_AXE_TALLY=<path>` re-measures the whole axe band (#1244): the preview
// stops excluding the page-level rules and stops failing, and the reporter
// appends one JSON line per story. see docs/dev-docs/development/testing.md
const axeTally = process.env.ROLTER_AXE_TALLY;

// where `bun run test:stories` wants vitest's JSON report; unset on a bare run
const reportFile = process.env.ROLTER_STORY_REPORT;

// the reporters live here and nowhere else: `--reporter` on a command line
// replaces this list rather than adding to it, which is how a tally run under
// the wrapper once lost its writer
const reporters: (string | [string, Record<string, unknown>])[] = ["default"];
// the wrapper's check that no story file ran short reads this report
if (reportFile) reporters.push(["json", { outputFile: reportFile }]);
if (axeTally) reporters.push("./.storybook/axe-tally-reporter.ts");

export default defineConfig({
  test: {
    reporters,
    // a story that passes has nothing to say: the dashboard logs React `act`
    // notices and query warnings from its stubs on nearly every screen story, and
    // across 2,000 of them they bury the one failure that is worth reading. a
    // failing story still prints everything it logged. a run-wide option, which
    // is why it is not on the project below
    silent: "passed-only",
    projects: [
      {
        extends: true,
        plugins: [
          storybookTest({
            configDir: fileURLToPath(new URL("./.storybook", import.meta.url)),
            // the addon sizes the viewport to 1200x900 for a story that names none,
            // and the dashboard's breakpoints (the LLM Logs drawer needs `xl`, 1280)
            // are drawn for the 1280x800 desktop the stories were written at. the
            // size itself is `DESKTOP` in src/lib/story-viewport.ts, which the
            // preview registers under this name. this config cannot import it:
            // that module pulls in `storybook/test`
            initialGlobals: { viewport: { value: "rolterDesktop", isRotated: false } },
          }),
        ],
        test: {
          name: "storybook",
          // the 15 seconds a whole story had under the test runner's jest. a play
          // function that waits out several polling intervals (Logs, Dashboard) is
          // written against this budget
          testTimeout: 15_000,
          // the preview's `beforeAll` loads every font face before the first story
          hookTimeout: 30_000,
          env: axeTally ? { ROLTER_AXE_TALLY: "1" } : {},
          browser: {
            enabled: true,
            headless: true,
            provider: playwright(executablePath ? { launchOptions: { executablePath } } : {}),
            instances: [{ browser: "chromium" }],
          },
        },
      },
    ],
  },
});
