import type { StorybookConfig } from "@storybook/react-vite";
import { fileURLToPath, URL } from "node:url";
import { mergeConfig } from "vite";

const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  // Storybook 10 folds the former "essentials" (controls, actions, viewport,
  // backgrounds, toolbars) into core; docs is the one still-separate addon.
  addons: ["@storybook/addon-docs", "@storybook/addon-mcp"],
  features: {
    componentsManifest: true,
    // `build-storybook` bundles React's development build, the one `storybook
    // dev` and `bun run dev` run. React's production build never double-invokes
    // the effects under a StrictMode, so against it every StrictModeHost story
    // fails `expectDoubleInvoked()`, and before that probe existed the
    // StrictMode stories passed without the lifecycle they assert on ever
    // running (#1887). the static build is the fixture CI runs the play tests
    // against and is published nowhere
    developmentModeForBuild: true,
  },
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
