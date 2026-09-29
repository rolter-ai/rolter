import "../src/index.css";
// side-effect init of i18next so any story rendering a `useTranslation`
// component (the nav sidebar, the locale picker) resolves copy instead of
// echoing raw keys (#489)
import "../src/lib/i18n";

import * as React from "react";
import type { Decorator, Preview } from "@storybook/react";
import { configure } from "storybook/test";

import { DEFAULT_LOCALE, LOCALES, LOCALE_NAMES, setLocale, type Locale } from "../src/lib/i18n";

// testing-library waits one second by default, which is a unit-test budget: it
// assumes the thing being awaited is a render, and a render is immediate. a
// screen story is not that. it mounts a page that resolves org, then team, then
// project, then its own endpoints, each one a fetch through the stub and a
// react-query transition, and the whole chain has to finish before the first
// `findByText` can see anything.
//
// on an idle machine that chain lands in a couple of hundred milliseconds, so
// the default holds and every story passes in isolation. under `test-storybook`,
// which runs the suite across as many workers as the box has cores, it does not:
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

// the Rolter Design System is a dark-only control-plane theme (see src/index.css
// header): `:root` is the dark surface and there is no light variant, so stories
// render on the design's dark canvas rather than a fabricated light mode. every
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
  decorators: [withLocale, withSurface],
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
    controls: { expanded: true },
    layout: "fullscreen",
    backgrounds: {
      default: "surface-base",
      values: [{ name: "surface-base", value: "#111113" }],
    },
  },
};

export default preview;
