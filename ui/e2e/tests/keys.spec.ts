import { expect, test } from "@playwright/test";

// virtual-key lifecycle: mint -> the created-key dialog shows the secret once ->
// revoke. driven through the dashboard against the live control plane; the
// seeded scope (global-setup) pins the project so minting is enabled.

function uniqueName(): string {
  return `e2e-key-${Math.random().toString(36).slice(2, 8)}`;
}

test("virtual key mint → reveal → revoke", async ({ page }) => {
  const name = uniqueName();
  await page.goto("/virtual-keys");

  // mint
  await page.getByRole("button", { name: /add virtual key/i }).click();
  await page.getByPlaceholder("backend service").fill(name);
  await page.getByRole("button", { name: /^create$/i }).click();

  // the created-key dialog reveals the plaintext secret exactly once
  const created = page.getByRole("dialog");
  await expect(created.getByText(/key created/i)).toBeVisible();
  const secret = created.locator("code");
  await expect(secret).toBeVisible();
  await expect(secret).not.toBeEmpty();
  // dismiss the reveal dialog
  await page.keyboard.press("Escape");

  // the new key is listed by name
  const row = page.locator("div.grid", { hasText: name });
  await expect(row.first()).toBeVisible();

  // revoke — trash opens a confirm dialog, confirm removes the row
  await row.first().getByRole("button", { name: /delete key/i }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(/delete virtual key/i)).toBeVisible();
  await dialog.getByRole("button", { name: /^delete$/i }).click();

  await expect(page.locator("div.grid", { hasText: name })).toHaveCount(0);
});
