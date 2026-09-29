import { describe, expect, it } from "bun:test";

import type { GuardrailProviderRow } from "./api";
import {
  providerStatus,
  readEffectiveWebhook,
  resolveEnforcement,
  type EffectiveWebhook,
} from "./guardrail-enforcement";

const ROW: GuardrailProviderRow = {
  id: "primary",
  name: "Production LLM Guard",
  enabled: true,
  url: "https://guardrails.internal/v1/evaluate",
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

/** the effective webhook the postgres store builds from `ROW` */
const FROM_ROW: EffectiveWebhook = {
  enabled: true,
  url: ROW.url,
  stage: "pre_call",
  timeout_ms: 1800,
  max_retries: 1,
  failure_mode: "fail_closed",
  max_body_bytes: 65536,
  auth: { bearer: { token_env: "ROLTER_GUARDRAIL_TOKEN" } },
};

const OFF: EffectiveWebhook = {
  enabled: false,
  url: "",
  stage: "pre_call",
  timeout_ms: 2000,
  max_retries: 0,
  failure_mode: "fail_open",
  max_body_bytes: 65536,
};

describe("readEffectiveWebhook", () => {
  it("reads the section the control plane serves", () => {
    expect(readEffectiveWebhook({ providers: [], guardrail_webhook: FROM_ROW })).toEqual(FROM_ROW);
  });

  it("refuses a missing or malformed section rather than guessing", () => {
    expect(readEffectiveWebhook(undefined)).toBeNull();
    expect(readEffectiveWebhook([])).toBeNull();
    expect(readEffectiveWebhook({ providers: [] })).toBeNull();
    expect(
      readEffectiveWebhook({ guardrail_webhook: { ...FROM_ROW, stage: "output" } }),
    ).toBeNull();
    expect(readEffectiveWebhook({ guardrail_webhook: { ...FROM_ROW, enabled: "yes" } })).toBeNull();
  });
});

describe("resolveEnforcement", () => {
  it("credits the active registry row when the effective webhook is that row", () => {
    const enforcement = resolveEnforcement([ROW], FROM_ROW);
    expect(enforcement).toMatchObject({ state: "enforced", provider: ROW, overridden: null });
    expect(providerStatus(ROW, enforcement)).toBe("enforced");
  });

  it("says a post-call registry row enforces nothing", () => {
    const row = { ...ROW, stage: "post_call" as const };
    const enforcement = resolveEnforcement([row], { ...FROM_ROW, stage: "post_call" });
    expect(enforcement).toMatchObject({ state: "inert", provider: row });
    expect(providerStatus(row, enforcement)).toBe("inert");
  });

  it("names the config-file webhook when it differs from the active row", () => {
    const file = { ...FROM_ROW, url: "https://policy.corp/evaluate", auth: undefined };
    const enforcement = resolveEnforcement([ROW], file);
    expect(enforcement).toMatchObject({ state: "enforced", provider: null, overridden: ROW });
    expect(providerStatus(ROW, enforcement)).toBe("overridden");
  });

  it("names the config-file webhook when no registry row is active", () => {
    const paused = { ...ROW, enabled: false };
    const enforcement = resolveEnforcement([paused], FROM_ROW);
    expect(enforcement).toMatchObject({ state: "enforced", provider: null, overridden: null });
    expect(providerStatus(paused, enforcement)).toBe("paused");
  });

  it("treats any differing field as the file, auth included", () => {
    const noAuth = { ...FROM_ROW, auth: undefined };
    expect(resolveEnforcement([ROW], noAuth)).toMatchObject({ provider: null, overridden: ROW });
    const otherTimeout = { ...FROM_ROW, timeout_ms: 2000 };
    expect(resolveEnforcement([ROW], otherTimeout)).toMatchObject({ provider: null });
  });

  it("reports a post-call config-file webhook as inert, overriding a working row", () => {
    const enforcement = resolveEnforcement([ROW], {
      ...FROM_ROW,
      url: "https://policy.corp/evaluate",
      stage: "post_call",
    });
    expect(enforcement).toMatchObject({ state: "inert", provider: null, overridden: ROW });
    expect(providerStatus(ROW, enforcement)).toBe("overridden");
  });

  it("is off when nothing is enabled anywhere", () => {
    const paused = { ...ROW, enabled: false };
    const enforcement = resolveEnforcement([paused], OFF);
    expect(enforcement).toEqual({ state: "off" });
    expect(providerStatus(paused, enforcement)).toBe("paused");
  });

  it("is unknown when the effective webhook cannot be read", () => {
    const enforcement = resolveEnforcement([ROW], null);
    expect(enforcement).toEqual({ state: "unknown" });
    expect(providerStatus(ROW, enforcement)).toBe("unknown");
  });

  it("is unknown when an enabled row meets a disabled effective webhook", () => {
    expect(resolveEnforcement([ROW], OFF)).toEqual({ state: "unknown" });
  });

  it("still calls an enabled post-call row inert when the config is unreadable", () => {
    const row = { ...ROW, stage: "post_call" as const };
    expect(providerStatus(row, resolveEnforcement([row], null))).toBe("inert");
  });
});
