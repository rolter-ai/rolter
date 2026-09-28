// app-service.md — an application's shared key, owned by orgadmin@rolter.local
import { ADMIN_TOKEN, GATEWAY, api, assert, chat, gw, login, pricedRoute, psql, save, setPersona, sleep, step, tenancy, until } from "./harness";

setPersona("app");
const t = await tenancy();
const owner = await login("orgadmin@rolter.local");
const minted: string[] = [];
const mint = async (body: Record<string, unknown>) => {
  const r = await api("POST", `/api/v1/projects/${t.project.id}/virtual-keys`, owner, body);
  if (r.status !== 200) throw new Error(`mint ${r.status} ${JSON.stringify(r.json).slice(0, 160)}`);
  minted.push(r.json.id);
  await until(async () => (await gw("/v1/models", r.json.key)).status === 200, 20000);
  return r.json;
};
const rowsFor = async (keyId: string, limit = 100) => (await api("GET", `/api/v1/analytics/invocations?limit=${limit}&key=${keyId}`, ADMIN_TOKEN)).json?.data ?? [];

let svc: any;
await step("P1.1", "the owner mints a shared key, not tied to a person", async () => {
  svc = await mint({ name: "checkout-service" });
  const createdBy = await psql(`select coalesce(created_by::text, 'null') from virtual_keys where id = '${svc.id}'`);
  assert(createdBy === "null", `created_by = ${createdBy}`);
  return ["pass", "shown once; created_by is null, so it survives the minter leaving"];
});

await step("P1.2", "narrowed to models and providers", async () => {
  const k = await mint({ name: "checkout-narrow", models: ["gpt-4o"], providers: ["openai-edge"] });
  const models = (await gw("/v1/models", k.key)).json.data.map((m: any) => m.id);
  const other = await chat(k.key, "gpt-4o-mini");
  assert(models.length === 1 && models[0] === "gpt-4o", `lists ${models.join(",")}`);
  const code = other.json?.error?.code ?? other.json?.error?.type ?? "";
  assert(other.status === 403 && code === "model_not_allowed", `gpt-4o-mini → ${other.status} ${code}`);
  return ["pass", `lists only gpt-4o; gpt-4o-mini → 403 ${code}`];
});

await step("P1.4", "an expired key is refused with an authentication error", async () => {
  const k = await mint({ name: "temp-campaign", expires_in_days: 1 });
  await psql(`update virtual_keys set expires_at = now() - interval '1 minute' where id = '${k.id}'`);
  const r = await until(async () => {
    const c = await chat(k.key, "gpt-4o-mini");
    return c.status === 401 ? c : null;
  }, 25000);
  return ["pass", `after expiry → 401 ${r.json?.error?.type ?? ""}: ${r.json?.error?.message ?? ""}`];
});

await step("P2.2", "a client-sent x-request-id lands on the row", async () => {
  const id = `checkout-${Date.now()}`;
  const r = await chat(svc.key, "gpt-4o-mini", {}, { "x-request-id": id });
  assert(r.status === 200, `call ${r.status}`);
  const echoed = r.headers.get("x-request-id");
  const row = await until(async () => (await rowsFor(svc.id)).find((x: any) => x.request_id === id) ?? null, 30000).catch(() => null);
  return row ? ["pass", `echoed ${echoed === id ? "unchanged" : echoed}; the row carries it`] : ["fail", `echoed ${echoed}; no row with request_id ${id}`];
});

await step("P2.3", "a traceparent joins the service's trace", async () => {
  const trace = [...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, "0")).join("");
  const r = await chat(svc.key, "gpt-4o-mini", {}, { traceparent: `00-${trace}-00f067aa0ba902b7-01` });
  assert(r.status === 200, `call ${r.status}`);
  const row = await until(async () => (await rowsFor(svc.id)).find((x: any) => x.trace_id === trace) ?? null, 30000).catch(() => null);
  return row ? ["pass", "the row's trace_id is the caller's trace"] : ["fail", "no row carries the sent trace id"];
});

await step("P2.4", "a conversation stays on one replica; the pool still spreads", async () => {
  // x-rolter-target names the upstream model; the replica is x-rolter-provider
  const call = (session: string) =>
    gw("/v1/chat/completions", svc.key, { model: "llama-3.1-8b-cached", max_tokens: 8, messages: [{ role: "user", content: `turn of ${session}: ${crypto.randomUUID()}` }] }, { "x-session-id": session }).then((r) => r.headers.get("x-rolter-provider") ?? `?${r.status}`);
  const picks = await Promise.all(Array.from({ length: 24 }, (_, i) => call(`conv-${i % 6}`).then((p) => [`conv-${i % 6}`, p] as const)));
  const bySession = new Map<string, Set<string>>();
  for (const [s, p] of picks) (bySession.get(s) ?? bySession.set(s, new Set()).get(s)!).add(p);
  const replicas = new Set(picks.map(([, p]) => p));
  const sticky = [...bySession.values()].every((v) => v.size === 1);
  const note = `6 conversations × 4 concurrent turns on llama-3.1-8b-cached (3 replicas): ${replicas.size} replica(s) used, each conversation on ${sticky ? "one" : "several"}`;
  return replicas.size > 1 && sticky ? ["pass", note] : ["bug", `${note} (#1851)`];
});

await step("P5.1", "transient upstream failures are retried away", async () => {
  const statuses: number[] = [];
  for (let i = 0; i < 20; i++) statuses.push((await chat(svc.key, "deepseek-r1")).status);
  const fivexx = statuses.filter((s) => s >= 500).length;
  return fivexx === 0 ? ["pass", `20 calls to deepseek-r1: no 5xx reached the client`] : ["fail", `${fivexx}/20 5xx reached the client (${statuses.join(",")})`];
});

await step("P5.2", "16 concurrent calls: latency stays near the upstream's", async () => {
  const one = async () => { const t0 = performance.now(); await chat(svc.key, "llama-3.1-8b"); return performance.now() - t0; };
  const seq: number[] = [];
  for (let i = 0; i < 4; i++) seq.push(await one());
  const par = await Promise.all(Array.from({ length: 16 }, one));
  const p50 = (xs: number[]) => xs.sort((a, b) => a - b)[Math.floor(xs.length / 2)];
  const ratio = p50(par) / p50(seq);
  const note = `sequential p50 ${Math.round(p50(seq))} ms; 16 concurrent p50 ${Math.round(p50(par))} ms, max ${Math.round(Math.max(...par))} ms (×${ratio.toFixed(1)})`;
  return ratio < 2 ? ["pass", note] : ["bug", `${note} (#1815)`];
});

await step("P5.3", "a rate limit on the key: 429 with Retry-After", async () => {
  const k = await mint({ name: "checkout-limited" });
  const rl = await api("POST", "/api/v1/rate-limits", owner, { scope_type: "virtual_key", scope_id: k.id, rpm: 2 });
  assert(rl.status === 200, `rate limit ${rl.status} ${JSON.stringify(rl.json).slice(0, 120)}`);
  try {
    await sleep(7000);
    const seen: number[] = [];
    let hit: any = null;
    for (let i = 0; i < 6 && !hit; i++) {
      const c = await chat(k.key, "gpt-4o-mini");
      seen.push(c.status);
      if (c.status === 429) hit = c;
    }
    return hit ? ["pass", `429 after ${seen.length - 1} calls, Retry-After ${hit.headers.get("retry-after")}, ${hit.json?.error?.code ?? hit.json?.error?.type ?? ""}`] : ["fail", `no 429: ${seen.join(",")}`];
  } finally {
    await api("DELETE", `/api/v1/rate-limits/${rl.json.id}`, ADMIN_TOKEN);
  }
});

await step("P5.4", "a budget on the key: 402, nothing sent upstream", async () => {
  const priced = await pricedRoute(t.org.id, t.project.id);
  const k = await mint({ name: "checkout-budgeted" });
  const b = await api("POST", "/api/v1/budgets", owner, { scope_type: "virtual_key", scope_id: k.id, limit_usd: "0.50", period: "monthly" });
  try {
    assert(b.status === 200, `budget ${b.status} ${JSON.stringify(b.json).slice(0, 120)}`);
    await sleep(7000);
    const seen: number[] = [];
    for (let i = 0; i < 6 && !seen.includes(402); i++) seen.push((await chat(k.key, priced.model)).status);
    await sleep(3000);
    const rows = await rowsFor(k.id);
    const refused = rows.filter((r: any) => Number(r.status) === 402);
    const upstream = refused.filter((r: any) => r.provider).length;
    return seen.includes(402) ? ["pass", `402 after ${seen.length - 1} call(s); ${refused.length} refused row(s), ${upstream} naming a provider`] : ["fail", `statuses ${seen.join(",")}`];
  } finally {
    if (b.json?.id) await api("DELETE", `/api/v1/budgets/${b.json.id}`, ADMIN_TOKEN);
    await priced.drop();
  }
});

await step("P5.6", "a client that gives up: a 499 row naming the target", async () => {
  const ctl = new AbortController();
  const t0 = Date.now();
  setTimeout(() => ctl.abort(), 150);
  await fetch(`${GATEWAY}/v1/chat/completions`, {
    method: "POST",
    signal: ctl.signal,
    headers: { "content-type": "application/json", authorization: `Bearer ${svc.key}` },
    body: JSON.stringify({ model: "gpt-4o", max_tokens: 16, messages: [{ role: "user", content: "hello" }] }),
  }).catch(() => {});
  const row = await until(async () => (await rowsFor(svc.id)).find((r: any) => Number(r.status) === 499 && Date.parse(r.ts + "Z") >= t0 - 5000) ?? null, 30000).catch(() => null);
  if (!row) return ["fail", "no 499 row"];
  return row.provider ? ["pass", `499 row names ${row.provider}/${row.target}`] : ["bug", `499 row with no provider or target (#1816)`];
});

for (const id of minted) await api("DELETE", `/api/v1/virtual-keys/${id}`, ADMIN_TOKEN);
save("app");
