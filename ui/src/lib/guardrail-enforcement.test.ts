import { describe, expect, it } from "bun:test";

import type { GuardrailProviderRow } from "./api";
import {
  policyScopeLabel,
  providerStatus,
  readSemanticGuards,
  resolveEnforcement,
  type SemanticGuards,
  type SemanticProvider,
} from "./guardrail-enforcement";

const ROW: GuardrailProviderRow = {
  id: "primary",
  name: "toxicity",
  enabled: true,
  kind: "webhook",
  url: "https://guardrails.internal/v1/evaluate",
  model_path: null,
  model_sha256: null,
  stage: "pre_call",
  timeout_ms: 1800,
  max_retries: 1,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  auth_kind: "bearer",
  auth_env: "ROLTER_GUARDRAIL_TOKEN",
  created_at: "2026-08-02T00:00:00Z",
  updated_at: "2026-08-02T00:00:00Z",
};

const FROM_ROW: SemanticProvider = {
  name: "toxicity",
  url: ROW.url,
  stage: "pre_call",
  failure_mode: "fail_closed",
  timeout_ms: 1800,
  source: "registry",
  provider_id: "primary",
};

const FROM_FILE: SemanticProvider = {
  name: "pii",
  url: "https://policy.corp/evaluate",
  stage: "post_call",
  failure_mode: "fail_open",
  timeout_ms: 1500,
  source: "file",
  provider_id: null,
};

const GUARDS: SemanticGuards = { providers: [FROM_ROW, FROM_FILE], policies: [], unattached: [] };

describe("readSemanticGuards", () => {
  it("reads the semantic object the control plane serves", () => {
    expect(readSemanticGuards({ source: "none", semantic: GUARDS })).toEqual(GUARDS);
  });

  it("refuses a missing or malformed object rather than guessing", () => {
    expect(readSemanticGuards(undefined)).toBeNull();
    expect(readSemanticGuards([])).toBeNull();
    expect(readSemanticGuards({ source: "file" })).toBeNull();
    expect(readSemanticGuards({ semantic: { ...GUARDS, unattached: "none" } })).toBeNull();
    expect(
      readSemanticGuards({
        semantic: { ...GUARDS, providers: [{ ...FROM_ROW, stage: "output" }] },
      }),
    ).toBeNull();
    expect(
      readSemanticGuards({
        semantic: { ...GUARDS, policies: [{ name: "p", enabled: true }] },
      }),
    ).toBeNull();
  });
});

describe("resolveEnforcement", () => {
  it("lists every provider the gateways run", () => {
    const enforcement = resolveEnforcement([ROW], GUARDS);
    expect(enforcement).toMatchObject({ state: "enforced", providers: [FROM_ROW, FROM_FILE] });
    expect(providerStatus(ROW, enforcement)).toBe("enforced");
  });

  it("credits every enabled row at once", () => {
    const other = { ...ROW, id: "second", name: "pii-registry" };
    const enforcement = resolveEnforcement([ROW, other], {
      ...GUARDS,
      providers: [FROM_ROW, { ...FROM_ROW, name: "pii-registry", provider_id: "second" }],
    });
    expect(providerStatus(ROW, enforcement)).toBe("enforced");
    expect(providerStatus(other, enforcement)).toBe("enforced");
  });

  it("says a row shadowed by a file provider of the same name is overridden", () => {
    const shadowed = { ...ROW, id: "shadow", name: "pii" };
    expect(providerStatus(shadowed, resolveEnforcement([shadowed], GUARDS))).toBe("overridden");
  });

  it("is off when nothing is enabled anywhere", () => {
    const paused = { ...ROW, enabled: false };
    const enforcement = resolveEnforcement([paused], {
      providers: [],
      policies: [],
      unattached: [],
    });
    expect(enforcement).toEqual({ state: "off" });
    expect(providerStatus(paused, enforcement)).toBe("paused");
  });

  it("is unknown when the effective document cannot be read", () => {
    const enforcement = resolveEnforcement([ROW], null);
    expect(enforcement).toEqual({ state: "unknown" });
    expect(providerStatus(ROW, enforcement)).toBe("unknown");
  });

  it("is unknown when an enabled row meets an empty provider list", () => {
    expect(resolveEnforcement([ROW], { providers: [], policies: [], unattached: [] })).toEqual({
      state: "unknown",
    });
  });
});

describe("policyScopeLabel", () => {
  const kind = (value: string) => `<${value}>`;
  it("names the scope and its id, and leaves a deployment bare", () => {
    expect(policyScopeLabel({ scope_kind: "route", scope_id: "gpt-4o" }, kind)).toBe(
      "<route> gpt-4o",
    );
    expect(policyScopeLabel({ scope_kind: "deployment", scope_id: null }, kind)).toBe(
      "<deployment>",
    );
  });
});
