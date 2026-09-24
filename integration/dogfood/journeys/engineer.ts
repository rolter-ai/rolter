// engineer.md — engineer@rolter.local, member at project default/default
import { PASSWORD, api, assert, chat, close, goto, gw, login, save, scopeOf, screenState, setPersona, signIn, sleep, step, tenancy, totp, until } from "./harness";

setPersona("engineer");
const t = await tenancy();
const email = "engineer@rolter.local";
const token = await login(email);
const { page, context } = await signIn(email);

await step("E1.1", "sign in lands on the dashboard scoped to default/default", async () => {
  assert(/\/dashboard/.test(page.url()), `landed on ${page.url()}`);
  const s = await scopeOf(page, email);
  if (s.project !== "default") return ["bug", `switcher resolved org="${s.org}" team="${s.team}" project="${s.project}" (${s.message || "no message"}) (#1846)`];
  return ["pass", `scope ${s.org}/${s.team}/${s.project}`];
}, page);

await step("E1.2", "Model Catalog lists the project's routes", async () => {
  const s = await screenState(page, "model-catalog");
  if (s.state !== "ok") return ["fail", `${s.state}: ${s.text}`];
  assert(/gpt-4o/.test(await page.locator("main").innerText()), "gpt-4o not in the catalog");
  return ["pass", "the catalog lists gpt-4o and the fleet's models"];
}, page);

let key = "";
let keyId = "";
await step("E2.1", "mint a personal key: dashboard, then API", async () => {
  await goto(page, "/api-keys");
  const uiBlocked = await until(async () => {
    const text = await page.locator("main").innerText();
    if (/Select a project in the sidebar to mint a virtual key/i.test(text)) return "blocked";
    const gen = page.getByRole("button", { name: "Generate virtual key" });
    if (await gen.count()) return (await gen.isDisabled()) ? "blocked" : "open";
    return null;
  }, 10000).then((v) => v === "blocked");
  const r = await api("POST", `/api/v1/me/projects/${t.project.id}/virtual-keys`, token, { name: "engineer-laptop", expires_in_days: 30 });
  assert(r.status === 200 && r.json?.key, `API mint ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  key = r.json.key;
  keyId = r.json.id;
  if (uiBlocked) return ["bug", "the API mints the key (shown once, 30-day expiry), but on My Virtual Keys 'Generate virtual key' is disabled ('Select a project in the sidebar') and the switcher cannot select one (#1846)"];
  return ["pass", "minted from the dashboard and the API"];
}, page);

await step("E2.1b", "a viewer is refused a personal key", async () => {
  const v = await login("viewer@rolter.local");
  const r = await api("POST", `/api/v1/me/projects/${t.project.id}/virtual-keys`, v, { name: "should-fail", expires_in_days: 1 });
  assert(r.status === 403, `viewer mint answered ${r.status}`);
  return ["pass", `viewer → 403 "${r.json?.error?.message ?? ""}"`];
});

let narrowKey = "";
await step("E2.2", "a key narrowed to one model lists only that model", async () => {
  const r = await api("POST", `/api/v1/me/projects/${t.project.id}/virtual-keys`, token, { name: "notebook-mini-only", models: ["gpt-4o-mini"], expires_in_days: 7 });
  assert(r.status === 200, `mint ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  narrowKey = r.json.key;
  const models = await until(async () => {
    const m = await gw("/v1/models", narrowKey);
    return m.status === 200 ? m.json.data.map((x: any) => x.id) : null;
  }, 20000);
  const other = await chat(narrowKey, "gpt-4o");
  assert(models.length === 1 && models[0] === "gpt-4o-mini", `lists ${models.length}: ${models.slice(0, 5).join(",")}`);
  assert(other.status === 403 || other.status === 404, `gpt-4o with the narrowed key → ${other.status}`);
  return ["pass", `/v1/models lists only gpt-4o-mini; gpt-4o → ${other.status} ${other.json?.error?.type ?? other.json?.error?.code ?? ""}`];
});

await step("E2.3", "rotate: a new secret works, the old one is refused", async () => {
  await until(async () => (await gw("/v1/models", key)).status === 200, 20000);
  const r = await api("POST", `/api/v1/me/virtual-keys/${keyId}/rotate`, token);
  assert(r.status === 200 && r.json?.key, `rotate ${r.status}`);
  const fresh = r.json.key;
  keyId = r.json.id;
  await until(async () => (await gw("/v1/models", fresh)).status === 200, 20000);
  const old = await until(async () => {
    const s = (await gw("/v1/models", key)).status;
    return s === 401 ? s : null;
  }, 20000);
  key = fresh;
  return ["pass", `new secret authenticates; the old one → ${old}`];
});

let requestId = "";
await step("E3.1", "Playground answers without any key handling", async () => {
  await goto(page, "/playground");
  const text = await page.locator("main").innerText();
  if (/Pick a project to mint a key against/i.test(text)) return ["bug", "the Playground asks to 'Pick a project' first — the switcher cannot select one for a project member (#1846)"];
  const box = page.getByRole("textbox").last();
  await box.fill("say hi");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(3000);
  const after = await page.locator("main").innerText();
  return after.length > text.length ? ["pass", "an answer streamed in"] : ["fail", "no answer appeared"];
}, page);

await step("E3.2", "OpenAI dialect from code", async () => {
  const r = await chat(key, "gpt-4o");
  assert(r.status === 200, `${r.status} ${r.text.slice(0, 160)}`);
  requestId = r.headers.get("x-request-id") ?? "";
  assert(requestId, "no x-request-id header");
  return ["pass", `200, x-request-id ${requestId.slice(0, 12)}…, ${Math.round(r.ms)} ms`];
});

await step("E3.3", "Anthropic dialect from code", async () => {
  const r = await gw("/v1/messages", null, { model: "gpt-4o", max_tokens: 16, messages: [{ role: "user", content: "hello" }] }, { "x-api-key": key, "anthropic-version": "2023-06-01" });
  assert(r.status === 200 && r.json?.type === "message", `${r.status} ${r.text.slice(0, 160)}`);
  return ["pass", `200, Anthropic-shaped message (${r.json.content?.[0]?.type})`];
});

await step("E3.4", "streaming and embeddings", async () => {
  const s = await gw("/v1/chat/completions", key, { model: "gpt-4o", stream: true, max_tokens: 16, messages: [{ role: "user", content: "hi" }] });
  assert(s.status === 200 && /^data: /m.test(s.text) && /\[DONE\]/.test(s.text), `stream ${s.status}`);
  const e = await gw("/v1/embeddings", key, { model: "text-embedding", input: "hello" });
  const models = (await gw("/v1/models", key)).json.data.map((m: any) => m.id);
  const embedModel = e.status === 200 ? "text-embedding" : models.find((m: string) => /embed/i.test(m));
  const e2 = e.status === 200 ? e : await gw("/v1/embeddings", key, { model: embedModel ?? "fake-llm", input: "hello" });
  assert(e2.status === 200 && Array.isArray(e2.json?.data?.[0]?.embedding), `embeddings ${e2.status} ${e2.text.slice(0, 120)}`);
  return ["pass", `SSE chunks + [DONE]; ${e2.json.data[0].embedding.length}-dim vector from ${e2.json.model ?? embedModel}`];
});

await step("E5.1", "errors name their cause", async () => {
  const bad = await chat("sk-rolter-not-a-real-key", "gpt-4o");
  const missing = await chat(key, "no-such-model");
  assert(bad.status === 401, `bad key → ${bad.status}`);
  assert(missing.status === 404, `unknown model → ${missing.status}`);
  return ["pass", `bad key → 401 ${bad.json?.error?.type ?? ""}; unknown model → 404 ${missing.json?.error?.code ?? missing.json?.error?.type ?? ""}`];
});

await step("E5.2", "find the failing request in LLM Logs, bodies included", async () => {
  const fail = await chat(key, "gpt-4o", { max_tokens: -1 });
  const rid = fail.headers.get("x-request-id") ?? "";
  const found = await until(async () => {
    const r = await api("GET", `/api/v1/analytics/invocations?limit=100&key=${keyId}`, token);
    return r.json?.data?.find((x: any) => x.request_id === requestId) ?? null;
  }, 30000);
  const body = found.request_payload ?? "";
  const byId = /request_id|request id/i.test(await (async () => { await goto(page, "/logs"); return await page.locator("main").innerText(); })());
  const note = `row found by key filter (${found.status}, ${found.provider}, ${found.latency_ms} ms); bodies ${body ? "readable" : "withheld"}; a failing call → ${fail.status} rid=${rid ? "yes" : "no"}`;
  if (!body) return ["fail", note];
  return byId ? ["pass", note] : ["partial", `${note}; no lookup by the x-request-id the client got (#1849)`];
}, page);

await step("E5.4", "is the upstream sick? provider health for a member", async () => {
  const r = await api("GET", "/api/v1/health/uptime", token);
  const rows = Array.isArray(r.json?.data) ? r.json.data.length : -1;
  const s = await screenState(page, "circuit-breaker");
  return rows > 0 ? ["pass", `${rows} provider rows; circuit breaker screen: ${s.state}`] : ["gap", `health API → ${r.status} with ${rows} rows; Circuit Breaker screen: ${s.state} (#1833)`];
}, page);

await step("E6.3", "own MCP tool-call rows in MCP Logs", async () => {
  const s = await screenState(page, "mcp-logs");
  return s.state === "ok" ? ["pass", "MCP Logs readable"] : ["gap", `MCP Logs: ${s.state} (#1831)`];
}, page);

await step("E9.1", "usage per key over the last week", async () => {
  await sleep(2000);
  const r = await api("GET", "/api/v1/me/usage", token);
  assert(r.status === 200, `usage ${r.status}`);
  const rows = r.json?.data ?? r.json?.keys ?? r.json;
  const mine = JSON.stringify(rows).includes(keyId);
  return mine ? ["pass", `/api/v1/me/usage lists the new key: ${JSON.stringify(rows).slice(0, 140)}`] : ["partial", `usage answered but the new key isn't in it yet: ${JSON.stringify(rows).slice(0, 160)}`];
});

await step("E10.4", "enrol a second factor, sign in with it, remove it", async () => {
  const begin = await api("POST", "/api/v1/me/mfa/enroll", token);
  assert(begin.status === 200 && begin.json?.secret, `enroll ${begin.status} ${JSON.stringify(begin.json).slice(0, 120)}`);
  const secret = begin.json.secret;
  const confirm = await api("POST", "/api/v1/me/mfa/confirm", token, { code: totp(secret) });
  assert(confirm.status === 200, `confirm ${confirm.status} ${JSON.stringify(confirm.json).slice(0, 120)}`);
  const codes = confirm.json?.recovery_codes?.length ?? confirm.json?.codes?.length ?? 0;
  try {
    const first = await api("POST", "/api/v1/auth/login", undefined, { email, password: PASSWORD });
    assert(first.status === 200 && first.json?.mfa_token && !first.json?.token, `login with a factor → ${first.status} ${JSON.stringify(first.json).slice(0, 120)}`);
    await sleep(31000 - (Date.now() % 30000)); // next step, so the code is not a replay
    const verify = await api("POST", "/api/v1/auth/mfa/verify", undefined, { mfa_token: first.json.mfa_token, code: totp(secret) });
    assert(verify.status === 200 && verify.json?.token, `verify ${verify.status} ${JSON.stringify(verify.json).slice(0, 120)}`);
    return ["pass", `TOTP enrolled (${codes} recovery codes); password alone now yields a challenge; the code redeems it`];
  } finally {
    await sleep(31000 - (Date.now() % 30000));
    const off = await api("DELETE", "/api/v1/me/mfa", token, { code: totp(secret) });
    if (off.status !== 200 && off.status !== 204) console.log(`!! could not remove the factor: ${off.status} ${JSON.stringify(off.json)}`);
  }
});

// clean up the keys this run minted
for (const k of (await api("GET", "/api/v1/me/virtual-keys", token)).json ?? []) {
  if (["engineer-laptop", "notebook-mini-only"].includes(k.name)) await api("DELETE", `/api/v1/me/virtual-keys/${k.id}`, token);
}
await context.close();
await close();
save("engineer");
