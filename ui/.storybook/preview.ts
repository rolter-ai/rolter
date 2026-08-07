import "../src/index.css";
// side-effect init of i18next so any story rendering a `useTranslation`
// component (the nav sidebar, the locale picker) resolves copy instead of
// echoing raw keys (#489)
import "../src/lib/i18n";

import * as React from "react";
import type { Decorator, Preview } from "@storybook/react";

import { DEFAULT_LOCALE, LOCALES, LOCALE_NAMES, setLocale, type Locale } from "../src/lib/i18n";

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
