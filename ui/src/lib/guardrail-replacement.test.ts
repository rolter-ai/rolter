import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import type { GuardrailRuleInput } from "./api";
import {
  DETECTOR_TOKENS,
  PATTERN_TOKEN,
  defaultToken,
  replacementToken,
  ruleBody,
  withSource,
} from "./guardrail-replacement";

// the dialog's placeholder and the card both promise the token the gateway
// writes, so the map has to be the gateway's own, read out of the source
const GUARDRAILS = fileURLToPath(
  new URL("../../../crates/rolter-core/src/guardrails.rs", import.meta.url),
);

/** `BuiltinRule::default_token`'s arms, keyed by the serde (snake_case) name */
function rustDetectorTokens(source: string): Record<string, string> {
  const start = source.indexOf("pub fn default_token(self)");
  if (start < 0) throw new Error("BuiltinRule::default_token not found in guardrails.rs");
  const body = source.slice(start, source.indexOf("\n    }\n", start));
  const arms = [...body.matchAll(/Self::(\w+)\s*=>\s*"([^"]*)"/g)];
  return Object.fromEntries(
    arms.map(([, variant, token]) => [
      variant.replace(/[A-Z]/g, (c, at: number) => (at ? "_" : "") + c.toLowerCase()),
      token,
    ]),
  );
}

/** the token `CompiledRule::from_config` falls back on for a custom regex */
function rustPatternToken(source: string): string | undefined {
  return /\(None,\s*Some\(pattern\)\)\s*=>\s*\([^,]+,\s*"([^"]*)"\)/.exec(source)?.[1];
}

const RULE: GuardrailRuleInput = {
  name: "Mask phone numbers",
  enabled: true,
  source_type: "builtin",
  builtin: "email",
  pattern: null,
  stage: "pre_call",
  action: "redact",
  replacement: null,
  include_system: false,
  position: 0,
};

describe("the gateway's default tokens", () => {
  it("reads the arms out of the Rust match", () => {
    const source = `
    pub fn default_token(self) -> &'static str {
        match self {
            Self::Email => "[REDACTED:EMAIL]",
            Self::ApiToken => "[REDACTED:API_TOKEN]",
        }
    }
    pub fn other(self) -> &'static str { match self { Self::Phone => "not a token" } }
`;
    expect(rustDetectorTokens(source)).toEqual({
      email: "[REDACTED:EMAIL]",
      api_token: "[REDACTED:API_TOKEN]",
    });
  });

  it("lists every detector with the token the gateway writes", () => {
    const tokens = rustDetectorTokens(readFileSync(GUARDRAILS, "utf8"));
    expect(Object.keys(tokens).length).toBeGreaterThan(0);
    expect(DETECTOR_TOKENS).toEqual(tokens as typeof DETECTOR_TOKENS);
  });

  it("uses the gateway's fallback for a custom regex", () => {
    expect(PATTERN_TOKEN).toBe(rustPatternToken(readFileSync(GUARDRAILS, "utf8"))!);
    expect(defaultToken(null)).toBe("[REDACTED]");
    expect(defaultToken(undefined)).toBe("[REDACTED]");
    expect(defaultToken("phone")).toBe("[REDACTED:PHONE]");
  });
});

describe("replacementToken", () => {
  it("falls back to the detector's token when a redact rule sets none", () => {
    expect(replacementToken({ action: "redact", builtin: "phone", replacement: null })).toBe(
      "[REDACTED:PHONE]",
    );
    // a config-file rule leaves both fields out
    expect(replacementToken({ action: "redact" })).toBe("[REDACTED]");
  });

  it("reports the token a redact rule sets", () => {
    expect(replacementToken({ action: "redact", builtin: "email", replacement: "<email>" })).toBe(
      "<email>",
    );
  });

  it("reports no token for a rule that never rewrites", () => {
    for (const action of ["block", "annotate"] as const) {
      expect(
        replacementToken({ action, builtin: "phone", replacement: "[REDACTED:EMAIL]" }),
      ).toBeNull();
    }
  });
});

describe("withSource", () => {
  it("leaves an empty token empty, so the field shows the new default", () => {
    expect(withSource(RULE, { builtin: "phone" })).toEqual({ ...RULE, builtin: "phone" });
  });

  it("clears a token still at the previous detector's default", () => {
    const stored = { ...RULE, replacement: "[REDACTED:EMAIL]" };
    expect(withSource(stored, { builtin: "phone" }).replacement).toBeNull();
    expect(
      withSource(stored, { source_type: "pattern", builtin: null, pattern: "" }).replacement,
    ).toBeNull();
    const pattern = { ...RULE, source_type: "pattern" as const, builtin: null, pattern: "x" };
    expect(
      withSource(
        { ...pattern, replacement: "[REDACTED]" },
        { source_type: "builtin", builtin: "email", pattern: null },
      ).replacement,
    ).toBeNull();
  });

  it("keeps a token the user typed", () => {
    const typed = { ...RULE, replacement: "<contact>" };
    expect(withSource(typed, { builtin: "phone" }).replacement).toBe("<contact>");
    // another detector's default was typed, not inherited
    const other = { ...RULE, builtin: "phone" as const, replacement: "[REDACTED:EMAIL]" };
    expect(withSource(other, { builtin: "payment_card" }).replacement).toBe("[REDACTED:EMAIL]");
  });
});

describe("ruleBody", () => {
  it("sends a redact rule's token, and none when it is empty", () => {
    expect(ruleBody({ ...RULE, replacement: "<phone>" }).replacement).toBe("<phone>");
    expect(ruleBody({ ...RULE, replacement: "" }).replacement).toBeNull();
    expect(ruleBody(RULE).replacement).toBeNull();
  });

  it("sends no token for a rule that never rewrites", () => {
    for (const action of ["block", "annotate"] as const) {
      expect(ruleBody({ ...RULE, action, replacement: "[REDACTED:EMAIL]" })).toEqual({
        ...RULE,
        action,
        replacement: null,
      });
    }
  });
});
