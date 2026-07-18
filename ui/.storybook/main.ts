import type { StorybookConfig } from "@storybook/react-vite";
import { fileURLToPath, URL } from "node:url";
import { mergeConfig } from "vite";

const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  addons: ["@storybook/addon-essentials"],
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
