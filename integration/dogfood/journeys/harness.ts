// Headless driver for the user-journey scripts (docs/dev-docs/product/). Each
// persona script signs in through the real login form in Chromium for the
// dashboard steps, calls the API as the persona for the rest, and records one
// result per step. `run.ts` runs them all; see the README beside this file.
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";

export const REPO = resolve(import.meta.dir, "../../..");
const DOGFOOD = `${REPO}/integration/dogfood`;
export const OUT = process.env.JOURNEYS_OUT ?? `${DOGFOOD}/.journeys`;
export const CONTROL = process.env.ROLTER_CONTROL_URL ?? "http://127.0.0.1:4001";
export const GATEWAY = process.env.ROLTER_GATEWAY_URL ?? "http://127.0.0.1:4000";
export const INTERNAL = process.env.ROLTER_INTERNAL_URL ?? "http://127.0.0.1:4002";
export const CH = process.env.CLICKHOUSE_URL ?? "http://127.0.0.1:8123";
export const ROLTER_BIN = process.env.ROLTER_BIN ?? `${REPO}/target/debug/rolter`;
const PG_CONTAINER = process.env.ROLTER_PG_CONTAINER ?? "rolter-postgres-1";
mkdirSync(`${OUT}/shots`, { recursive: true });

// the dashboard's own playwright-core, so the runner needs no install of its own
const { chromium } = await import(`${REPO}/ui/node_modules/playwright-core/index.mjs`);
type Browser = any;
type BrowserContext = any;
type Page = any;

function envFile(path: string): Record<string, string> {
  const out: Record<string, string> = {};
  if (!existsSync(path)) return out;
  for (const line of readFileSync(path, "utf8").split("\n")) {
    const m = /^([A-Z_]+)=(.*)$/.exec(line.trim());
    if (m) out[m[1]] = m[2].replace(/^"|"$/g, "");
  }
  return out;
}
export const creds = { ...envFile(`${DOGFOOD}/creds.env`), ...envFile(`${DOGFOOD}/.tokens.env`) };
export const PASSWORD = creds.DEV_PASSWORD;
export const ADMIN_TOKEN = creds.ROLTER_ADMIN_TOKEN;
export const INTERNAL_TOKEN = creds.ROLTER_INTERNAL_TOKEN;
export const DATABASE_URL = `postgres://${creds.DEV_PG_USER}:${creds.DEV_PG_PASSWORD}@127.0.0.1:5432/rolter`;
/** the environment the host-side `rolter` CLI needs: the store and the KEK */
export function cliEnv(): Record<string, string> {
  const kek = existsSync(`${DOGFOOD}/.kek`) ? readFileSync(`${DOGFOOD}/.kek`, "utf8").trim() : "";
  return { ...(process.env as Record<string, string>), ...creds, ROLTER_DATABASE_URL: DATABASE_URL, ROLTER_KEK: kek, RUST_BACKTRACE: "0" };
}
export const dogfoodKey = () => readFileSync(`${DOGFOOD}/.virtual-key`, "utf8").trim();
/** a fresh password for an account a step creates and deletes again; never a literal */
export const throwawayPassword = () => `Jr-${crypto.randomUUID()}`;

export type Status = "pass" | "fail" | "gap" | "bug" | "partial" | "skip";
export interface Result {
  persona: string;
  step: string;
  name: string;
  status: Status;
  note: string;
  shot?: string;
}
export const results: Result[] = [];
let persona = "?";
export function setPersona(p: string) {
  persona = p;
}
export function record(step: string, name: string, status: Status, note = "", shot?: string) {
  results.push({ persona, step, name, status, note, shot });
  const mark = { pass: "PASS", fail: "FAIL", gap: "GAP ", bug: "BUG ", partial: "PART", skip: "SKIP" }[status];
  console.log(`${mark} ${persona}/${step} ${name}${note ? " — " + note : ""}`);
}
export function save(name: string) {
  const path = `${OUT}/results-${name}.json`;
  writeFileSync(path, JSON.stringify(results, null, 2));
  console.log(`\nwrote ${results.length} results to ${path}`);
}

/** run one step: a thrown error is a FAIL with its message, never a crash */
export async function step(id: string, name: string, fn: () => Promise<Status | [Status, string] | void>, page?: Page) {
  let shot: string | undefined;
  try {
    const out = await fn();
    if (page) shot = await snap(page, id);
    if (Array.isArray(out)) record(id, name, out[0], out[1], shot);
    else record(id, name, out ?? "pass", "", shot);
  } catch (err) {
    if (page) shot = await snap(page, id).catch(() => undefined);
    record(id, name, "fail", String((err as Error).message ?? err).split("\n")[0].slice(0, 300), shot);
  }
}

export async function snap(page: Page, id: string): Promise<string> {
  const path = `${OUT}/shots/${persona}-${id.replace(/[^a-zA-Z0-9.-]/g, "_")}.png`;
  await page.screenshot({ path, fullPage: false });
  return path;
}

let browser: Browser | null = null;
export async function launch(): Promise<Browser> {
  // CHROMIUM_PATH points at a system Chromium; unset, playwright's own is used
  browser ??= await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined, headless: true });
  return browser;
}
export async function close() {
  await browser?.close();
  browser = null;
}

export interface Session {
  context: BrowserContext;
  page: Page;
  token: string;
}

/** sign in through the real login form, the way a person does */
export async function signIn(email: string, password = PASSWORD): Promise<Session> {
  const b = await launch();
  const context = await b.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  await page.goto(`${CONTROL}/`);
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await page.waitForURL(/\/(dashboard|playground|logs|api-keys)/, { timeout: 15000 });
  await page.waitForLoadState("networkidle").catch(() => {});
  const token = await page.evaluate(() => {
    for (const key of Object.keys(localStorage)) {
      if (/token/i.test(key)) return localStorage.getItem(key) ?? "";
    }
    return "";
  });
  return { context, page, token };
}

export async function goto(page: Page, path: string) {
  await page.goto(`${CONTROL}${path}`);
  await page.waitForLoadState("networkidle").catch(() => {});
  await page.waitForTimeout(400);
}

export async function api(method: string, path: string, bearer?: string, body?: unknown) {
  const res = await fetch(`${CONTROL}${path}`, {
    method,
    headers: {
      "content-type": "application/json",
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json: any = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = text;
  }
  return { status: res.status, json, headers: res.headers };
}

export async function login(email: string, password = PASSWORD): Promise<string> {
  const r = await api("POST", "/api/v1/auth/login", undefined, { email, password });
  if (r.status !== 200 || !r.json?.token) throw new Error(`login ${email} -> ${r.status} ${JSON.stringify(r.json).slice(0, 120)}`);
  return r.json.token;
}

export async function gw(path: string, key: string | null, body?: unknown, headers: Record<string, string> = {}) {
  const t0 = performance.now();
  const res = await fetch(`${GATEWAY}${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      "content-type": "application/json",
      ...(key ? { authorization: `Bearer ${key}` } : {}),
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json: any = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    json = text;
  }
  return { status: res.status, json, text, headers: res.headers, ms: performance.now() - t0 };
}

export async function chat(key: string, model = "gpt-4o", extra: Record<string, unknown> = {}, headers: Record<string, string> = {}) {
  return gw("/v1/chat/completions", key, { model, max_tokens: 16, messages: [{ role: "user", content: "hello" }], ...extra }, headers);
}

export async function clickhouse(sql: string): Promise<any[]> {
  const res = await fetch(`${CH}/?default_format=JSON`, { method: "POST", body: sql });
  if (!res.ok) throw new Error(`clickhouse ${res.status}: ${(await res.text()).slice(0, 200)}`);
  return ((await res.json()) as any).data;
}

export async function psql(sql: string): Promise<string> {
  const proc = Bun.spawnSync(["docker", "exec", PG_CONTAINER, "psql", "-U", creds.DEV_PG_USER ?? "rolter", "-d", "rolter", "-tAc", sql]);
  if (proc.exitCode !== 0) throw new Error(`psql: ${proc.stderr.toString().slice(0, 200)}`);
  return proc.stdout.toString().trim();
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function until<T>(fn: () => Promise<T | null | undefined | false>, ms = 20000, every = 500): Promise<T> {
  const end = Date.now() + ms;
  let last: unknown;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v as T;
    } catch (e) {
      last = e;
    }
    await sleep(every);
  }
  throw new Error(`timed out${last ? ": " + String(last) : ""}`);
}

export function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(msg);
}

/** the dogfood tenancy, resolved once */
export async function tenancy() {
  const orgs = (await api("GET", "/api/v1/orgs", ADMIN_TOKEN)).json;
  const org = orgs.find((o: any) => o.slug === "default");
  const projects = (await api("GET", `/api/v1/orgs/${org.id}/projects`, ADMIN_TOKEN)).json;
  const teams = (await api("GET", `/api/v1/orgs/${org.id}/teams`, ADMIN_TOKEN)).json;
  return {
    org,
    teams,
    projects,
    team: teams.find((t: any) => t.name === "default"),
    project: projects.find((p: any) => p.name === "default" && p.team_name === "default"),
    sandbox: projects.find((p: any) => p.name === "sandbox"),
  };
}


/** what the scope switcher (inside the account menu) resolved to, plus its message if any */
export async function scopeOf(page: Page, email: string) {
  const trigger = page.getByRole("button", { name: new RegExp(email.replace(/[.@]/g, (c) => "\\" + c)) }).first();
  await trigger.click();
  await page.getByRole("combobox", { name: "Org", exact: true }).first().waitFor({ timeout: 8000 }).catch(() => {});
  const read = async (name: string) => {
    const box = page.getByRole("combobox", { name, exact: true }).first();
    if (!(await box.count())) return "";
    return (await box.inputValue().catch(() => "")) || "";
  };
  const org = await read("Org");
  const team = await read("Team");
  const project = await read("Project");
  const body = await page.locator("body").innerText().catch(() => "");
  const message = /failed to load (orgs|teams|projects)|no (org|team|project) configured[^\n]*/i.exec(body)?.[0] ?? "";
  const footer = (await trigger.innerText().catch(() => "")).replace(/\s+/g, " ");
  await page.keyboard.press("Escape");
  return { org, team, project, message, footer };
}

export type ScreenState = "ok" | "denied" | "error" | "no-scope" | "hidden";

/** classify one screen as the signed-in persona sees it */
export async function screenState(page: Page, key: string): Promise<{ state: ScreenState; text: string }> {
  await goto(page, `/${key}`);
  await page.waitForTimeout(600);
  const main = (await page.locator("main").innerText().catch(() => "")).replace(/\s+/g, " ");
  let state: ScreenState = "ok";
  if (/You do not have access to/i.test(main)) state = "denied";
  else if (/Could not load/i.test(main)) state = "error";
  else if (/Select a project|Pick a project/i.test(main)) state = "no-scope";
  return { state, text: main.slice(0, 160) };
}

/** the rail labels this persona is shown */
export async function railLabels(page: Page): Promise<string[]> {
  return await page.locator("nav button").evaluateAll((els) => els.map((e) => (e.textContent ?? "").trim()).filter(Boolean));
}

export const LEAVES = "playground dashboard logs mcp-logs connectors logs-settings model-catalog providers provider-groups budgets routing-rules complexity-router circuit-breaker pricing-overrides model-settings mcp-catalog mcp-library tool-groups auth-sessions oauth-grants mcp-settings plugins alerting-channels alerting-rules alerting-history virtual-keys gov-users gov-teams business-units customers user-provisioning sso rbac access-profiles audit-logs guardrail-rules guardrail-providers cluster adaptive-dashboard adaptive-settings prompt-repo skills-repo client-settings compatibility effective-config security api-keys performance feature-flags".split(" ");

/** RFC 6238 TOTP (SHA-1, 30 s, 6 digits) from a base32 secret */
export function totp(secretB32: string, at = Date.now()): string {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let bits = "";
  for (const c of secretB32.replace(/=+$/, "").toUpperCase()) bits += alphabet.indexOf(c).toString(2).padStart(5, "0");
  const bytes = new Uint8Array(Math.floor(bits.length / 8));
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(bits.slice(i * 8, i * 8 + 8), 2);
  const counter = Math.floor(at / 1000 / 30);
  const msg = new Uint8Array(8);
  new DataView(msg.buffer).setBigUint64(0, BigInt(counter));
  const h = new Bun.CryptoHasher("sha1", bytes).update(msg).digest() as Uint8Array;
  const o = h[h.length - 1] & 0xf;
  const n = ((h[o] & 0x7f) << 24) | (h[o + 1] << 16) | (h[o + 2] << 8) | h[o + 3];
  return String(n % 1_000_000).padStart(6, "0");
}

/** a route in `projectId` served by vllm-a100-01 at a punitive price, so one call spends real money */
export async function pricedRoute(orgId: string, projectId: string, token = ADMIN_TOKEN) {
  const model = `jr-priced-${Date.now() % 1_000_000}`;
  const providers = (await api("GET", `/api/v1/orgs/${orgId}/providers`, ADMIN_TOKEN)).json;
  const a100 = providers.find((p: any) => p.name === "vllm-a100-01");
  const r = await api("POST", `/api/v1/projects/${projectId}/routes`, token, { model });
  if (r.status !== 200) throw new Error(`priced route ${r.status} ${JSON.stringify(r.json).slice(0, 120)}`);
  await api("POST", `/api/v1/routes/${r.json.id}/targets`, token, { provider_id: a100.id, upstream_model: "meta-llama/Llama-3.1-8B-Instruct" });
  await api("PUT", "/api/v1/model-prices", ADMIN_TOKEN, { model, input_per_mtok: "20000", output_per_mtok: "20000" });
  return {
    model,
    drop: async () => {
      await api("DELETE", `/api/v1/routes/${r.json.id}`, ADMIN_TOKEN);
      await api("DELETE", `/api/v1/model-prices/${model}`, ADMIN_TOKEN);
    },
  };
}
