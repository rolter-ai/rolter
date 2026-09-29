import { describe, expect, it } from "bun:test";

import type { GuardrailRuleRow } from "./api";
import {
  readEffectivePolicy,
  resolvePolicy,
  switchesOff,
  type EffectiveRule,
} from "./guardrail-policy";

const ROW: GuardrailRuleRow = {
  id: "rule-email",
  name: "Redact customer email",
  enabled: true,
  source_type: "builtin",
  builtin: "email",
  pattern: null,
  stage: "pre_call",
  action: "redact",
  replacement: "[REDACTED:EMAIL]",
  include_system: false,
  position: 10,
  created_at: "2026-08-02T00:00:00Z",
  updated_at: "2026-08-02T00:00:00Z",
};

/** the effective rule the postgres store builds from `ROW` */
const FROM_ROW: EffectiveRule = {
  name: ROW.name,
  builtin: "email",
  stage: "pre_call",
  action: "redact",
  replacement: "[REDACTED:EMAIL]",
  include_system: false,
};

/** a rule only the config file defines */
const FILE_ONLY: EffectiveRule = {
  name: "Block AWS keys",
  pattern: "AKIA[A-Z0-9]{16}",
  stage: "pre_call",
  action: "block",
  include_system: true,
};

const config = (on: boolean, rules: EffectiveRule[], enabled = on, routes: unknown[] = []) => ({
  providers: [],
  routes,
  feature_flags: { guardrails: on },
  guardrails: { enabled, streaming_post_call: "reject", rules },
});

/** a route as `GET /api/v1/config` lists it, with an `advanced.guardrails` override */
const route = (model: string, guardrails?: { disable?: string[]; enable?: string[] }) => ({
  model,
  strategy: "round_robin",
  targets: [{ provider: "openai", model, weight: 1 }],
  advanced: guardrails === undefined ? { capabilities: [] } : { capabilities: [], guardrails },
});

describe("readEffectivePolicy", () => {
  it("reads the flag, the streaming mode and the merged rules", () => {
    expect(readEffectivePolicy(config(true, [FILE_ONLY, FROM_ROW]))).toEqual({
      on: true,
      streaming: "reject",
      rules: [FILE_ONLY, FROM_ROW],
      routes: [],
    });
  });

  it("reads an absent rules list as empty, since the store skips an empty one", () => {
    const document = config(false, []);
    delete (document.guardrails as { rules?: unknown }).rules;
    expect(readEffectivePolicy(document)).toEqual({
      on: false,
      streaming: "reject",
      rules: [],
      routes: [],
    });
  });

  it("reads each route's override, and skips a route that sets none", () => {
    const policy = readEffectivePolicy(
      config(true, [FROM_ROW], true, [
        route("gpt-4o"),
        // an empty override serializes as `{}`, since both lists skip when empty
        route("claude-sonnet", {}),
        route("support-bot", { disable: [ROW.name] }),
        route("fake-llm", { enable: [ROW.name] }),
        { model: "no-advanced", strategy: "round_robin", targets: [] },
      ]),
    );
    expect(policy?.routes).toEqual([
      { model: "support-bot", disable: [ROW.name], enable: [] },
      { model: "fake-llm", disable: [], enable: [ROW.name] },
    ]);
  });

  it("refuses a route list it cannot read, since an override switches rules off", () => {
    for (const routes of [
      [{ strategy: "round_robin" }],
      [route("gpt-4o", { disable: "Redact customer email" as never })],
      [route("gpt-4o", { enable: [42 as never] })],
      [{ model: "gpt-4o", advanced: [] }],
      [{ model: "gpt-4o", advanced: { guardrails: ["x"] } }],
    ]) {
      expect(readEffectivePolicy(config(true, [FROM_ROW], true, routes))).toBeNull();
    }
    const document = { ...config(true, [FROM_ROW]), routes: {} };
    expect(readEffectivePolicy(document)).toBeNull();
  });

  it("refuses the default document the control plane answers when its store fails", () => {
    // `GatewayConfig::default()`: the flag defaults on, the section off
    expect(readEffectivePolicy(config(true, [], false))).toBeNull();
  });

  it("refuses a missing or malformed document rather than guessing", () => {
    expect(readEffectivePolicy(undefined)).toBeNull();
    expect(readEffectivePolicy([])).toBeNull();
    expect(readEffectivePolicy({ guardrails: { enabled: true } })).toBeNull();
    expect(readEffectivePolicy({ ...config(true, []), feature_flags: {} })).toBeNull();
    const streaming = config(true, []);
    streaming.guardrails.streaming_post_call = "buffer";
    expect(readEffectivePolicy(streaming)).toBeNull();
    expect(
      readEffectivePolicy(config(true, [{ ...FROM_ROW, stage: "output" as never }])),
    ).toBeNull();
    expect(
      readEffectivePolicy(config(true, [{ ...FROM_ROW, builtin: "iban" as never }])),
    ).toBeNull();
  });
});

describe("resolvePolicy", () => {
  it("credits a row the effective policy carries field for field", () => {
    const resolved = resolvePolicy([ROW], readEffectivePolicy(config(true, [FROM_ROW])));
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "enforced" });
    expect(resolved.fileRules).toEqual([]);
  });

  it("stops calling a row enforced while the flag is off", () => {
    const resolved = resolvePolicy([ROW], readEffectivePolicy(config(false, [FROM_ROW])));
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "off" });
  });

  it("lists a rule no row has the name of as the config file's", () => {
    const resolved = resolvePolicy([ROW], readEffectivePolicy(config(true, [FILE_ONLY, FROM_ROW])));
    expect(resolved.fileRules).toEqual([FILE_ONLY]);
  });

  it("reports a row whose name the config file takes with a different rule", () => {
    const file = { ...FROM_ROW, action: "block" as const, replacement: undefined };
    const resolved = resolvePolicy([ROW], readEffectivePolicy(config(true, [file])));
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "overridden", by: file });
    expect(resolved.fileRules).toEqual([file]);
  });

  it("treats any differing field as the file's rule", () => {
    for (const file of [
      { ...FROM_ROW, include_system: true },
      { ...FROM_ROW, stage: "post_call" as const },
      { ...FROM_ROW, replacement: "[EMAIL]" },
      { ...FROM_ROW, builtin: undefined, pattern: "[a-z]+@corp" },
    ]) {
      const resolved = resolvePolicy([ROW], readEffectivePolicy(config(true, [file])));
      expect(resolved.rows.get(ROW.id)).toMatchObject({ state: "overridden" });
    }
  });

  it("names the config-file rule a paused row would clash with", () => {
    const paused = { ...ROW, enabled: false };
    // a paused row never reaches the policy, so even an identical rule is the file's
    const resolved = resolvePolicy([paused], readEffectivePolicy(config(true, [FROM_ROW])));
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "paused", clash: FROM_ROW });
    expect(resolved.fileRules).toEqual([FROM_ROW]);
  });

  it("is unknown for an enabled row the policy does not carry", () => {
    const resolved = resolvePolicy([ROW], readEffectivePolicy(config(true, [])));
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "unknown" });
  });

  it("is unknown for every enabled row when the config is unreadable", () => {
    const paused = { ...ROW, id: "rule-paused", name: "Paused", enabled: false };
    const resolved = resolvePolicy([ROW, paused], null);
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "unknown" });
    expect(resolved.rows.get(paused.id)).toEqual({ state: "paused", clash: null });
    expect(resolved.fileRules).toEqual([]);
  });
});

describe("switchesOff", () => {
  const override = (disable: string[], enable: string[] = []) => ({ model: "m", disable, enable });

  it("switches off a rule the route disables", () => {
    expect(switchesOff(override([ROW.name]), ROW.name)).toBe(true);
    expect(switchesOff(override(["Another rule"]), ROW.name)).toBe(false);
  });

  it("lets enable win a conflict, as RouteGuardrails::allows does", () => {
    expect(switchesOff(override([ROW.name], [ROW.name]), ROW.name)).toBe(false);
  });

  it("matches the rule's trimmed name, the one the gateway compiles", () => {
    expect(switchesOff(override([ROW.name]), ` ${ROW.name} `)).toBe(true);
    // an override is compared as written, so a padded one names no rule
    expect(switchesOff(override([` ${ROW.name}`]), ROW.name)).toBe(false);
  });
});

describe("offRoutes", () => {
  const resolve = (routes: unknown[], rules = [FILE_ONLY, FROM_ROW], rows = [ROW]) =>
    resolvePolicy(rows, readEffectivePolicy(config(true, rules, true, routes)));

  it("names the routes that switch each rule off, sorted", () => {
    const resolved = resolve([
      route("support-bot", { disable: [ROW.name] }),
      route("gpt-4o"),
      route("internal-search", { disable: [ROW.name, FILE_ONLY.name] }),
    ]);
    expect(resolved.offRoutes.get(ROW.name)).toEqual(["internal-search", "support-bot"]);
    expect(resolved.offRoutes.get(FILE_ONLY.name)).toEqual(["internal-search"]);
    // the card state is unchanged: an exception is extra, not a different state
    expect(resolved.rows.get(ROW.id)).toEqual({ state: "enforced" });
  });

  it("leaves out a route that also enables the rule", () => {
    const resolved = resolve([
      route("support-bot", { disable: [ROW.name], enable: [ROW.name] }),
      route("fake-llm", { enable: [ROW.name] }),
    ]);
    expect(resolved.offRoutes.has(ROW.name)).toBe(false);
  });

  it("ignores an override naming a rule the policy no longer has", () => {
    const resolved = resolve([route("support-bot", { disable: ["Deleted rule"] })]);
    expect(resolved.offRoutes.size).toBe(0);
    expect([...resolved.offRoutes.keys()]).not.toContain("Deleted rule");
  });

  it("names a route listed twice once", () => {
    const twice = route("support-bot", { disable: [ROW.name] });
    expect(resolve([twice, twice]).offRoutes.get(ROW.name)).toEqual(["support-bot"]);
  });

  it("is empty when the config is unreadable", () => {
    expect(resolvePolicy([ROW], null).offRoutes.size).toBe(0);
  });
});
