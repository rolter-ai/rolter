import { expect, test } from "@playwright/test";

// full CRUD journey for a provider: create -> edit -> delete, driven through the
// dashboard against the live control plane. the seeded scope (global-setup) pins
// the tenant so the create controls are enabled.

function uniqueName(): string {
  return `e2e-prov-${Math.random().toString(36).slice(2, 8)}`;
}

test("provider create → edit → delete", async ({ page }) => {
  const name = uniqueName();
  await page.goto("/providers");

  // create — the ProviderSheet opens with Name / Kind / API base fields
  await page.getByRole("button", { name: /add provider/i }).click();
  await page.getByPlaceholder("openai-primary").first().fill(name);
  await page.getByPlaceholder("https://api.openai.com/v1").fill("http://sim-a:8000");
  await page.getByRole("button", { name: /create provider/i }).click();

  // the new provider row appears (name shows in both a name and a slug cell)
  const row = page.locator("div.grid", { hasText: name });
  await expect(row.first()).toBeVisible();

  // edit — reopen the sheet for this provider and save a changed API base
  await row.first().getByRole("button", { name: /^edit$/i }).click();
  const apiBase = page.getByPlaceholder("https://api.openai.com/v1");
  await expect(apiBase).toBeVisible();
  await apiBase.fill("http://sim-b:8000");
  await page.getByRole("button", { name: /save provider/i }).click();
  await expect(page.getByRole("button", { name: /save provider/i })).toHaveCount(0);
  await expect(row.first()).toBeVisible();

  // delete — trash button opens a confirm dialog; confirm removes the row
  await row.first().getByRole("button", { name: /delete provider/i }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText(/delete provider/i)).toBeVisible();
  await dialog.getByRole("button", { name: /^delete$/i }).click();

  await expect(page.locator("div.grid", { hasText: name })).toHaveCount(0);
});
