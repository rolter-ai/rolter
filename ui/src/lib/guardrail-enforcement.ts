import type { GuardrailPolicyRow, GuardrailProviderRow } from "@/lib/api";

/**
 * One guardrail provider a gateway runs, as `semantic.providers` of
 * `GET /api/v1/guardrails/effective` lists it: the config file's
 * `[guardrail_webhook]` / `[[semantic_guards.providers]]` plus every enabled
 * registry row (a file provider wins a name collision).
 */
export interface SemanticProvider {
  name: string;
  url: string;
  stage: "pre_call" | "post_call";
  failure_mode: "fail_open" | "fail_closed";
  timeout_ms: number;
  source: "file" | "registry";
  /** the registry row, `null` for a provider the config file defines */
  provider_id: string | null;
}

/** a policy as the gateways run it; providers are named, not identified */
export interface SemanticPolicy {
  name: string;
  enabled: boolean;
  scope: GuardrailPolicyRow["scope_kind"];
  scope_id: string | null;
  providers: string[];
  thresholds: Record<string, number>;
  on_violation: GuardrailPolicyRow["on_violation"];
  streaming: GuardrailPolicyRow["streaming"];
  source: "file" | "registry";
  policy_id: string | null;
}

export interface SemanticGuards {
  providers: SemanticProvider[];
  policies: SemanticPolicy[];
  /** enabled providers no enabled policy lists: they run on every request no policy matches */
  unattached: string[];
}

const STAGES = new Set(["pre_call", "post_call"]);
const FAILURE_MODES = new Set(["fail_open", "fail_closed"]);
const SOURCES = new Set(["file", "registry"]);
const SCOPES = new Set(["deployment", "org", "project", "route", "key"]);
const ON_VIOLATION = new Set(["block", "annotate"]);
const STREAMING = new Set(["reject", "buffer"]);

const isObject = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);
const isString = (value: unknown): value is string => typeof value === "string";
const isOneOf = (set: Set<string>, value: unknown) => isString(value) && set.has(value);
const nullableString = (value: unknown) => value === null || isString(value);

function readProvider(value: unknown): SemanticProvider | null {
  if (!isObject(value)) return null;
  if (
    !isString(value.name) ||
    !isString(value.url) ||
    !isOneOf(STAGES, value.stage) ||
    !isOneOf(FAILURE_MODES, value.failure_mode) ||
    typeof value.timeout_ms !== "number" ||
    !isOneOf(SOURCES, value.source) ||
    !nullableString(value.provider_id ?? null)
  ) {
    return null;
  }
  return { ...value, provider_id: value.provider_id ?? null } as unknown as SemanticProvider;
}

function readPolicy(value: unknown): SemanticPolicy | null {
  if (!isObject(value)) return null;
  if (
    !isString(value.name) ||
    typeof value.enabled !== "boolean" ||
    !isOneOf(SCOPES, value.scope) ||
    !nullableString(value.scope_id ?? null) ||
    !Array.isArray(value.providers) ||
    !value.providers.every(isString) ||
    !isObject(value.thresholds) ||
    !Object.values(value.thresholds).every((n) => typeof n === "number") ||
    !isOneOf(ON_VIOLATION, value.on_violation) ||
    !isOneOf(STREAMING, value.streaming) ||
    !isOneOf(SOURCES, value.source) ||
    !nullableString(value.policy_id ?? null)
  ) {
    return null;
  }
  return {
    ...value,
    scope_id: value.scope_id ?? null,
    policy_id: value.policy_id ?? null,
  } as unknown as SemanticPolicy;
}

function readAll<T>(list: unknown, read: (value: unknown) => T | null): T[] | null {
  if (!Array.isArray(list)) return null;
  const out: T[] = [];
  for (const item of list) {
    const value = read(item);
    if (value === null) return null;
    out.push(value);
  }
  return out;
}

/**
 * Read the `semantic` object out of the answer of the effective endpoint, or
 * `null` when it is missing or not the shape this screen knows. A status screen
 * must not guess from a document it does not recognise: `null` is "cannot
 * confirm".
 */
export function readSemanticGuards(doc: unknown): SemanticGuards | null {
  if (!isObject(doc) || !isObject(doc.semantic)) return null;
  const providers = readAll(doc.semantic.providers, readProvider);
  const policies = readAll(doc.semantic.policies, readPolicy);
  const unattached = readAll(doc.semantic.unattached, (v) => (isString(v) ? v : null));
  if (!providers || !policies || !unattached) return null;
  return { providers, policies, unattached };
}

/**
 * What the gateways do with external guardrails. A registry row that says
 * `enabled` is not the same thing as a provider they run, so the screen reports
 * `semantic.providers` and not the switch (#2162, #459).
 */
export type Enforcement =
  /** the effective document could not be read, or it contradicts the registry */
  | { state: "unknown" }
  /** no provider is enabled anywhere */
  | { state: "off" }
  | {
      state: "enforced";
      providers: SemanticProvider[];
      unattached: string[];
      policies: SemanticPolicy[];
    };

/** What one registry row's card says about it. */
export type ProviderStatus = "enforced" | "overridden" | "unknown" | "paused";

/**
 * Resolve what the gateways enforce from the registry and the effective
 * document. `semantic` is `null` when it could not be read.
 */
export function resolveEnforcement(
  rows: GuardrailProviderRow[],
  semantic: SemanticGuards | null,
): Enforcement {
  if (!semantic) return { state: "unknown" };
  if (semantic.providers.length === 0) {
    // an enabled registry row always reaches the effective document, so none
    // there next to an enabled row means the two reads disagree
    return rows.some((row) => row.enabled) ? { state: "unknown" } : { state: "off" };
  }
  return {
    state: "enforced",
    providers: semantic.providers,
    unattached: semantic.unattached,
    policies: semantic.policies,
  };
}

/** What a registry row's card should say, given the resolved enforcement. */
export function providerStatus(
  row: GuardrailProviderRow,
  enforcement: Enforcement,
): ProviderStatus {
  if (!row.enabled) return "paused";
  if (enforcement.state !== "enforced") return "unknown";
  if (enforcement.providers.some((provider) => provider.provider_id === row.id)) return "enforced";
  // a config-file provider of the same name wins the collision
  if (enforcement.providers.some((p) => p.source === "file" && p.name === row.name)) {
    return "overridden";
  }
  return "unknown";
}

/** the scope a policy is pinned to, as a short noun phrase key and its id */
export function policyScopeLabel(
  policy: Pick<GuardrailPolicyRow, "scope_kind" | "scope_id">,
  kindLabel: (kind: GuardrailPolicyRow["scope_kind"]) => string,
): string {
  return policy.scope_id
    ? `${kindLabel(policy.scope_kind)} ${policy.scope_id}`
    : kindLabel(policy.scope_kind);
}
