import type { StorybookConfig } from "@storybook/react-vite";
import { fileURLToPath, URL } from "node:url";
import { mergeConfig } from "vite";

const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(ts|tsx)"],
  // Storybook 10 folds the former "essentials" (controls, actions, viewport,
  // backgrounds, toolbars) into core; docs is the one still-separate addon.
  // addon-vitest is the story tests (vitest.config.ts) and addon-a11y is their
  // axe check: its `afterEach` comes from the preview annotations of the addons
  // registered here, so dropping it would leave every story unchecked
  addons: [
    "@storybook/addon-docs",
    "@storybook/addon-mcp",
    "@storybook/addon-vitest",
    "@storybook/addon-a11y",
  ],
  features: {
    componentsManifest: true,
    // `build-storybook` bundles React's development build, the one `storybook
    // dev` and `bun run dev` run. React's production build never double-invokes
    // the effects under a StrictMode, so against it every StrictModeHost story
    // fails `expectDoubleInvoked()`, and before that probe existed the
    // StrictMode stories passed without the lifecycle they assert on ever
    // running (#1887). the story tests run on vite's own dev transform, which is
    // the development build too, so this keeps the static build, published
    // nowhere, the same React as the thing the tests ran
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
