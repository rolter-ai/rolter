import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

// this spec exercises the real login UI, so start from a clean (unauthenticated)
// storage state rather than the seeded one the other specs use
test.use({ storageState: { cookies: [], origins: [] } });

const HERE = path.dirname(fileURLToPath(import.meta.url));
const tenant = JSON.parse(
  readFileSync(path.join(HERE, "..", ".auth", "tenant.json"), "utf8"),
) as { email: string; password: string };

test("login with a real account lands on the dashboard", async ({ page }) => {
  await page.goto("/");

  // the login form is shown when there is no session
  await expect(page.getByRole("heading", { name: /sign in to the control plane/i })).toBeVisible();

  await page.getByRole("textbox", { name: /email/i }).fill(tenant.email);
  await page.locator('input[type="password"]').fill(tenant.password);
  await page.getByRole("button", { name: /sign in/i }).click();

  // the auth gate falls through to the app shell — the login heading is gone and
  // the primary nav is present
  await expect(
    page.getByRole("heading", { name: /sign in to the control plane/i }),
  ).toHaveCount(0);
  await expect(page.getByText("rolter", { exact: false }).first()).toBeVisible();
});
