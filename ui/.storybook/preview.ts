// the vendored geist faces, from the same module `main.tsx` imports: without
// it every story is drawn in whatever fallback the browser has (#2051)
import "../src/lib/fonts";
import "../src/index.css";
// side-effect init of i18next so any story rendering a `useTranslation`
// component (the nav sidebar, the locale picker) resolves copy instead of
// echoing raw keys (#489)
import "../src/lib/i18n";

import * as React from "react";
import type { Decorator, Preview } from "@storybook/react";
import { configure } from "storybook/test";

import { DEFAULT_LOCALE, LOCALES, LOCALE_NAMES, setLocale, type Locale } from "../src/lib/i18n";
import { a11yGate, pageGateProblem } from "../src/lib/story-a11y";
import { loadTokenFonts } from "../src/lib/story-fonts";
import { DESKTOP_VIEWPORT } from "../src/lib/story-viewport";

// `ROLTER_AXE_TALLY` is set by vitest.config.ts, which passes it into the browser
// as an env var; under `storybook dev` and `storybook build` it is never there
const axeTally = Boolean(import.meta.env.ROLTER_AXE_TALLY);

// testing-library waits one second by default, which is a unit-test budget: it
// assumes the thing being awaited is a render, and a render is immediate. a
// screen story is not that. it mounts a page that resolves org, then team, then
// project, then its own endpoints, each one a fetch through the stub and a
// react-query transition, and the whole chain has to finish before the first
// `findByText` can see anything.
//
// on an idle machine that chain lands in a couple of hundred milliseconds, so
// the default holds and every story passes in isolation. under the story tests,
// which run the suite across as many workers as the box has cores, it does not:
// #1279 caught `Screens/Rbac` timing out on a branch that touched no RBAC code,
// with the failure dump showing the screen still on its tab header — the
// assertion was correct and the data was still in flight. re-running that one
// file passed in 3.5s.
//
// a per-assertion timeout would have to be repeated at every first-paint
// assertion in ~100 story files and would be forgotten in the next one, so the
// budget is set once, here, for every story. it is a ceiling on how long a
// *failing* assertion waits, never a delay a passing one pays, so the suite does
// not get slower — only less willing to call a slow machine a broken screen.
configure({ asyncUtilTimeout: 5000 });

// the dashboard theme is dark-only (see the src/index.css
// header): `:root` is the dark surface and there is no light variant, so stories
// render on the theme's dark canvas rather than a fabricated light mode. every
// story is wrapped in the base background/foreground tokens + a little padding so
// components sit on the real surface they ship against.
const withSurface: Decorator = (Story) =>
  React.createElement(
    "div",
    {
      className: "bg-background text-foreground",
      style: { minHeight: "100vh", padding: "1.5rem" },
    },
    React.createElement(Story),
  );

// storybook has no url/localStorage story for locale, so the toolbar owns it:
// flip the globe to proof a component against a longer-word language
const withLocale: Decorator = (Story, context) => {
  const locale = (context.globals.locale as Locale | undefined) ?? DEFAULT_LOCALE;
  React.useEffect(() => {
    void setLocale(locale);
  }, [locale]);
  return React.createElement(Story);
};

const preview: Preview = {
  // `font-display: swap` fetches a face only when text first asks for it and
  // draws the fallback until then, so a story measured or screenshotted in that
  // window sees the fallback's metrics. load every face once, before the first
  // story renders. `Behaviour/Fonts` asserts the faces are the ones on screen
  beforeAll: loadTokenFonts,
  decorators: [withLocale, withSurface],
  // the guard behind `withPageA11y` (#1373): a page story has to get the three
  // page-level rules enabled, with axe looking at the whole document, or its axe
  // check asserts less than it claims. the rule is `pageGateProblem`
  // (src/lib/story-a11y.ts, unit-tested); a tally measures rather than asserts
  afterEach: ({ id, parameters }) => {
    if (axeTally) return;
    const problem = pageGateProblem(id, parameters.a11y);
    if (problem) throw new Error(problem);
  },
  globalTypes: {
    locale: {
      description: "Dashboard language",
      defaultValue: DEFAULT_LOCALE,
      toolbar: {
        icon: "globe",
        items: LOCALES.map((l) => ({ value: l, title: LOCALE_NAMES[l] })),
        dynamicTitle: true,
      },
    },
  },
  parameters: {
    // every story is an accessibility test (#1181), run by addon-a11y's own
    // `afterEach`; see `a11yGate` for the rules and why each is off
    a11y: a11yGate(axeTally),
    // the size a story is drawn at when it names none. the viewport addon only
    // sizes the preview iframe inside the Storybook UI; under vitest the addon
    // calls `page.viewport` with whatever `globals.viewport` names, and
    // vitest.config.ts makes this the default global
    viewport: { options: { rolterDesktop: DESKTOP_VIEWPORT } },
    controls: { expanded: true },
    layout: "fullscreen",
    backgrounds: {
      default: "surface-base",
      values: [{ name: "surface-base", value: "#111113" }],
    },
  },
};

export default preview;
