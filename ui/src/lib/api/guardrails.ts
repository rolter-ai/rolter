import { getJson, sendJson } from "./request";

export interface GuardrailRuleRow {
  id: string;
  name: string;
  enabled: boolean;
  source_type: "builtin" | "pattern";
  builtin: "email" | "phone" | "api_token" | "payment_card" | null;
  pattern: string | null;
  stage: "pre_call" | "post_call";
  action: "annotate" | "block" | "redact";
  replacement: string | null;
  include_system: boolean;
  position: number;
  created_at: string;
  updated_at: string;
}

export type GuardrailRuleInput = Omit<GuardrailRuleRow, "id" | "created_at" | "updated_at">;

export interface GuardrailProviderRow {
  id: string;
  name: string;
  enabled: boolean;
  /** `webhook` posts to `url`; `local` runs a classifier from `model_path` on each gateway */
  kind: "webhook" | "local";
  /** empty for a local provider */
  url: string;
  /** the weights file on the gateway's file system; null for a webhook */
  model_path: string | null;
  /** the weights file's SHA-256, 64 hex digits; null for a webhook */
  model_sha256: string | null;
  stage: "pre_call" | "post_call";
  timeout_ms: number;
  max_retries: number;
  failure_mode: "fail_open" | "fail_closed";
  max_body_bytes: number;
  /** total bytes a local classifier may score; null uses eight body windows */
  max_scored_bytes: number | null;
  /** whether text beyond the local scoring budget is scored or refused */
  on_truncated: "score" | "fail_closed";
  auth_kind: "none" | "bearer" | "shared_secret";
  auth_env: string | null;
  created_at: string;
  updated_at: string;
}

export type GuardrailProviderInput = Omit<GuardrailProviderRow, "id" | "created_at" | "updated_at">;

/** the scopes a guardrail policy can be pinned to, narrowest last */
export const POLICY_SCOPES = ["deployment", "org", "project", "route", "key"] as const;
export type GuardrailPolicyScope = (typeof POLICY_SCOPES)[number];

export interface GuardrailPolicyRow {
  id: string;
  name: string;
  enabled: boolean;
  scope_kind: GuardrailPolicyScope;
  /** null for deployment; an org / project / virtual-key uuid; a route's public model name */
  scope_id: string | null;
  /** registry provider ids, consulted in this order */
  provider_ids: string[];
  /** category -> threshold in [0, 1]; a provider score >= threshold is a violation; "*" applies to any category not listed */
  thresholds: Record<string, number>;
  on_violation: "block" | "annotate";
  streaming: "reject" | "buffer";
  description: string | null;
  created_at: string;
  updated_at: string;
}

export type GuardrailPolicyInput = Omit<GuardrailPolicyRow, "id" | "created_at" | "updated_at">;

export const fetchGuardrailRules = () => getJson<GuardrailRuleRow[]>("/api/v1/guardrails/rules");

export const createGuardrailRule = (body: GuardrailRuleInput) =>
  sendJson<GuardrailRuleRow>("POST", "/api/v1/guardrails/rules", body);

export const updateGuardrailRule = (id: string, body: GuardrailRuleInput) =>
  sendJson<GuardrailRuleRow>("PUT", `/api/v1/guardrails/rules/${id}`, body);

export const deleteGuardrailRule = (id: string) =>
  sendJson<void>("DELETE", `/api/v1/guardrails/rules/${id}`);

/**
 * The guardrail rules and webhook the gateways run, each with where it comes
 * from (`GET /api/v1/guardrails/effective`, superadmin). Left as `unknown`:
 * `readRuleSources` and `readWebhookSource` check the shape, since a status
 * screen must not guess from a document it does not recognise.
 */
export const fetchEffectiveGuardrails = () => getJson<unknown>("/api/v1/guardrails/effective");

export const fetchGuardrailProviders = () =>
  getJson<GuardrailProviderRow[]>("/api/v1/guardrails/providers");

export const createGuardrailProvider = (body: GuardrailProviderInput) =>
  sendJson<GuardrailProviderRow>("POST", "/api/v1/guardrails/providers", body);

export const updateGuardrailProvider = (id: string, body: GuardrailProviderInput) =>
  sendJson<GuardrailProviderRow>("PUT", `/api/v1/guardrails/providers/${id}`, body);

export const deleteGuardrailProvider = (id: string) =>
  sendJson<void>("DELETE", `/api/v1/guardrails/providers/${id}`);

/** a draft rule and the text to run it on; nothing is saved (#2159) */
export interface GuardrailRuleTestInput {
  builtin: GuardrailRuleRow["builtin"];
  pattern: string | null;
  stage: GuardrailRuleRow["stage"];
  action: GuardrailRuleRow["action"];
  replacement: string | null;
  include_system: boolean;
  /** treat the sample as a system message, which a rule skips unless it opts in */
  system: boolean;
  sample: string;
}

export interface GuardrailRuleTestResult {
  outcome: "no_match" | "annotated" | "redacted" | "blocked" | "skipped";
  /** why the rule did not look at the sample, for `skipped` */
  skip?: "system_message" | "over_scan_budget";
  /** every match, even when `spans` is capped */
  match_count: number;
  /** UTF-16 offsets into the sample, so they index a JS string as they are */
  spans: { start: number; end: number }[];
  /** the sample after redaction, for `redacted` */
  redacted?: string;
}

/** why a rule did not act on the sample in a policy run (#3084) */
export type GuardrailTestSkip =
  | "system_message"
  | "over_scan_budget"
  | "other_stage"
  | "after_block"
  | "policy_disabled"
  | "not_compiled";

/**
 * The whole rule set run on a sample, with an optional unsaved draft slotted in
 * at its position (#3084). A draft carrying a saved rule's name replaces it.
 */
export interface GuardrailPolicyTestInput {
  sample: string;
  stage: GuardrailRuleRow["stage"];
  /** treat the sample as a system message, which a rule skips unless it opts in */
  system: boolean;
  draft?: GuardrailPolicyDraft;
}

export type GuardrailPolicyDraft = Omit<GuardrailRuleTestInput, "system" | "sample"> & {
  name?: string | null;
  /** where it runs among the dashboard rules, as on save */
  position?: number;
};

export interface GuardrailPolicyStep {
  rule: string;
  stage: GuardrailRuleRow["stage"];
  action: GuardrailRuleRow["action"];
  outcome: GuardrailRuleTestResult["outcome"];
  skip?: GuardrailTestSkip;
  /** matches against the text as this rule saw it, after earlier redactions */
  match_count: number;
  /** the config file runs first, then the dashboard rules by position */
  source: "file" | "dashboard" | "draft";
}

export interface GuardrailPolicyTestResult {
  outcome: GuardrailRuleTestResult["outcome"];
  steps: GuardrailPolicyStep[];
  /** the rule that would reject the request */
  blocked_by?: string;
  /** the sample after every redaction, when it changed */
  redacted?: string;
}

/** a draft provider; the control plane sends one sample envelope to it (#2265) */
export interface GuardrailProviderTestInput {
  url: string;
  stage: GuardrailProviderRow["stage"];
  timeout_ms: number;
  max_body_bytes: number;
  auth_kind: GuardrailProviderRow["auth_kind"];
  auth_env: string | null;
  /** the user message sent; the server uses a fixed sentence when absent */
  sample?: string;
  /** call without the credential, which the control plane may not hold */
  skip_credential?: boolean;
}

export type GuardrailProviderTestErrorKind =
  | "egress_denied"
  | "credential_unset"
  | "timeout"
  | "connection"
  | "tls"
  | "http_status"
  | "unparsable"
  | "response_too_large"
  | "other";

export interface GuardrailProviderTestResult {
  /** a decision came back; otherwise `error` says why not */
  ok: boolean;
  decision?: {
    action: "allow" | "block" | "transform" | "annotate";
    reason?: string;
    content?: unknown;
    annotations?: unknown;
  };
  error?: { kind: GuardrailProviderTestErrorKind; message: string };
  credential: "not_configured" | "sent" | "skipped" | "unset";
  http_status?: number;
  latency_ms: number;
  truncated: boolean;
}

export const testGuardrailRule = (body: GuardrailRuleTestInput) =>
  sendJson<GuardrailRuleTestResult>("POST", "/api/v1/guardrails/rules/test", body);

export const testGuardrailPolicy = (body: GuardrailPolicyTestInput) =>
  sendJson<GuardrailPolicyTestResult>("POST", "/api/v1/guardrails/policy/test", body);

export const testGuardrailProvider = (body: GuardrailProviderTestInput) =>
  sendJson<GuardrailProviderTestResult>("POST", "/api/v1/guardrails/providers/test", body);

export const fetchGuardrailPolicies = () =>
  getJson<GuardrailPolicyRow[]>("/api/v1/guardrails/policies");

export const createGuardrailPolicy = (body: GuardrailPolicyInput) =>
  sendJson<GuardrailPolicyRow>("POST", "/api/v1/guardrails/policies", body);

export const updateGuardrailPolicy = (id: string, body: GuardrailPolicyInput) =>
  sendJson<GuardrailPolicyRow>("PUT", `/api/v1/guardrails/policies/${id}`, body);

export const deleteGuardrailPolicy = (id: string) =>
  sendJson<void>("DELETE", `/api/v1/guardrails/policies/${id}`);
