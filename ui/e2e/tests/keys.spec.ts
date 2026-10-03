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

test("virtual key mint → reveal → revoke", async ({ page, context }) => {
  const name = uniqueName();
  // the reveal asks before an uncopied key is closed over (#2421), so the
  // journey copies it the way an operator would, and reads the clipboard back
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/virtual-keys");

  // mint. a project with no keys yet shows the header button and the empty
  // state's CTA side by side, both opening the same sheet — the header one is
  // there in every state, so it is the one to drive
  await page.getByRole("button", { name: t("pages.virtualKeys.add"), exact: true }).click();
  const sheet = page.getByRole("dialog");
  await sheet.getByLabel(t("keyMint.name"), { exact: true }).fill(name);
  const minted = page.waitForResponse(
    (res) =>
      res.request().method() === "POST" &&
      /\/api\/v1\/projects\/[^/]+\/virtual-keys$/.test(new URL(res.url()).pathname),
  );
  await sheet.getByRole("button", { name: t("common.create"), exact: true }).click();
  const response = await minted;
  expect(response.ok()).toBe(true);
  const { key } = (await response.json()) as { key: string };
  expect(key).not.toBe("");

  // the created-key dialog reveals the plaintext secret exactly once. it is
  // found by its title, and the secret by the value the server minted: the
  // next-step snippet under it carries code of its own
  const created = page.getByRole("dialog", {
    name: t("pages.virtualKeys.createdTitle"),
    exact: true,
  });
  await expect(created.getByText(key, { exact: true })).toBeVisible();
  await created
    .getByRole("button", {
      name: t("common.copyValue", { label: t("common.copy"), value: key }),
      exact: true,
    })
    .click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(key);
  // copied, so done closes without asking
  await created.getByRole("button", { name: t("common.done"), exact: true }).click();
  await expect(created).toHaveCount(0);

  // the new key is listed, and its row actions are named after it
  const revoke = page.getByRole("button", {
    name: t("pages.virtualKeys.deleteKey", { name }),
    exact: true,
  });
  await expect(revoke).toBeVisible();

  // revoke — trash opens a confirm dialog named after the key, confirm removes
  // the row
  await revoke.click();
  const dialog = page.getByRole("dialog", {
    name: t("pages.virtualKeys.confirm.deleteTitle", { name }),
    exact: true,
  });
  await expect(dialog).toBeVisible();
  await dialog
    .getByRole("button", { name: t("pages.virtualKeys.confirm.deleteConfirm"), exact: true })
    .click();

  await expect(dialog).toHaveCount(0);
  await expect(revoke).toHaveCount(0);
});
