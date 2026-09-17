import { expect, test } from "@playwright/test";

import { t } from "../i18n";

// virtual-key lifecycle: mint -> the created-key dialog shows the secret once ->
// revoke. driven through the dashboard against the live control plane; the
// seeded scope (global-setup) pins the project so minting is enabled.
//
// controls are found by role and by the accessible name the catalog gives them,
// so a rewording in en.json moves the spec with it (#1504)

function uniqueName(): string {
  return `e2e-key-${Math.random().toString(36).slice(2, 8)}`;
}

test("virtual key mint → reveal → revoke", async ({ page }) => {
  const name = uniqueName();
  await page.goto("/virtual-keys");

  // mint. a project with no keys yet shows the header button and the empty
  // state's CTA side by side, both opening the same sheet — the header one is
  // there in every state, so it is the one to drive
  await page.getByRole("button", { name: t("pages.virtualKeys.add"), exact: true }).click();
  const sheet = page.getByRole("dialog");
  await sheet.getByLabel(t("keyMint.name"), { exact: true }).fill(name);
  await sheet.getByRole("button", { name: t("common.create"), exact: true }).click();

  // the created-key dialog reveals the plaintext secret exactly once
  const created = page.getByRole("dialog");
  await expect(created.getByText(t("pages.virtualKeys.createdTitle"))).toBeVisible();
  const secret = created.locator("code");
  await expect(secret).toBeVisible();
  await expect(secret).not.toBeEmpty();
  await created.getByRole("button", { name: t("common.done"), exact: true }).click();

  // the new key is listed, and its row actions are named after it
  const revoke = page.getByRole("button", {
    name: t("pages.virtualKeys.deleteKey", { name }),
    exact: true,
  });
  await expect(revoke).toBeVisible();

  // revoke — trash opens a confirm dialog, confirm removes the row
  await revoke.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(t("pages.virtualKeys.deleteTitle"))).toBeVisible();
  await dialog.getByRole("button", { name: t("common.delete"), exact: true }).click();

  await expect(revoke).toHaveCount(0);
});
