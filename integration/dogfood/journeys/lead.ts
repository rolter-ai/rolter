// team-lead.md — lead@rolter.local, admin at team default
import { ADMIN_TOKEN, api, assert, chat, close, gw, login, save, scopeOf, screenState, setPersona, signIn, sleep, step, tenancy, throwawayPassword, until } from "./harness";

setPersona("lead");
const t = await tenancy();
const email = "lead@rolter.local";
const token = await login(email);
const { page, context } = await signIn(email);
const cleanup: (() => Promise<unknown>)[] = [];

await step("T1.1", "sign in lands scoped to team default", async () => {
  const s = await scopeOf(page, email);
  if (s.team !== "default") return ["bug", `switcher resolved org="${s.org}" team="${s.team}" project="${s.project}" (${s.message || "no message"}) (#1846)`];
  return ["pass", `scope ${s.org}/${s.team}/${s.project}`];
}, page);

await step("T1.2", "the team's traffic and bodies, nothing of other teams'", async () => {
  const rows = (await api("GET", "/api/v1/analytics/invocations?limit=200", token)).json.data;
  const foreign = rows.filter((r: any) => r.team_id && r.team_id !== t.team.id);
  const withBodies = rows.filter((r: any) => r.request_payload).length;
  assert(rows.length > 0, "no rows");
  assert(foreign.length === 0, `${foreign.length} rows of other teams`);
  const s = await screenState(page, "logs");
  return ["pass", `${rows.length} rows, all team default, ${withBodies} with bodies; LLM Logs screen: ${s.state}`];
}, page);

await step("T1.3", "Model Catalog and Routing Rules show the team's routes", async () => {
  const api200 = (await api("GET", `/api/v1/projects/${t.project.id}/routes`, token)).status;
  const cat = await screenState(page, "model-catalog");
  const rr = await screenState(page, "routing-rules");
  if (rr.state !== "ok") return ["bug", `API routes → ${api200}; Model Catalog ${cat.state}; Routing Rules ${rr.state} — no project in scope (#1846)`];
  return ["pass", `catalog ${cat.state}, routing rules ${rr.state}`];
}, page);

let projectId = "";
await step("T2.1", "create a project for a new workstream", async () => {
  const r = await api("POST", `/api/v1/teams/${t.team.id}/projects`, token, { name: "lead-workstream" });
  assert(r.status === 200, `create project ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  projectId = r.json.id;
  cleanup.push(() => api("DELETE", `/api/v1/projects/${projectId}`, ADMIN_TOKEN));
  return ["pass", "POST /api/v1/teams/{id}/projects → 200 as team admin (the switcher's + is out of reach: #1846)"];
});

let inviteeToken = "";
let inviteeId = "";
await step("T2.2", "invite an engineer as a member of the new project", async () => {
  const email2 = `newhire-${Date.now()}@rolter.local`;
  const r = await api("POST", `/api/v1/orgs/${t.org.id}/invitations`, token, { email: email2, role: "member", scope_type: "project", scope_id: projectId });
  assert(r.status === 200, `invite ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  const link = r.json.token ?? r.json.invitation?.token;
  assert(link, `no token in ${JSON.stringify(r.json).slice(0, 160)}`);
  const acc = await api("POST", `/api/v1/invitations/accept/${link}/accept`, undefined, { password: throwawayPassword() });
  assert(acc.status === 200 && acc.json?.token, `accept ${acc.status} ${JSON.stringify(acc.json).slice(0, 160)}`);
  inviteeToken = acc.json.token;
  inviteeId = acc.json.user.id;
  cleanup.push(() => api("DELETE", `/api/v1/users/${inviteeId}`, ADMIN_TOKEN));
  return ["pass", "invitation at project scope by a team admin; accepted → signed in as a member"];
});

await step("T2.3", "get the link to the person", async () => ["partial", "the API returns the token once; rolter sends no e-mail (#1828)"]);

let routeModel = `lead-team-llama-${Date.now() % 100000}`;
let sharedKey = "";
await step("T3.1", "route a public model name to an existing provider", async () => {
  const providers = (await api("GET", `/api/v1/orgs/${t.org.id}/providers`, ADMIN_TOKEN)).json;
  const a100 = providers.find((p: any) => p.name === "vllm-a100-01");
  const r = await api("POST", `/api/v1/projects/${projectId}/routes`, token, { model: routeModel });
  assert(r.status === 200, `route ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  const tg = await api("POST", `/api/v1/routes/${r.json.id}/targets`, token, { provider_id: a100.id, upstream_model: "meta-llama/Llama-3.1-8B-Instruct" });
  assert(tg.status === 200, `target ${tg.status} ${JSON.stringify(tg.json).slice(0, 160)}`);
  return ["pass", `route ${routeModel} → vllm-a100-01, created by the team admin`];
});

await step("T3.2", "adding a provider is refused, naming the org admin role", async () => {
  const r = await api("POST", `/api/v1/orgs/${t.org.id}/providers`, token, { name: "lead-own-openai", kind: "openai", api_base: "https://api.openai.com/v1" });
  assert(r.status === 403, `add provider → ${r.status}`);
  return ["pass", `403 "${r.json?.error?.message ?? ""}"`];
});

await step("T3.3", "mint a shared key for the team's service and call the new route", async () => {
  const r = await api("POST", `/api/v1/projects/${projectId}/virtual-keys`, token, { name: "team-service" });
  assert(r.status === 200, `mint ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  sharedKey = r.json.key;
  const c = await until(async () => {
    const x = await chat(sharedKey, routeModel);
    return x.status === 200 ? x : null;
  }, 30000);
  return ["pass", `shared key → ${routeModel}: ${c.status} via ${c.headers.get("x-rolter-provider")}`];
});

await step("T4.2", "a rate limit on the project answers 429 with Retry-After", async () => {
  const other = await api("POST", `/api/v1/projects/${projectId}/virtual-keys`, token, { name: "team-service-2" });
  const key2 = other.json.key;
  const rl = await api("POST", "/api/v1/rate-limits", token, { scope_type: "project", scope_id: projectId, rpm: 3 });
  assert(rl.status === 200, `rate limit ${rl.status} ${JSON.stringify(rl.json).slice(0, 160)}`);
  const dropLimit = () => api("DELETE", `/api/v1/rate-limits/${rl.json.id}`, ADMIN_TOKEN);
  await sleep(7000);
  let hit: any = null;
  const seen: number[] = [];
  for (let i = 0; i < 8 && !hit; i++) {
    const c = await gw("/v1/models", key2);
    const x = await chat(key2, "gpt-4o-mini");
    seen.push(x.status);
    if (x.status === 429) hit = x;
  }
  await dropLimit();
  if (!hit) return ["fail", `no 429 in ${seen.length} calls (${seen.join(",")})`];
  return ["pass", `429 after ${seen.length - 1} calls, Retry-After: ${hit.headers.get("retry-after")}`];
});

await step("T4.1", "a spend cap on the project answers 402 once reached", async () => {
  // price the test model (deployment-wide prices are a superadmin's, so the admin token sets it)
  const price = await api("PUT", "/api/v1/model-prices", ADMIN_TOKEN, { model: routeModel, input_per_mtok: "20000", output_per_mtok: "20000" });
  assert(price.status === 200, `price ${price.status} ${JSON.stringify(price.json).slice(0, 120)}`);
  cleanup.push(() => api("DELETE", `/api/v1/model-prices/${routeModel}`, ADMIN_TOKEN));
  const b = await api("POST", "/api/v1/budgets", token, { scope_type: "project", scope_id: projectId, limit_usd: "0.50", period: "monthly" });
  assert(b.status === 200, `budget ${b.status} ${JSON.stringify(b.json).slice(0, 160)}`);
  (globalThis as any).budgetId = b.json.id;
  await sleep(7000);
  const statuses: number[] = [];
  let body = "";
  for (let i = 0; i < 40; i++) {
    const c = await chat(sharedKey, routeModel);
    statuses.push(c.status);
    if (c.status === 402) { body = c.text; break; }
  }
  const got402 = statuses.includes(402);
  (globalThis as any).dropBudget = () => api("DELETE", `/api/v1/budgets/${b.json.id}`, ADMIN_TOKEN);
  return got402 ? ["pass", `402 after ${statuses.length - 1} calls: ${body.slice(0, 140)}`] : ["fail", `no 402 in ${statuses.length} calls (${[...new Set(statuses)].join(",")})`];
});

await step("T4.4", "raise the budget mid-month", async () => {
  const list = (await api("GET", "/api/v1/budgets", token)).json;
  const mine = (Array.isArray(list) ? list : list?.data ?? []).find((b: any) => b.scope_id === projectId);
  const r = await api("PUT", `/api/v1/budgets/${mine?.id}`, token, { limit_usd: "5.00" });
  await (globalThis as any).dropBudget?.();
  await sleep(6000);
  return r.status === 405 || r.status === 404 ? ["partial", `no edit: PUT → ${r.status}; delete and recreate`] : ["pass", `PUT → ${r.status}`];
});

await step("T5.1", "spend by model over the month, the team's only", async () => {
  const r = await api("GET", "/api/v1/analytics/by-model", token);
  assert(r.status === 200, `by-model ${r.status}`);
  const models = (r.json.data ?? []).map((x: any) => x.model);
  assert(!models.includes("sandbox-llama"), "sees research/sandbox's model");
  return ["pass", `${models.length} models, none of research/sandbox's; includes ${models.includes(routeModel) ? routeModel : "(new route not yet rolled up)"}`];
});

await step("T5.2", "provider health for the lead", async () => {
  const r = await api("GET", "/api/v1/health/uptime", token);
  return (r.json?.data?.length ?? 0) > 0 ? ["pass", `${r.json.data.length} rows`] : ["gap", `200 with 0 rows (#1833)`];
});

await step("T6.1", "remove the new hire's membership; they lose the project at once", async () => {
  const members = (await api("GET", `/api/v1/orgs/${t.org.id}/memberships`, token));
  const all = members.status === 200 ? members.json : (await api("GET", `/api/v1/orgs/${t.org.id}/memberships`, ADMIN_TOKEN)).json;
  const m = all.find((x: any) => x.user_id === inviteeId);
  assert(m, "membership not found");
  const personal = await api("POST", `/api/v1/me/projects/${projectId}/virtual-keys`, inviteeToken, { name: "newhire-laptop", expires_in_days: 7 });
  assert(personal.status === 200, `newhire mint ${personal.status}`);
  (globalThis as any).newhireKey = personal.json.key;
  await until(async () => (await gw("/v1/models", personal.json.key)).status === 200, 20000);
  const del = await api("DELETE", `/api/v1/memberships/${m.id}`, token);
  const listVia = members.status === 200 ? "the lead" : "the admin token (the lead cannot list org memberships)";
  assert(del.status === 200 || del.status === 204, `delete membership as the lead → ${del.status} ${JSON.stringify(del.json).slice(0, 120)}`);
  const after = await api("GET", `/api/v1/projects/${projectId}/routes`, inviteeToken);
  assert(after.status === 403, `newhire still reads routes: ${after.status}`);
  const note = `membership removed by the lead → the new hire's dashboard calls answer 403`;
  return members.status === 200 ? ["pass", note] : ["partial", `${note}; but the lead cannot list the team's memberships to find it — listed with the admin token (#1850)`];
});

await step("T6.2", "their personal key stops working", async () => {
  await sleep(7000);
  const r = await chat((globalThis as any).newhireKey, routeModel);
  return r.status === 401 || r.status === 403 ? ["pass", `refused: ${r.status}`] : ["bug", `the leaver's personal key still answers ${r.status} (#1841)`];
});

await step("T6.3", "the shared key keeps working", async () => {
  const r = await chat(sharedKey, "gpt-4o-mini");
  return r.status !== 401 ? ["pass", `shared key → ${r.status}`] : ["fail", `shared key → ${r.status}`];
});

for (const f of cleanup.reverse()) await f().catch(() => {});
await context.close();
await close();
save("lead");
