import { expect, test } from "@playwright/test";

import { t } from "../i18n";

// full CRUD journey for a provider: create -> edit -> delete, driven through the
// dashboard against the live control plane. the seeded scope (global-setup) pins
// the tenant so the create controls are enabled.
//
// fields are found by their label and buttons by their accessible name, read
// from the catalog: the API base placeholder follows the selected kind (#947),
// so it is not something to locate the field by (#1504)

function uniqueName(): string {
  return `e2e-prov-${Math.random().toString(36).slice(2, 8)}`;
}

test("provider create → edit → delete", async ({ page }) => {
  const name = uniqueName();
  await page.goto("/providers");

  // create. an org with no providers shows the header button and the empty
  // state's CTA together; the header one is present in every state
  await page
    .getByRole("button", { name: t("pages.providers.add"), exact: true })
    .first()
    .click();
  const sheet = page.getByRole("dialog");
  await sheet.getByLabel(t("providerSheet.fields.name"), { exact: true }).fill(name);
  await sheet
    .getByLabel(t("providerSheet.fields.apiBase"), { exact: true })
    .fill("http://sim-a:8000");
  await sheet.getByRole("button", { name: t("providerSheet.cta.create"), exact: true }).click();
  await expect(sheet).toHaveCount(0);

  // the new provider's row actions are named after it
  const edit = page.getByRole("button", {
    name: t("pages.providers.editOne", { name }),
    exact: true,
  });
  await expect(edit).toBeVisible();

  // edit — reopen the sheet for this provider and save a changed API base
  await edit.click();
  const apiBase = sheet.getByLabel(t("providerSheet.fields.apiBase"), { exact: true });
  await expect(apiBase).toHaveValue("http://sim-a:8000");
  await apiBase.fill("http://sim-b:8000");
  await sheet.getByRole("button", { name: t("providerSheet.cta.save"), exact: true }).click();
  await expect(sheet).toHaveCount(0);
  await expect(page.getByText("http://sim-b:8000")).toBeVisible();

  // delete — trash button opens a confirm dialog; confirm removes the row
  await page
    .getByRole("button", { name: t("pages.providers.deleteOne", { name }), exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(t("pages.providers.deleteTitle"), { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: t("common.delete"), exact: true }).click();

  await expect(edit).toHaveCount(0);
});
