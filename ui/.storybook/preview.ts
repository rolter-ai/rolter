import "../src/index.css";

import type { Preview } from "@storybook/react";

const preview: Preview = {
  parameters: {
    controls: { expanded: true },
    layout: "fullscreen",
  },
};

export default preview;
