import type { GuardrailRuleInput, GuardrailRuleRow } from "@/lib/api";

type Builtin = NonNullable<GuardrailRuleRow["builtin"]>;

/**
 * The token the gateway writes over a match when a redact rule on a built-in
 * detector sets no `replacement`.
 *
 * Mirrors `BuiltinRule::default_token` in `crates/rolter-core/src/guardrails.rs`.
 * `guardrail-replacement.test.ts` reads that match out of the Rust source and
 * fails when the two drift, so a detector added there cannot go unlisted here.
 */
export const DETECTOR_TOKENS: Record<Builtin, string> = {
  email: "[REDACTED:EMAIL]",
  phone: "[REDACTED:PHONE]",
  api_token: "[REDACTED:API_TOKEN]",
  payment_card: "[REDACTED:CARD]",
};

/**
 * The token a custom-regex rule with no `replacement` writes, the fallback in
 * `CompiledRule::from_config` in the same file.
 */
export const PATTERN_TOKEN = "[REDACTED]";

/** The token a redact rule with no `replacement` writes, from its detector. */
export const defaultToken = (builtin: Builtin | null | undefined): string =>
  builtin ? DETECTOR_TOKENS[builtin] : PATTERN_TOKEN;

/**
 * The token a rule writes over a match, or `null` for a rule that never
 * rewrites. Only `redact` rewrites, so a `block` or `annotate` rule reports no
 * token even when a stale `replacement` is stored with it (#2160).
 */
export function replacementToken(rule: {
  action: GuardrailRuleRow["action"];
  builtin?: GuardrailRuleRow["builtin"];
  replacement?: string | null;
}): string | null {
  if (rule.action !== "redact") return null;
  // the gateway falls back on `None` only, so a stored empty string stays empty
  return rule.replacement ?? defaultToken(rule.builtin);
}

/**
 * Apply a change of source or detector to the rule dialog's form.
 *
 * A token that is still empty, or still the previous detector's default, was
 * never edited, so it is cleared and the empty field shows the new detector's
 * default instead. A token the user typed is kept (#2160).
 */
export function withSource(
  form: GuardrailRuleInput,
  patch: Partial<Pick<GuardrailRuleInput, "source_type" | "builtin" | "pattern">>,
): GuardrailRuleInput {
  const next = { ...form, ...patch };
  if (form.replacement === defaultToken(form.builtin)) next.replacement = null;
  return next;
}

/**
 * The body the rule dialog sends. Only a redact rule carries a replacement,
 * and an empty one is sent as `null` so the gateway writes the detector's
 * default token (#2160).
 */
export function ruleBody(form: GuardrailRuleInput): GuardrailRuleInput {
  return { ...form, replacement: form.action === "redact" ? form.replacement || null : null };
}
