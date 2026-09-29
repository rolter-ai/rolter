import { describe, expect, it } from "bun:test";

import type { GuardrailRuleRow } from "./api";
import { readEffectivePolicy, resolvePolicy, type EffectiveRule } from "./guardrail-policy";

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

const config = (on: boolean, rules: EffectiveRule[], enabled = on) => ({
  providers: [],
  feature_flags: { guardrails: on },
  guardrails: { enabled, streaming_post_call: "reject", rules },
});

describe("readEffectivePolicy", () => {
  it("reads the flag, the streaming mode and the merged rules", () => {
    expect(readEffectivePolicy(config(true, [FILE_ONLY, FROM_ROW]))).toEqual({
      on: true,
      streaming: "reject",
      rules: [FILE_ONLY, FROM_ROW],
    });
  });

  it("reads an absent rules list as empty, since the store skips an empty one", () => {
    const document = config(false, []);
    delete (document.guardrails as { rules?: unknown }).rules;
    expect(readEffectivePolicy(document)).toEqual({ on: false, streaming: "reject", rules: [] });
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
