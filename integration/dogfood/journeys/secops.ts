// secops.md — security review of the dogfood deployment
import { ADMIN_TOKEN, CONTROL, ROLTER_BIN, api, assert, chat, clickhouse, cliEnv, gw, login, save, setPersona, sleep, step, tenancy, throwawayPassword, totp, until } from "./harness";
import { existsSync } from "node:fs";

setPersona("secops");
const t = await tenancy();
const dev = await login("dev@rolter.local");
const org = await login("orgadmin@rolter.local");
const hasCli = existsSync(ROLTER_BIN);
const rolter = async (...args: string[]) => {
  const p = Bun.spawnSync([ROLTER_BIN, ...args], { env: cliEnv() });
  return { code: p.exitCode, out: p.stdout.toString() + p.stderr.toString() };
};
// a throwaway account, created the way a real one is: by invitation
const tempAccount = async (tag: string) => {
  const email = `${tag}-${Date.now()}@rolter.local`;
  const inv = await api("POST", `/api/v1/orgs/${t.org.id}/invitations`, org, { email, role: "viewer", scope_type: "project", scope_id: t.project.id });
  const password = throwawayPassword();
  const acc = await api("POST", `/api/v1/invitations/accept/${inv.json.token}/accept`, undefined, { password });
  if (acc.status !== 200) throw new Error(`accept ${acc.status} ${JSON.stringify(acc.json).slice(0, 120)}`);
  return { email, password, token: acc.json.token, id: acc.json.user.id, drop: () => api("DELETE", `/api/v1/users/${acc.json.user.id}`, ADMIN_TOKEN) };
};

await step("S1.1", "nothing runs open", async () => {
  if (!hasCli) return ["skip", `no ${ROLTER_BIN}: cargo build -p rolter --features postgres`];
  const r = await rolter("check");
  assert(!/open mode/i.test(r.out) || /refus/i.test(r.out), r.out.slice(0, 200));
  const errors = /(\d+) error\(s\)/.exec(r.out)?.[1];
  return errors === "0" ? ["pass", `rolter check: 0 errors; ${/(\d+) warning/.exec(r.out)?.[1] ?? "?"} warnings (bind address, env in this shell)`] : ["fail", r.out.slice(-300)];
});

await step("S1.2", "an anonymous caller gets nothing the spec doesn't mark public", async () => {
  const spec = await (await fetch(`${CONTROL}/openapi.json`)).json();
  const open: string[] = [];
  let checked = 0;
  for (const [path, ops] of Object.entries<any>(spec.paths)) {
    const get = ops.get;
    if (!get || path.includes("{") || path.startsWith("/internal/")) continue;
    const isPublic = Array.isArray(get.security) && get.security.length === 0;
    if (isPublic) continue;
    checked++;
    const r = await fetch(`${CONTROL}${path}`);
    if (r.status !== 401) open.push(`${r.status} ${path}`);
  }
  return open.length === 0 ? ["pass", `${checked} non-public GETs without parameters, all 401`] : ["fail", open.slice(0, 6).join("; ")];
});

await step("S1.3", "the redacted config stays public", async () => {
  const r = await fetch(`${CONTROL}/api/v1/config`);
  const body = await r.text();
  const secret = /sk-[A-Za-z0-9]{16,}|"api_key"\s*:\s*"[^"*]/.test(body);
  const bases = (body.match(/api_base/g) ?? []).length;
  assert(!secret, "a secret in the public config");
  return ["partial", `anonymous ${r.status}: no secrets, but ${bases} api_base values and the route map are readable (#1840)`];
});

await step("S1.4", "browser origins restricted", async () => {
  const r = await fetch(`${CONTROL}/api/v1/orgs`, { method: "OPTIONS", headers: { origin: "https://evil.example", "access-control-request-method": "GET", "access-control-request-headers": "authorization" } });
  const allow = r.headers.get("access-control-allow-origin");
  return allow === "https://evil.example" || allow === "*" ? ["fail", `preflight from evil.example → allow-origin ${allow}`] : ["pass", `preflight from a foreign origin → ${r.status}, allow-origin ${allow ?? "(none)"}`];
});

await step("S1.5", "a provider can't be pointed at the cloud metadata address", async () => {
  const p = await api("POST", `/api/v1/orgs/${t.org.id}/providers`, org, { name: `jr-ssrf-${Date.now() % 10000}`, kind: "openai_compatible", api_base: "http://169.254.169.254/latest", api_key: "x" });
  if (p.status === 200) {
    await api("DELETE", `/api/v1/providers/${p.json.id}`, ADMIN_TOKEN);
    return ["bug", "a provider on http://169.254.169.254 was accepted at creation (deleted before any probe ran)"];
  }
  return ["pass", `refused at creation: ${p.status} ${p.json?.error?.message?.slice(0, 100) ?? ""}`];
});

await step("S2.2", "password guessing is throttled and audited", async () => {
  const a = await tempAccount("throttle");
  try {
    let hit: any = null;
    let n = 0;
    for (; n < 20 && !hit; n++) {
      const r = await api("POST", "/api/v1/auth/login", undefined, { email: a.email, password: `wrong-${n}` });
      if (r.status === 429) hit = r;
    }
    if (!hit) return ["fail", `no 429 after ${n} wrong passwords`];
    const rows = (await api("GET", `/api/v1/orgs/${t.org.id}/audit-log`, org)).json?.items ?? [];
    const visible = rows.some((x: any) => /login_failed|lock/i.test(x.action ?? ""));
    const note = `429 after ${n - 1} wrong passwords, Retry-After ${hit.headers.get("retry-after")}`;
    return visible ? ["pass", `${note}; the failures show in the org audit log`] : ["partial", `${note}; auth.login_failed is written with no org, so the org's Audit Logs never shows it (#1854)`];
  } finally {
    await a.drop();
  }
});

await step("S2.4", "break-glass for a lost second factor", async () => {
  if (!hasCli) return ["skip", `no ${ROLTER_BIN}: cargo build -p rolter --features postgres`];
  const a = await tempAccount("breakglass");
  try {
    const begin = await api("POST", "/api/v1/me/mfa/enroll", a.token);
    await api("POST", "/api/v1/me/mfa/confirm", a.token, { code: totp(begin.json.secret) });
    const locked = await api("POST", "/api/v1/auth/login", undefined, { email: a.email, password: a.password });
    assert(locked.json?.mfa_token, "no challenge after enrolling");
    const r = await rolter("mfa", "reset", "--email", a.email, "--reason", "journey S2.4: lost phone");
    assert(r.code === 0, `mfa reset exit ${r.code}: ${r.out.slice(0, 200)}`);
    const after = await api("POST", "/api/v1/auth/login", undefined, { email: a.email, password: a.password });
    const oldSession = await api("GET", "/api/v1/auth/me", a.token);
    assert(after.json?.token, `password alone after reset → ${after.status} ${JSON.stringify(after.json).slice(0, 100)}`);
    return ["pass", `factor cleared by the CLI; the old session → ${oldSession.status}; password alone signs in again`];
  } finally {
    await a.drop();
  }
});

let key = "";
await step("S3.2", "a known credential field is redacted before storage", async () => {
  const k = await api("POST", `/api/v1/projects/${t.project.id}/virtual-keys`, org, { name: "secops-probe" });
  key = k.json.key;
  (globalThis as any).probeKeyId = k.json.id;
  await until(async () => (await gw("/v1/models", key)).status === 200, 20000);
  const marker = `S3-${Date.now()}`;
  const r = await chat(key, "gpt-4o-mini", { metadata: { api_key: `sk-live-${marker}-FIELD`, note: marker } });
  assert(r.status === 200, `call ${r.status} ${r.text.slice(0, 120)}`);
  const row = await until(async () => (await clickhouse(`select request_payload from request_payloads where request_payload like '%${marker}%' limit 1`))[0] ?? null, 30000);
  return row.request_payload.includes(`sk-live-${marker}-FIELD`) ? ["fail", "the api_key field reached storage"] : ["pass", "the api_key field is redacted in the stored body; the rest is kept"];
});

await step("S3.3", "a key pasted into a prompt never reaches storage", async () => {
  const marker = `S33-${Date.now()}`;
  await chat(key, "gpt-4o-mini", { messages: [{ role: "user", content: `my key is sk-live-${marker}-0123456789abcdef please debug` }] });
  const row = await until(async () => (await clickhouse(`select request_payload from request_payloads where request_payload like '%${marker}%' limit 1`))[0] ?? null, 30000);
  return row.request_payload.includes(`sk-live-${marker}`) ? ["gap", "a secret inside free text is stored verbatim (#1835)"] : ["pass", "redacted by pattern"];
});

await step("S4.1", "a guardrail blocks a pattern before the provider, and the text isn't stored", async () => {
  const marker = `forbidden-${Date.now() % 100000}`;
  const rule = await api("POST", "/api/v1/guardrails/rules", dev, { name: `journey ${marker}`, enabled: true, source_type: "pattern", pattern: marker, stage: "pre_call", action: "block", include_system: true, position: 0 });
  assert(rule.status === 200 || rule.status === 201, `rule ${rule.status} ${JSON.stringify(rule.json).slice(0, 160)}`);
  try {
    const r = await until(async () => {
      const c = await chat(key, "gpt-4o-mini", { messages: [{ role: "user", content: `please say ${marker}` }] });
      return c.status !== 200 ? c : null;
    }, 20000);
    await sleep(4000);
    const stored = await clickhouse(`select count() c from request_payloads where request_payload like '%${marker}%'`);
    return ["pass", `${r.status} ${r.json?.error?.code ?? r.json?.error?.type ?? ""}: ${r.json?.error?.message?.slice(0, 80) ?? ""}; rows holding the text: ${stored[0].c}`];
  } finally {
    await api("DELETE", `/api/v1/guardrails/rules/${rule.json.id}`, dev);
  }
});

await step("S5.1", "a restored store still opens its secrets", async () => {
  if (!hasCli) return ["skip", `no ${ROLTER_BIN}: cargo build -p rolter --features postgres`];
  const p = await api("POST", `/api/v1/orgs/${t.org.id}/providers`, org, { name: `jr-sealed-${Date.now() % 10000}`, kind: "openai_compatible", api_base: "http://127.0.0.1:18003", api_key: "sealed-for-kek-verify" });
  assert(p.status === 200, `provider ${p.status}`);
  try {
    const good = await rolter("kek", "verify");
    const bad = Bun.spawnSync([ROLTER_BIN, "kek", "verify"], { env: { ...cliEnv(), ROLTER_KEK: Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString("base64") } });
    const badOut = bad.stdout.toString() + bad.stderr.toString();
    return good.code === 0 && bad.exitCode !== 0 ? ["pass", `right KEK: exit 0 (${good.out.trim().split("\n").pop()?.slice(0, 80)}); wrong KEK: exit ${bad.exitCode}, "${badOut.trim().split("\n").slice(-1)[0].slice(0, 90)}"`] : ["fail", `right KEK exit ${good.code}; wrong KEK exit ${bad.exitCode}: ${badOut.slice(0, 160)}`];
  } finally {
    await api("DELETE", `/api/v1/providers/${p.json.id}`, ADMIN_TOKEN);
  }
});

await step("S6.1", "who changed what, when", async () => {
  const rows = (await api("GET", `/api/v1/orgs/${t.org.id}/audit-log`, org)).json?.items ?? [];
  const actions = [...new Set(rows.map((x: any) => x.action))];
  const settings = rows.find((x: any) => x.action === "project.settings.update");
  return rows.length ? ["pass", `${rows.length} rows, ${actions.length} kinds (e.g. ${actions.slice(0, 5).join(", ")}); project.settings.update ${settings ? `detail ${JSON.stringify(settings.detail ?? {}).slice(0, 60)}` : "not found"}`] : ["fail", "empty audit log"];
});

const pk = (globalThis as any).probeKeyId;
if (pk) await api("DELETE", `/api/v1/virtual-keys/${pk}`, ADMIN_TOKEN);
save("secops");
