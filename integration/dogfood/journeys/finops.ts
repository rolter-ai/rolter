// finops.md — finops@rolter.local, viewer at org default
import { ADMIN_TOKEN, api, assert, chat, close, goto, login, pricedRoute, save, scopeOf, screenState, setPersona, signIn, sleep, step, tenancy, until } from "./harness";

setPersona("finops");
const t = await tenancy();
const email = "finops@rolter.local";
const token = await login(email);
const admin = await login("orgadmin@rolter.local");
const { page, context } = await signIn(email);

await step("F1.1", "sign in: the org's Dashboard, mutations disabled with the role named", async () => {
  const s = await scopeOf(page, email);
  assert(s.org === "default" && s.project, `scope ${JSON.stringify(s)}`);
  await goto(page, "/budgets");
  const add = page.getByRole("button", { name: "Add budget" }).first();
  await add.waitFor({ timeout: 8000 });
  await until(async () => await add.isDisabled(), 8000);
  const title = (await add.getAttribute("title")) ?? "";
  return /admin/i.test(title) ? ["pass", `scope ${s.org}/${s.team}/${s.project}; Add budget disabled: "${title}"`] : ["partial", `disabled, but the title doesn't name the role: "${title}"`];
}, page);

await step("F1.2", "spend over time and by model: the org's traffic", async () => {
  const mine = (await api("GET", "/api/v1/analytics/summary", token)).json.data[0];
  const all = (await api("GET", "/api/v1/analytics/summary", ADMIN_TOKEN)).json.data[0];
  const ts = await api("GET", "/api/v1/analytics/timeseries?bucket=day", token);
  const bm = await api("GET", "/api/v1/analytics/by-model", token);
  const d = await screenState(page, "dashboard");
  assert(ts.status === 200 && bm.status === 200, `timeseries ${ts.status}, by-model ${bm.status}`);
  return ["pass", `${mine.requests} requests / $${Number(mine.cost_usd ?? 0).toFixed(4)} (deployment ${all.requests}); ${bm.json.data.length} models; Dashboard screen ${d.state}`];
}, page);

await step("F1.3", "a request without its prompt", async () => {
  const rows = (await api("GET", "/api/v1/analytics/invocations?limit=50", token)).json.data;
  const withheld = rows.filter((r: any) => Number(r.payload_withheld) === 1).length;
  const leaked = rows.filter((r: any) => r.request_payload || r.response_payload).length;
  assert(leaked === 0, `${leaked} rows with bodies`);
  return ["pass", `${rows.length} rows with tokens and cost; ${withheld} flagged payload_withheld, none with bodies`];
});

let bu: any = null;
let cust: any = null;
await step("F1.4", "spend by business unit and customer", async () => {
  bu = (await api("POST", `/api/v1/orgs/${t.org.id}/business-units`, admin, { name: `Research ${Date.now() % 10000}` })).json;
  cust = (await api("POST", `/api/v1/orgs/${t.org.id}/customers`, admin, { name: `Globex ${Date.now() % 10000}`, business_unit_id: bu.id })).json;
  const key = await api("POST", `/api/v1/projects/${t.project.id}/virtual-keys`, admin, { name: "globex-service" });
  const set = await api("PUT", `/api/v1/virtual-keys/${key.json.id}/attribution`, admin, { business_unit_id: bu.id, customer_id: cust.id });
  assert(set.status === 200, `attribution ${set.status} ${JSON.stringify(set.json).slice(0, 120)}`);
  (globalThis as any).attrKey = key.json;
  await until(async () => (await chat(key.json.key, "gpt-4o-mini")).status === 200, 20000);
  for (let i = 0; i < 3; i++) await chat(key.json.key, "gpt-4o-mini");
  const row = await until(async () => {
    const r = await api("GET", "/api/v1/analytics/by-attribution?dimension=customer", token);
    return (r.json?.data ?? []).find((x: any) => x.customer_id === cust.id || x.id === cust.id) ?? null;
  }, 30000).catch(() => null);
  const s = await screenState(page, "customers");
  return row ? ["pass", `by-attribution rolls up the customer: ${JSON.stringify(row).slice(0, 120)}; Customers screen ${s.state}`] : ["fail", `no by-attribution row for the new customer; Customers screen ${s.state}`];
}, page);

await step("F2.1", "traffic that counted as free is named", async () => {
  const rows = (await api("GET", "/api/v1/analytics/invocations?limit=200", token)).json.data;
  const unpriced = [...new Set(rows.filter((r: any) => Number(r.unpriced) === 1).map((r: any) => r.model))];
  return unpriced.length ? ["pass", `${unpriced.length} unpriced models in the last 200 rows: ${unpriced.slice(0, 6).join(", ")}…`] : ["fail", "no row flagged unpriced"];
});

await step("F2.2", "pricing is a superadmin's", async () => {
  const r = await api("PUT", "/api/v1/model-prices", admin, { model: "gpt-4o", input_per_mtok: "1", output_per_mtok: "1" });
  const f = await api("PUT", "/api/v1/model-prices", token, { model: "gpt-4o", input_per_mtok: "1", output_per_mtok: "1" });
  assert(r.status === 403 && f.status === 403, `org admin ${r.status}, finops ${f.status}`);
  return ["pass", "org admin → 403, finops → 403: the price catalog is deployment-wide"];
});

await step("F3.1", "a monthly cap per business unit answers 402 for the unit's keys", async () => {
  const priced = await pricedRoute(t.org.id, t.project.id);
  try {
    const b = await api("POST", "/api/v1/budgets", admin, { scope_type: "business_unit", scope_id: bu.id, limit_usd: "0.50", period: "monthly" });
    assert(b.status === 200, `budget ${b.status} ${JSON.stringify(b.json).slice(0, 160)}`);
    try {
      await sleep(7000);
      const k = (globalThis as any).attrKey.key;
      const seen: number[] = [];
      let body = "";
      for (let i = 0; i < 10; i++) {
        const c = await chat(k, priced.model);
        seen.push(c.status);
        if (c.status === 402) { body = c.text; break; }
      }
      return seen.includes(402) ? ["pass", `402 after ${seen.length - 1} priced call(s): ${body.slice(0, 130)}`] : ["fail", `statuses ${seen.join(",")}`];
    } finally {
      await api("DELETE", `/api/v1/budgets/${b.json.id}`, ADMIN_TOKEN);
    }
  } finally {
    await priced.drop();
  }
});

await step("F3.1b", "finops can't set the cap themselves", async () => {
  const r = await api("POST", "/api/v1/budgets", token, { scope_type: "org", scope_id: t.org.id, limit_usd: "1", period: "monthly" });
  assert(r.status === 403, `finops budget → ${r.status}`);
  return ["pass", "403: budgets need admin at the org"];
});

await step("F4.2", "an alert on spend velocity", async () => {
  const r = await api("POST", "/api/v1/alert-rules", admin, { name: "spend jump", kind: "spend_velocity" });
  const s = await screenState(page, "alerting-rules");
  return ["gap", `org admin → ${r.status}; Alerting → Rules for finops: ${s.state} (#1829)`];
}, page);

await step("F5.2", "month-end numbers by script", async () => {
  const r = await api("GET", "/api/v1/analytics/by-attribution?dimension=business_unit&since=2026-09-01T00:00:00Z", token);
  assert(r.status === 200, `by-attribution ${r.status}`);
  return ["partial", `the API answers (${(r.json?.data ?? []).length} units) with a session; no CSV export (#1838)`];
});

// cleanup
const k = (globalThis as any).attrKey;
if (k) await api("DELETE", `/api/v1/virtual-keys/${k.id}`, ADMIN_TOKEN);
if (cust?.id) await api("DELETE", `/api/v1/customers/${cust.id}`, ADMIN_TOKEN);
if (bu?.id) await api("DELETE", `/api/v1/business-units/${bu.id}`, ADMIN_TOKEN);
await context.close();
await close();
save("finops");
