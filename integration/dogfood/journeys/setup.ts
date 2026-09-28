// Cross-project traffic for the isolation steps: a route and a key in
// research/sandbox, and fresh traffic in both projects. Idempotent.
import { ADMIN_TOKEN, OUT, api, chat, dogfoodKey, sleep, tenancy, until, gw } from "./harness";
import { writeFileSync, existsSync, readFileSync } from "node:fs";

const t = await tenancy();
const providers = (await api("GET", `/api/v1/orgs/${t.org.id}/providers`, ADMIN_TOKEN)).json;
const a100 = providers.find((p: any) => p.name === "vllm-a100-01");

let routes = (await api("GET", `/api/v1/projects/${t.sandbox.id}/routes`, ADMIN_TOKEN)).json;
let route = routes.find((r: any) => r.model === "sandbox-llama");
if (!route) {
  const created = await api("POST", `/api/v1/projects/${t.sandbox.id}/routes`, ADMIN_TOKEN, { model: "sandbox-llama" });
  if (created.status !== 200) throw new Error(`route: ${created.status} ${JSON.stringify(created.json)}`);
  route = created.json;
  const target = await api("POST", `/api/v1/routes/${route.id}/targets`, ADMIN_TOKEN, {
    provider_id: a100.id,
    upstream_model: "meta-llama/Llama-3.1-8B-Instruct",
  });
  if (target.status !== 200) throw new Error(`target: ${target.status} ${JSON.stringify(target.json)}`);
}

const keyFile = `${OUT}/sandbox.key`;
let key = existsSync(keyFile) ? readFileSync(keyFile, "utf8").trim() : "";
if (!key) {
  const minted = await api("POST", `/api/v1/projects/${t.sandbox.id}/virtual-keys`, ADMIN_TOKEN, { name: "sandbox-traffic" });
  if (minted.status !== 200) throw new Error(`key: ${minted.status} ${JSON.stringify(minted.json)}`);
  key = minted.json.key;
  writeFileSync(keyFile, key);
}

await until(async () => (await gw("/v1/models", key)).status === 200, 20000);
const models = (await gw("/v1/models", key)).json?.data?.map((m: any) => m.id) ?? [];
console.log("sandbox key sees models:", models.length, models.slice(0, 6));
let ok = 0;
for (let i = 0; i < 5; i++) {
  const r = await chat(key, "sandbox-llama");
  if (r.status === 200) ok++;
  else console.log("sandbox call", r.status, JSON.stringify(r.json).slice(0, 160));
}
console.log(`sandbox traffic: ${ok}/5 ok`);

const defaultKey = dogfoodKey();
let d = 0;
for (let i = 0; i < 5; i++) if ((await chat(defaultKey, "gpt-4o-mini")).status === 200) d++;
console.log(`default-project traffic: ${d}/5 ok`);
await sleep(3000);
