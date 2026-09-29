import type { GuardrailProviderRow } from "@/lib/api";

/**
 * The `guardrail_webhook` section of the effective config (`GET /api/v1/config`).
 *
 * This is the webhook the control plane hands every gateway, after the file and
 * the registry are merged: an enabled `[guardrail_webhook]` in the config file
 * wins, and the one enabled registry provider fills in only when it is off
 * (`MergedConfigStore::load` in `crates/rolter-store/src/lib.rs`).
 */
export interface EffectiveWebhook {
  enabled: boolean;
  url: string;
  stage: "pre_call" | "post_call";
  timeout_ms: number;
  max_retries: number;
  failure_mode: "fail_open" | "fail_closed";
  max_body_bytes: number;
  /** `{ bearer: { token_env } }` or `{ shared_secret: { secret_env } }`; absent for none */
  auth?: unknown;
}

/**
 * What the gateway actually does with external guardrails.
 *
 * A registry row that says `enabled` is not the same thing: the gateway runs
 * only the pre-call stage, and a config-file webhook overrides the registry
 * whenever it is on. The screen reports this instead of the switch (#2162).
 */
export type Enforcement =
  /** the effective webhook could not be read, or it contradicts the registry */
  | { state: "unknown" }
  /** no webhook is on anywhere, so the gateway consults no guardrail service */
  | { state: "off" }
  | {
      /**
       * `enforced`: every request goes to the webhook before it goes upstream.
       * `inert`: a webhook is on at the post-call stage, which the gateway does
       * not run yet (`consult_pre_call` returns early for it), so it checks
       * nothing
       */
      state: "enforced" | "inert";
      /** the registry row in force, or `null` when the config-file webhook is */
      provider: GuardrailProviderRow | null;
      /** an enabled registry row the config-file webhook is overriding */
      overridden: GuardrailProviderRow | null;
      webhook: EffectiveWebhook;
    };

/** What one registry row's card says about it. */
export type ProviderStatus = "enforced" | "inert" | "overridden" | "unknown" | "paused";

const STAGES = new Set(["pre_call", "post_call"]);
const FAILURE_MODES = new Set(["fail_open", "fail_closed"]);

/**
 * Read the effective webhook out of the config document, or `null` when the
 * section is missing or malformed. A shape this screen does not recognise is
 * treated as unreadable rather than guessed at, since the answer it feeds is a
 * security status.
 */
export function readEffectiveWebhook(config: unknown): EffectiveWebhook | null {
  if (!config || typeof config !== "object" || Array.isArray(config)) return null;
  const section = (config as Record<string, unknown>).guardrail_webhook;
  if (!section || typeof section !== "object") return null;
  const hook = section as Record<string, unknown>;
  if (
    typeof hook.enabled !== "boolean" ||
    typeof hook.url !== "string" ||
    typeof hook.stage !== "string" ||
    !STAGES.has(hook.stage) ||
    typeof hook.failure_mode !== "string" ||
    !FAILURE_MODES.has(hook.failure_mode) ||
    typeof hook.timeout_ms !== "number" ||
    typeof hook.max_retries !== "number" ||
    typeof hook.max_body_bytes !== "number"
  ) {
    return null;
  }
  return hook as unknown as EffectiveWebhook;
}

/** the credential a hook carries, as `kind:env`, in the shape both sides share */
function authKey(kind: string, env: string | null | undefined): string {
  return kind === "none" || !env ? "none" : `${kind}:${env}`;
}

function webhookAuthKey(auth: unknown): string {
  if (!auth || typeof auth !== "object") return "none";
  const value = auth as Record<string, Record<string, unknown> | undefined>;
  if (typeof value.bearer?.token_env === "string") {
    return authKey("bearer", value.bearer.token_env);
  }
  if (typeof value.shared_secret?.secret_env === "string") {
    return authKey("shared_secret", value.shared_secret.secret_env);
  }
  return "none";
}

/**
 * Whether the effective webhook is this registry row, field for field, as the
 * postgres store maps a row into one. Anything that differs means the config
 * file supplied the webhook instead.
 */
function isRow(row: GuardrailProviderRow, hook: EffectiveWebhook): boolean {
  return (
    row.url === hook.url &&
    row.stage === hook.stage &&
    row.failure_mode === hook.failure_mode &&
    row.timeout_ms === hook.timeout_ms &&
    row.max_retries === hook.max_retries &&
    row.max_body_bytes === hook.max_body_bytes &&
    authKey(row.auth_kind, row.auth_env) === webhookAuthKey(hook.auth)
  );
}

/**
 * Resolve what the gateway enforces from the registry and the effective
 * webhook. `webhook` is `null` when the effective config could not be read.
 */
export function resolveEnforcement(
  providers: GuardrailProviderRow[],
  webhook: EffectiveWebhook | null,
): Enforcement {
  if (!webhook) return { state: "unknown" };
  // the store lets at most one row be enabled (`guardrail_providers_one_enabled`)
  const active = providers.find((provider) => provider.enabled) ?? null;
  if (!webhook.enabled) {
    // an enabled registry row always reaches the effective config, so a
    // disabled webhook next to one means the two reads disagree
    return active ? { state: "unknown" } : { state: "off" };
  }
  const fromRegistry = active !== null && isRow(active, webhook);
  return {
    state: webhook.stage === "pre_call" ? "enforced" : "inert",
    provider: fromRegistry ? active : null,
    overridden: fromRegistry ? null : active,
    webhook,
  };
}

/** What a registry row's card should say, given the resolved enforcement. */
export function providerStatus(
  row: GuardrailProviderRow,
  enforcement: Enforcement,
): ProviderStatus {
  if (!row.enabled) return "paused";
  if (enforcement.state === "enforced" || enforcement.state === "inert") {
    if (enforcement.overridden?.id === row.id) return "overridden";
    if (enforcement.provider?.id === row.id) return enforcement.state;
  }
  // whatever the effective config says, the gateway never runs this stage
  if (row.stage === "post_call") return "inert";
  return "unknown";
}
