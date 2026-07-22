import { defineConfig, devices } from "@playwright/test";

// browser-level e2e for the dashboard SPA (#619). drives the built app via the
// vite dev server (which proxies /api -> control:4001 and /gw -> gateway:4000)
// against a running fake-vLLM rolter stack (integration/e2e docker compose).
//
// bring the stack up first (from repo root):
//   ROLTER_KEK=$(...) docker compose -f integration/e2e/docker-compose.e2e.yml up -d --wait
// then: cd ui && bun run e2e
//
// global-setup seeds a tenant + admin user via the control admin API and writes
// an authenticated storageState, so specs start logged in; login.spec exercises
// the actual login UI separately.
const PORT = Number(process.env.E2E_UI_PORT) || 3000;
const BASE_URL = process.env.E2E_BASE_URL || `http://localhost:${PORT}`;

export default defineConfig({
  testDir: "./e2e/tests",
  outputDir: "./e2e/.output",
  globalSetup: "./e2e/global-setup.ts",
  // the app talks to a real stack; keep it serial and give generous timeouts so
  // first-paint + snapshot propagation don't flake
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  expect: { timeout: 10_000 },
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: BASE_URL,
    // authenticated session seeded by global-setup (localStorage token)
    storageState: "./e2e/.auth/state.json",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  webServer: {
    command: "bun run dev",
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
