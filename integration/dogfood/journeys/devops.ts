// devops.md — operating the dogfood deployment (dev@rolter.local, superadmin)
import { CONTROL, GATEWAY, api, assert, close, goto, login, save, setPersona, signIn, sleep, step, until } from "./harness";

setPersona("devops");
const dev = await login("dev@rolter.local");

await step("D2.1", "liveness and readiness on both planes", async () => {
  const out: string[] = [];
  for (const [plane, base] of [["gateway", GATEWAY], ["control", CONTROL]]) {
    for (const p of ["/healthz", "/readyz"]) out.push(`${plane}${p} ${(await fetch(base + p)).status}`);
  }
  assert(out.every((x) => x.endsWith("200")), out.join(", "));
  return ["pass", out.join(", ")];
});

await step("D2.2", "every node live and converged", async () => {
  // converged within a poll or two of the last config change
  const nodes = await until(async () => {
    const n = (await api("GET", "/api/v1/cluster/nodes", dev)).json;
    return n.every((x: any) => x.live && x.converged) ? n : null;
  }, 20000);
  return ["pass", nodes.map((n: any) => `${n.id} ${n.role} v${n.config_version} live converged`).join("; ")];
});

await step("D2.4", "request rate, latency, errors, queue depth in /metrics", async () => {
  const m = await (await fetch(`${GATEWAY}/metrics`)).text();
  const has = (re: RegExp) => re.test(m);
  const got = { requests: has(/^rolter_requests_total/m), latency: has(/^rolter_request_latency_ms_bucket/m), errors: has(/^rolter_upstream_errors_total/m), queueDepth: has(/queue_depth/m) };
  const missing = Object.entries(got).filter(([, v]) => !v).map(([k]) => k);
  return missing.length === 0 ? ["pass", "all four families"] : ["partial", `present: ${Object.keys(got).filter((k) => !missing.includes(k)).join(", ")}; missing: ${missing.join(", ")} — rolter_inflight_requests has no provider label (#1855)`];
});

await step("D3.3", "switch a subsystem off and on without a restart", async () => {
  const before = (await api("GET", "/api/v1/feature-flags", dev)).json;
  const { updated_at, unavailable, ...flags } = before;
  const put = await api("PUT", "/api/v1/feature-flags", dev, { ...flags, response_cache: false });
  if (put.status !== 200) {
    const blocked = unavailable?.map((u: any) => u.flag).join(", ");
    // what the screen does with the same state
    const s = await signIn("dev@rolter.local");
    await goto(s.page, "/feature-flags");
    const toggle = s.page.getByRole("switch", { name: /response cache/i }).first();
    let ui = "no Response cache switch found";
    if (await toggle.count()) {
      await toggle.click();
      await sleep(1500);
      const text = (await s.page.locator("body").innerText()).replace(/\s+/g, " ");
      ui = /unavailable/i.test(text) && /cache_aware_routing|Cache-aware/i.test(text) ? "the screen's save is refused the same way" : `screen after toggling: ${/saved|updated/i.test(text) ? "saved" : "no confirmation"}`;
      const now = (await api("GET", "/api/v1/feature-flags", dev)).json;
      if (now.response_cache !== before.response_cache) await api("PUT", "/api/v1/feature-flags", dev, { ...flags, cache_aware_routing: false, response_cache: before.response_cache });
    }
    await s.context.close();
    return ["bug", `PUT with response_cache=false → ${put.status} "${put.json?.error?.message?.slice(0, 110)}": a stored-on but unavailable flag (${blocked}) blocks every other change; ${ui}`];
  }
  const v0 = (await api("GET", "/api/v1/cluster/nodes", dev)).json[0].config_version;
  await until(async () => (await api("GET", "/api/v1/cluster/nodes", dev)).json.every((n: any) => n.config_version > v0 && n.converged) || null, 20000);
  await api("PUT", "/api/v1/feature-flags", dev, flags);
  return ["pass", `response_cache off → every node converged past v${v0}; restored`];
});

await step("D4.3", "be paged: a channel and an error-rate rule", async () => {
  const ch = await api("POST", "/api/v1/alert-channels", dev, { name: "journey webhook", endpoint: "http://127.0.0.1:9/journey", enabled: false });
  assert(ch.status === 200 || ch.status === 201, `channel ${ch.status} ${JSON.stringify(ch.json).slice(0, 120)}`);
  try {
    const rule = await api("POST", "/api/v1/alert-rules", dev, { name: "journey error rate", signal: "error_rate", threshold: 0.2, window_secs: 300, channel_id: ch.json.id, enabled: false });
    assert(rule.status === 200 || rule.status === 201, `rule ${rule.status} ${JSON.stringify(rule.json).slice(0, 120)}`);
    await api("DELETE", `/api/v1/alert-rules/${rule.json.id}`, dev);
    return ["pass", "channel and error_rate rule created (disabled; firing not exercised here)"];
  } finally {
    await api("DELETE", `/api/v1/alert-channels/${ch.json.id}`, dev);
  }
});

await step("D4.4", "drain a node (the only gateway)", async () => {
  const node = (await api("GET", "/api/v1/cluster/nodes", dev)).json.find((n: any) => n.role === "gateway");
  const r = await api("PUT", `/api/v1/cluster/nodes/${node.id}/drain`, dev, { draining: true });
  if (r.status === 200) {
    await api("PUT", `/api/v1/cluster/nodes/${node.id}/drain`, dev, { draining: false });
    return ["fail", "the only live gateway was drained (undrained again)"];
  }
  return ["pass", `refused for the only live gateway: ${r.status} "${r.json?.error?.message?.slice(0, 100)}"`];
});

await close();
save("devops");
