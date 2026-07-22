import { expect, test } from "@playwright/test";

// authenticated smoke: every built screen mounts without throwing an uncaught
// error, and the app shell (nav) stays put across navigation. uses the seeded
// storageState from global-setup (default project in playwright.config).

const SCREENS = [
  "dashboard",
  "model-catalog",
  "providers",
  "virtual-keys",
  "logs",
  "audit-logs",
];

test("built screens mount without uncaught errors", async ({ page }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(err.message));

  for (const screen of SCREENS) {
    await page.goto(`/${screen}`);
    // the shell renders the brand mark on every authenticated screen
    await expect(page.getByText("rolter", { exact: false }).first()).toBeVisible();
    // the route resolved to this screen (not bounced back to the login gate)
    await expect(page).toHaveURL(new RegExp(`/${screen}$`));
  }

  expect(pageErrors, `uncaught errors: ${pageErrors.join("; ")}`).toHaveLength(0);
});

test("unknown route redirects to the dashboard", async ({ page }) => {
  await page.goto("/definitely-not-a-real-route");
  await expect(page).toHaveURL(/\/dashboard$/);
});
