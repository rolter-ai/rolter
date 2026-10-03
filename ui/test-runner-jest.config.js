import { getJestConfig } from "@storybook/test-runner";

// the story runner's jest config, the stock one plus one optional override: when
// ROLTER_CHROMIUM_PATH names a chromium binary, launch that instead of the
// revision the pinned playwright downloads (#2678). unset, nothing changes
const config = getJestConfig();
const executablePath = process.env.ROLTER_CHROMIUM_PATH;

export default executablePath
  ? {
      ...config,
      testEnvironmentOptions: {
        ...config.testEnvironmentOptions,
        "jest-playwright": {
          ...config.testEnvironmentOptions?.["jest-playwright"],
          launchOptions: { executablePath },
        },
      },
    }
  : config;
