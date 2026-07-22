import type { StorybookConfig } from "@storybook/react-vite";
import { fileURLToPath, URL } from "node:url";
import { mergeConfig } from "vite";

const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  // Storybook 10 folds the former "essentials" (controls, actions, viewport,
  // backgrounds, toolbars) into core; docs is the one still-separate addon.
  addons: ["@storybook/addon-docs"],
  framework: {
    name: "@storybook/react-vite",
    options: {},
  },
  async viteFinal(base) {
    return mergeConfig(base, {
      define: {
        __APP_VERSION__: JSON.stringify("storybook"),
      },
      resolve: {
        alias: {
          "@": fileURLToPath(new URL("../src", import.meta.url)),
        },
      },
    });
  },
};

export default config;
