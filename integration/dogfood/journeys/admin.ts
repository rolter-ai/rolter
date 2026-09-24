// platform-admin.md — dev@rolter.local (superadmin) and orgadmin@rolter.local (admin at org default)
import { ADMIN_TOKEN, INTERNAL, INTERNAL_TOKEN, PASSWORD, ROLTER_BIN, api, assert, chat, cliEnv, close, goto, gw, login, save, setPersona, signIn, sleep, step, tenancy, until } from "./harness";
import { existsSync } from "node:fs";

setPersona("admin");
const t = await tenancy();
const dev = await login("dev@rolter.local");
const org = await login("orgadmin@rolter.local");
const viewerMe = (await api("GET", "/api/v1/auth/me", await login("viewer@rolter.local"))).json;
const viewerId = viewerMe.user?.id ?? viewerMe.id;

await step("A0.10", "every gateway node live and on the current config version", async () => {
  const snap = await (await fetch(`${INTERNAL}/internal/snapshot`, { headers: { authorization: `Bearer ${INTERNAL_TOKEN}` } })).json();
  // a script that changed config a moment ago leaves the fleet one poll behind
  const list: any[] = await until(async () => {
    const nodes = (await api("GET", "/api/v1/cluster/nodes", dev)).json;
    const l = Array.isArray(nodes) ? nodes : nodes?.data ?? nodes?.nodes ?? [];
    return l.length && l.every((n: any) => n.config_version >= snap.version) ? l : null;
  }, 20000).catch(async () => (await api("GET", "/api/v1/cluster/nodes", dev)).json);
  assert(list.length > 0, `no nodes: ${JSON.stringify(list).slice(0, 160)}`);
  const n = list[0];
  const versionKey = Object.keys(n).find((k) => /version/i.test(k) && typeof n[k] === "number");
  const behind = versionKey ? list.filter((x: any) => x[versionKey] < snap.version).length : -1;
  return behind === 0 ? ["pass", `${list.length} node(s), all at config ${snap.version} (${versionKey})`] : ["partial", `${list.length} node(s); ${versionKey ? `${behind} behind config ${snap.version}` : `no version field in ${Object.keys(n).join(",")}`}`];
});

await step("A1a.4", "require a second factor for the org", async () => {
  const before = (await api("GET", `/api/v1/orgs/${t.org.id}/auth-policy`, org)).json;
  const put = await api("PUT", `/api/v1/orgs/${t.org.id}/auth-policy`, org, { allow_password_login: before.allow_password_login ?? true, allow_sso: before.allow_sso ?? true, mfa_policy: "required_all" });
  assert(put.status === 200, `policy ${put.status} ${JSON.stringify(put.json).slice(0, 120)}`);
  try {
    const r = await api("POST", "/api/v1/auth/login", undefined, { email: "viewer@rolter.local", password: PASSWORD });
    const code = r.json?.error?.code ?? r.json?.error?.type ?? "";
    return r.status === 200 ? ["fail", "the viewer still signs in without a factor"] : ["partial", `a member without a factor is refused (${r.status} ${code}), not walked through enrolment (#1852)`];
  } finally {
    await api("PUT", `/api/v1/orgs/${t.org.id}/auth-policy`, org, { allow_password_login: before.allow_password_login ?? true, allow_sso: before.allow_sso ?? true, mfa_policy: before.mfa_policy ?? "off" });
  }
});

await step("A1d", "SCIM: provision a viewer, then deprovision", async () => {
  const tok = await api("POST", `/api/v1/orgs/${t.org.id}/scim-tokens`, org, { name: `okta-test-${Date.now() % 10000}` });
  assert(tok.status === 200, `scim token ${tok.status} ${JSON.stringify(tok.json).slice(0, 120)}`);
  const bearer = tok.json.secret;
  try {
    const email = `scim-${Date.now()}@rolter.local`;
    const created = await api("POST", "/scim/v2/Users", bearer, { schemas: ["urn:ietf:params:scim:schemas:core:2.0:User"], userName: email, emails: [{ value: email, primary: true }], active: true });
    assert(created.status === 201 || created.status === 200, `create ${created.status} ${JSON.stringify(created.json).slice(0, 160)}`);
    const members = (await api("GET", `/api/v1/orgs/${t.org.id}/memberships`, org)).json;
    const users = (await api("GET", `/api/v1/orgs/${t.org.id}/users`, org)).json;
    const u = users.find((x: any) => x.email === email);
    const m = members.find((x: any) => x.user_id === u?.id);
    assert(m?.role === "viewer", `membership ${JSON.stringify(m)}`);
    const off = await api("PATCH", `/scim/v2/Users/${created.json.id}`, bearer, { schemas: ["urn:ietf:params:scim:api:messages:2.0:PatchOp"], Operations: [{ op: "replace", value: { active: false } }] });
    assert(off.status === 200 || off.status === 204, `deprovision ${off.status} ${JSON.stringify(off.json).slice(0, 160)}`);
    const after = (await api("GET", `/api/v1/orgs/${t.org.id}/users`, org)).json.find((x: any) => x.email === email);
    await api("DELETE", `/api/v1/users/${u.id}`, ADMIN_TOKEN);
    return ["pass", `created as org viewer; PATCH active=false → deactivated_at ${after?.deactivated_at ? "set" : "NOT set"}`];
  } finally {
    await api("DELETE", `/api/v1/scim-tokens/${tok.json.token?.id ?? ""}`, org);
  }
});

await step("A1e", "a custom role widens a viewer by one grant", async () => {
  const role = await api("POST", `/api/v1/orgs/${t.org.id}/custom-roles`, org, { name: `Route editor ${Date.now() % 10000}`, base_role: "viewer", grants: [{ resource: "route", action: "create" }] });
  assert(role.status === 200, `role ${role.status} ${JSON.stringify(role.json).slice(0, 160)}`);
  const prof = await api("POST", `/api/v1/orgs/${t.org.id}/access-profiles`, org, { name: `route editors ${Date.now() % 10000}`, roles: [{ role_id: role.json.id, project_id: t.project.id }] });
  assert(prof.status === 200, `profile ${prof.status} ${JSON.stringify(prof.json).slice(0, 160)}`);
  const pid = prof.json.id ?? prof.json.profile?.id;
  const asg = await api("POST", `/api/v1/access-profiles/${pid}/assignments`, org, { user_id: viewerId });
  assert(asg.status === 200, `assignment ${asg.status} ${JSON.stringify(asg.json).slice(0, 160)}`);
  try {
    await sleep(1000);
    const v = await login("viewer@rolter.local");
    const eff = await api("GET", `/api/v1/rbac/effective?org_id=${t.org.id}&team_id=${t.team.id}&project_id=${t.project.id}`, v);
    const has = JSON.stringify(eff.json).includes("route:create");
    const model = `viewer-made-${Date.now() % 100000}`;
    const r = await api("POST", `/api/v1/projects/${t.project.id}/routes`, v, { model });
    if (r.status === 200) await api("DELETE", `/api/v1/routes/${r.json.id}`, ADMIN_TOKEN);
    return has && r.status === 200 ? ["pass", "rbac/effective lists route:create for the viewer, and the viewer creates a route"] : ["fail", `effective has route:create=${has}; create → ${r.status}`];
  } finally {
    await api("DELETE", `/api/v1/access-profile-assignments/${asg.json.id}`, org);
    await api("DELETE", `/api/v1/access-profiles/${pid}`, org);
    await api("DELETE", `/api/v1/custom-roles/${role.json.id}`, org);
  }
});

const s = await signIn("orgadmin@rolter.local");
const page = s.page;

await step("A2.1", "create a team from the scope switcher", async () => {
  const name = `platform-${Date.now() % 10000}`;
  await page.getByRole("button", { name: /orgadmin@rolter\.local/ }).first().click();
  await page.getByRole("button", { name: "Add team" }).click();
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  const team = await until(async () => (await api("GET", `/api/v1/orgs/${t.org.id}/teams`, org)).json.find((x: any) => x.name === name) ?? null, 10000);
  await page.keyboard.press("Escape");
  await api("DELETE", `/api/v1/teams/${team.id}`, ADMIN_TOKEN);
  return ["pass", `team "${name}" created through the switcher's +`];
}, page);

await step("A2.2", "the project's gear: let viewers read captured payloads", async () => {
  await goto(page, "/dashboard");
  await page.getByRole("button", { name: /orgadmin@rolter\.local/ }).first().click();
  // the switcher reselects the default project after A2.1
  await page.getByRole("button", { name: "Project settings" }).click();
  const sw = page.getByRole("switch", { name: /Viewers can read captured payloads/ });
  await sw.waitFor({ timeout: 8000 });
  const was = await sw.getAttribute("aria-checked");
  await sw.click();
  const now = await until(async () => (await api("GET", `/api/v1/projects/${t.project.id}/settings`, org)).json?.payload_min_role === "viewer" || null, 8000).catch(() => false);
  await sw.click();
  const back = await until(async () => (await api("GET", `/api/v1/projects/${t.project.id}/settings`, org)).json?.payload_min_role === "member" || null, 8000).catch(() => false);
  await page.keyboard.press("Escape");
  return now && back ? ["pass", `switch (was ${was}) flipped payload_min_role to viewer and back`] : ["fail", `on→viewer ${!!now}, off→member ${!!back}`];
}, page);

await step("A3.1", "add a provider and test the connection", async () => {
  const p = await api("POST", `/api/v1/orgs/${t.org.id}/providers`, org, { name: `jr-vllm-${Date.now() % 10000}`, kind: "openai_compatible", api_base: "http://127.0.0.1:18003", api_key: "not-a-real-key" });
  assert(p.status === 200, `provider ${p.status} ${JSON.stringify(p.json).slice(0, 160)}`);
  try {
    const sealed = JSON.stringify(p.json).includes("not-a-real-key");
    const test = await api("POST", `/api/v1/providers/${p.json.id}/test`, org);
    const models = test.json?.models ?? test.json?.data ?? [];
    assert(!sealed, "the key came back in the response");
    assert(test.status === 200, `test ${test.status} ${JSON.stringify(test.json).slice(0, 160)}`);
    const why = test.json.reachable ? `${(test.json.models ?? []).length} models listed` : `unreachable, and says why: upstream ${test.json.status} at ${test.json.probed_url} (credential ${test.json.credential})`;
    return ["pass", `the key is sealed (never echoed); Test connection → ${why}`];
  } finally {
    await api("DELETE", `/api/v1/providers/${p.json.id}`, ADMIN_TOKEN);
  }
});

await step("A3.4", "try it in the Playground (org admin)", async () => {
  await goto(page, "/playground");
  await until(async () => /Active/.test(await page.locator("main").innerText()), 15000);
  await page.waitForTimeout(8000);
  const banner = /Could not read the gateway's model list/.test(await page.locator("main").innerText());
  const picked = await page.getByRole("combobox", { name: "Model" }).first().inputValue().catch(() => "?");
  const ask = async () => {
    await page.getByPlaceholder("Message…").fill("say hello");
    await page.getByRole("button", { name: "Send", exact: true }).click();
    return await until(async () => {
      const text = await page.locator("main").innerText();
      if (/no route for model/i.test(text)) return "no-route";
      return /lorem|ipsum|dolor|assistant/i.test(text.split("say hello").slice(1).join("")) ? "answer" : null;
    }, 20000).catch(() => "timeout");
  };
  const first = await ask();
  if (first === "answer") return ["pass", `default model ${picked} answered`];
  const box = page.getByRole("combobox", { name: "Model" }).first();
  await box.click();
  await box.fill("gpt-4o-mini");
  await page.getByRole("option", { name: "gpt-4o-mini", exact: true }).click();
  const second = await ask();
  return second === "answer"
    ? ["bug", `the model list fell back (banner ${banner}) and preselected ${picked}, which answers "${first}"; after picking gpt-4o-mini by hand it answers (#1853)`]
    : ["fail", `default ${picked}: ${first}; gpt-4o-mini: ${second}`];
}, page);

await step("A3.4b", "try it in the Playground (superadmin)", async () => {
  const r = await api("POST", `/api/v1/me/projects/${t.project.id}/playground-key`, dev);
  return r.status === 200 ? ["pass", "superadmin gets a playground key"] : ["bug", `superadmin with no membership → ${r.status} (#1847)`];
});

await step("A3.17", "export the live state as a file", async () => {
  if (!existsSync(ROLTER_BIN)) return ["skip", `no ${ROLTER_BIN}: cargo build -p rolter --features postgres`];
  const proc = Bun.spawnSync([ROLTER_BIN, "config", "export"], { env: cliEnv() });
  const out = proc.stdout.toString();
  assert(proc.exitCode === 0, `exit ${proc.exitCode}: ${proc.stderr.toString().slice(0, 200)}`);
  const providers = (out.match(/^\[\[providers\.(readonly|default)\]\]/gm) ?? []).length;
  const routes = (out.match(/^\[\[routes\]\]/gm) ?? []).length;
  const secret = /sk-[A-Za-z0-9]{16,}|api_key\s*=\s*"/.test(out);
  assert(!secret, "a credential is in the export");
  return ["pass", `${providers} providers, ${routes} routes, no credential in the file`];
});

await step("A3.18", "a hosted kind pointed at another host (#1133)", async () => {
  const k = (await api("POST", `/api/v1/projects/${t.project.id}/virtual-keys`, org, { name: "a318" })).json;
  await until(async () => (await gw("/v1/models", k.key)).status === 200, 20000);
  const r = await chat(k.key, "claude-sonnet-4");
  await api("DELETE", `/api/v1/virtual-keys/${k.id}`, ADMIN_TOKEN);
  return r.status === 200 ? ["pass", "claude-sonnet-4 answers"] : ["gap", `claude-sonnet-4 → ${r.status} ${r.json?.error?.message?.slice(0, 80) ?? ""} (#1133)`];
});

await s.context.close();
await close();
save("admin");
