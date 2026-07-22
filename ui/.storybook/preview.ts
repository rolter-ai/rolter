import "../src/index.css";

import * as React from "react";
import type { Decorator, Preview } from "@storybook/react";

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

const preview: Preview = {
  decorators: [withSurface],
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
