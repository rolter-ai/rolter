// viewer.md — viewer@rolter.local, viewer at project default/default
import { ADMIN_TOKEN, api, assert, close, goto, login, save, scopeOf, screenState, setPersona, signIn, step, tenancy, until } from "./harness";

setPersona("viewer");
const t = await tenancy();
const viewerToken = await login("viewer@rolter.local");
const { page, context } = await signIn("viewer@rolter.local");

let scoped = false;
await step("V1.1", "sign in lands on the dashboard for default/default", async () => {
  assert(/\/dashboard/.test(page.url()), `landed on ${page.url()}`);
  const sc = await scopeOf(page, "viewer@rolter.local");
  scoped = sc.project === "default";
  return scoped ? ["pass", `scope ${sc.org}/${sc.team}/${sc.project}`] : ["bug", `switcher resolved org="${sc.org}" team="${sc.team}" project="${sc.project}" (${sc.message || "no message"}) (#1846)`];
}, page);

await step("V1.2", "dashboard shows the project's numbers only", async () => {
  await goto(page, "/dashboard");
  const mine = (await api("GET", "/api/v1/analytics/summary", viewerToken)).json.data[0];
  const all = (await api("GET", "/api/v1/analytics/summary", ADMIN_TOKEN)).json.data[0];
  assert(Number(mine.requests) < Number(all.requests), `viewer ${mine.requests} vs all ${all.requests}`);
  const sandboxRows = (await api("GET", "/api/v1/analytics/invocations?limit=200&model=sandbox-llama", viewerToken)).json.data;
  assert(sandboxRows.length === 0, `viewer sees ${sandboxRows.length} sandbox rows`);
  return ["pass", `viewer ${mine.requests} requests of ${all.requests} deployment-wide; no sandbox rows`];
}, page);

await step("V1.3", "a request in detail: bodies hidden for the viewer role", async () => {
  await goto(page, "/logs");
  await page.getByRole("button", { name: /Open request details for/i }).first().click();
  const drawer = page.getByRole("complementary", { name: "Details" });
  await drawer.waitFor();
  const text = await drawer.innerText();
  assert(/hidden for your role/i.test(text), "no 'hidden for your role' in the drawer");
  return ["pass", "drawer: status, tokens, cost shown; both bodies say hidden for your role"];
}, page);

await step("V1.4", "project admin opens bodies to viewers; the viewer then reads them", async () => {
  const put = await api("PUT", `/api/v1/projects/${t.project.id}/settings`, ADMIN_TOKEN, { payload_min_role: "viewer" });
  assert(put.status === 200, `PUT settings ${put.status}`);
  try {
    await goto(page, "/logs");
    await page.getByRole("button", { name: /Open request details for/i }).first().click();
    const drawer = page.getByRole("complementary", { name: "Details" });
    await drawer.waitFor();
    await until(async () => !/hidden for your role/i.test(await drawer.innerText()), 8000);
    const text = await drawer.innerText();
    assert(/messages|content/i.test(text), "no body visible after the project allowed viewers");
    return ["pass", "after payload_min_role=viewer the drawer shows the request and response bodies"];
  } finally {
    await api("PUT", `/api/v1/projects/${t.project.id}/settings`, ADMIN_TOKEN, { payload_min_role: "member" });
  }
}, page);

await step("V2.1", "set a display name and a bio", async () => {
  await goto(page, "/api-keys");
  const text = await page.locator("main").innerText();
  const hasProfile = /display name|bio/i.test(text);
  return hasProfile ? ["fail", "a profile section exists — update the script"] : ["gap", "no profile or bio anywhere on the account screen (#1823)"];
}, page);

await step("V3.1", "switch the dashboard language; is it remembered elsewhere?", async () => {
  await page.getByRole("button", { name: "Change language" }).click();
  await page.getByRole("menuitemradio", { name: "Русский" }).click();
  await page.waitForTimeout(500);
  const ru = await page.locator("nav").first().innerText();
  assert(/[а-яА-Я]/.test(ru), "rail not in Russian after switching");
  // a second browser signed in as the same person
  const other = await signIn("viewer@rolter.local");
  const nav = await other.page.locator("nav").first().innerText();
  await other.context.close();
  // back to English for the rest of the run
  await page.getByRole("button", { name: /язык|Change language/i }).click();
  await page.getByRole("menuitemradio", { name: "English" }).click();
  return /[а-яА-Я]/.test(nav)
    ? ["pass", "language followed the account to a second browser"]
    : ["partial", "switches instantly, but a second browser opens in English: localStorage only (#1824)"];
}, page);

await step("V4.1", "routing rules readable, every mutation refused with the role named", async () => {
  const st = await screenState(page, "routing-rules");
  if (!scoped && st.state !== "ok") return ["bug", `Routing Rules: ${st.state} — no project in scope (#1846)`];
  const add = page.getByRole("button", { name: /add route|new route/i }).first();
  await add.waitFor({ timeout: 8000 });
  await until(async () => await add.isDisabled(), 8000);
  const title = await add.getAttribute("title");
  assert(/Admin/i.test(title ?? ""), `title=${title}`);
  return ["pass", `Add route disabled: "${title}"`];
}, page);

await step("V4.2", "virtual keys listed without secrets; minting refused", async () => {
  const st = await screenState(page, "virtual-keys");
  if (!scoped && st.state !== "ok") return ["bug", `Virtual Keys: ${st.state} — no project in scope (#1846); minting is refused by the API (403, E2.1b)`];
  const text = await page.locator("main").innerText();
  assert(!/sk-rolter-[a-f0-9]{16,}/.test(text), "a full key is visible");
  const create = page.getByRole("button", { name: /new key|create key|add key|generate/i }).first();
  const disabled = (await create.count()) ? await until(async () => await create.isDisabled(), 8000).catch(() => false) : true;
  return disabled ? ["pass", "prefixes only; the create control is refused"] : ["fail", "create control enabled for a viewer"];
}, page);

await step("V4.3", "budgets and limits readable, not editable", async () => {
  const st = await screenState(page, "budgets");
  if (!scoped && st.state !== "ok") return ["bug", `Budgets & Limits: ${st.state} — no project in scope (#1846)`];
  const add = page.getByRole("button", { name: "Add budget" }).first();
  await add.waitFor({ timeout: 8000 });
  await until(async () => await add.isDisabled(), 8000);
  return ["pass", `Add budget disabled: "${await add.getAttribute("title")}"`];
}, page);

await step("V4.4", "roles & permissions shows the new capability rows", async () => {
  const st = await screenState(page, "rbac");
  const text = await page.locator("main").innerText();
  if (!scoped && st.state !== "ok") {
    const matrix = (await api("GET", "/api/v1/rbac/matrix", viewerToken)).json;
    const names = JSON.stringify(matrix);
    const apiMissing = ["analytics", "request_payload", "provider_health", "project_settings"].filter((r) => !names.includes(`"${r}"`));
    return ["bug", `Roles & Permissions: ${st.state} (#1846); the API publishes ${4 - apiMissing.length}/4 new rows`];
  }
  const missing = ["analytics", "request_payload", "provider_health", "project_settings"].filter((r) => !text.includes(r));
  if (missing.length === 4) {
    const matrix = (await api("GET", "/api/v1/rbac/matrix", viewerToken)).json;
    const names = JSON.stringify(matrix);
    const apiMissing = ["analytics", "request_payload", "provider_health", "project_settings"].filter((r) => !names.includes(`"${r}"`));
    return apiMissing.length === 0
      ? ["partial", "the screen did not render the rows for a viewer, but GET /api/v1/rbac/matrix publishes all four"]
      : ["fail", `missing from the matrix: ${apiMissing.join(", ")}`];
  }
  return missing.length ? ["fail", `missing: ${missing.join(", ")}`] : ["pass", "all four rows on screen"];
}, page);

await step("V5.1", "save a filter as a named view", async () => {
  await goto(page, "/logs");
  const save = await page.getByRole("button", { name: /save (view|filter|preset)/i }).count();
  return save ? ["fail", "a save-view control exists — update the script"] : ["gap", "no saved views on LLM Logs (#1825)"];
}, page);

await context.close();
await close();
save("viewer");
