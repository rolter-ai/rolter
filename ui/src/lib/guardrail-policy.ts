import type { GuardrailRuleRow } from "@/lib/api";

/**
 * One rule of the effective `guardrails` section (`GET /api/v1/config`), as
 * `GuardrailRule` in `crates/rolter-core/src/guardrails.rs` serializes it.
 * `builtin`, `pattern` and `replacement` are left out when unset.
 */
export interface EffectiveRule {
  name: string;
  builtin?: NonNullable<GuardrailRuleRow["builtin"]>;
  pattern?: string;
  stage: GuardrailRuleRow["stage"];
  action: GuardrailRuleRow["action"];
  replacement?: string;
  include_system: boolean;
}

/**
 * One route's `advanced.guardrails` override, as `RouteGuardrails` in
 * `crates/rolter-core/src/guardrails.rs` serializes it. Both lists are left
 * out when empty. Config-file routes and database routes carry it alike, the
 * latter through the `advanced` JSON column.
 */
export interface RouteOverride {
  /** the route's public model name */
  model: string;
  /** rules that do not apply on this route */
  disable: string[];
  /** rules that apply on this route, winning a conflict with `disable` */
  enable: string[];
}

/**
 * The guardrail policy the control plane hands every gateway, after the config
 * file and the dashboard's rules are merged (`MergedConfigStore::load` in
 * `crates/rolter-store/src/lib.rs`).
 */
export interface EffectivePolicy {
  /** the `guardrails` feature flag: off, no rule inspects traffic */
  on: boolean;
  streaming: "reject" | "passthrough";
  /** config-file rules first, in file order, then the dashboard's enabled rows */
  rules: EffectiveRule[];
  /** the routes that override the rule set, in config order */
  routes: RouteOverride[];
}

/** What one dashboard row's card says about it. */
export type RowState =
  /** the row is in the effective policy and the flag is on */
  | { state: "enforced" }
  /** the row is in the effective policy, but the `guardrails` flag is off */
  | { state: "off" }
  /**
   * a config-file rule has the row's name, so the store dropped the row and
   * runs the file's rule instead
   */
  | { state: "overridden"; by: EffectiveRule }
  /**
   * the row is paused. `clash` is a config-file rule under its name, which
   * would keep the row out of the policy if it were resumed
   */
  | { state: "paused"; clash: EffectiveRule | null }
  /** the effective policy could not be read, or it does not carry the row */
  | { state: "unknown" };

export interface PolicyResolution {
  /** `null` when the effective config could not be read */
  policy: EffectivePolicy | null;
  /** keyed by row id */
  rows: Map<string, RowState>;
  /** effective rules no dashboard row accounts for, so the config file's */
  fileRules: EffectiveRule[];
  /**
   * The routes that switch each effective rule off, keyed by rule name and
   * sorted. A rule every route runs has no entry, and so does a name an
   * override gives that no effective rule has.
   */
  offRoutes: Map<string, string[]>;
}

const STAGES = new Set(["pre_call", "post_call"]);
const ACTIONS = new Set(["annotate", "block", "redact"]);
const BUILTINS = new Set(["email", "phone", "api_token", "payment_card"]);
const STREAMING = new Set(["reject", "passthrough"]);

const isObject = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === "object" && !Array.isArray(value);

const optionalString = (value: unknown) => value === undefined || typeof value === "string";

function readRule(value: unknown): EffectiveRule | null {
  if (!isObject(value)) return null;
  if (
    typeof value.name !== "string" ||
    typeof value.stage !== "string" ||
    !STAGES.has(value.stage) ||
    typeof value.action !== "string" ||
    !ACTIONS.has(value.action) ||
    typeof value.include_system !== "boolean" ||
    !(
      value.builtin === undefined ||
      (typeof value.builtin === "string" && BUILTINS.has(value.builtin))
    ) ||
    !optionalString(value.pattern) ||
    !optionalString(value.replacement)
  ) {
    return null;
  }
  return value as unknown as EffectiveRule;
}

const isStrings = (value: unknown): value is string[] =>
  Array.isArray(value) && value.every((item) => typeof item === "string");

/**
 * A route's override, `undefined` for a route that sets none, or `null` for a
 * shape this screen does not recognise.
 */
function readRoute(value: unknown): RouteOverride | undefined | null {
  if (!isObject(value) || typeof value.model !== "string") return null;
  if (value.advanced === undefined) return undefined;
  if (!isObject(value.advanced)) return null;
  const section = value.advanced.guardrails;
  if (section === undefined) return undefined;
  if (!isObject(section)) return null;
  const disable = section.disable ?? [];
  const enable = section.enable ?? [];
  if (!isStrings(disable) || !isStrings(enable)) return null;
  if (disable.length === 0 && enable.length === 0) return undefined;
  return { model: value.model, disable, enable };
}

/**
 * Read the effective guardrail policy out of the config document, or `null`
 * when it cannot be trusted. The answer feeds a security status, so a shape
 * this screen does not recognise is treated as unreadable rather than guessed
 * at.
 *
 * The store sets `guardrails.enabled` from the flag on every load, so the two
 * always agree in a document it built. They disagree in the default document
 * `get_config` answers with when the store fails (the section off, the flag
 * on, no rules), which is exactly the one this must not report as the policy
 * (#2248 makes the endpoint fail instead).
 */
export function readEffectivePolicy(config: unknown): EffectivePolicy | null {
  if (!isObject(config)) return null;
  const flags = config.feature_flags;
  const section = config.guardrails;
  if (!isObject(flags) || typeof flags.guardrails !== "boolean") return null;
  if (!isObject(section) || section.enabled !== flags.guardrails) return null;
  if (typeof section.streaming_post_call !== "string") return null;
  if (!STREAMING.has(section.streaming_post_call)) return null;
  const raw = section.rules ?? [];
  if (!Array.isArray(raw)) return null;
  const rules: EffectiveRule[] = [];
  for (const item of raw) {
    const rule = readRule(item);
    if (!rule) return null;
    rules.push(rule);
  }
  // an override switches a rule off, so a route list that cannot be read would
  // leave every card claiming more coverage than the gateway gives
  const rawRoutes = config.routes ?? [];
  if (!Array.isArray(rawRoutes)) return null;
  const routes: RouteOverride[] = [];
  for (const item of rawRoutes) {
    const route = readRoute(item);
    if (route === null) return null;
    if (route) routes.push(route);
  }
  return {
    on: flags.guardrails,
    streaming: section.streaming_post_call as EffectivePolicy["streaming"],
    rules,
    routes,
  };
}

/**
 * Whether `route` switches the rule named `name` off, as
 * `RouteGuardrails::allows` decides it: `enable` wins a conflict, so a route
 * naming the rule in both lists runs it.
 *
 * The gateway compares the override's names with the rule's name trimmed
 * (`CompiledRule::from_config`), so an override has to name it that way too.
 */
export function switchesOff(route: RouteOverride, name: string): boolean {
  const rule = name.trim();
  if (route.enable.includes(rule)) return false;
  return route.disable.includes(rule);
}

/**
 * The routes that switch each effective rule off, keyed by rule name. Only the
 * policy's own rules are looked up, so an override naming a rule that no
 * longer exists shows nowhere; the config validator reports that one.
 */
function offRoutesFor(policy: EffectivePolicy): Map<string, string[]> {
  const off = new Map<string, string[]>();
  for (const rule of policy.rules) {
    // a set: the document is not validated, so a duplicate route model the
    // snapshot would refuse can still be listed twice here
    const models = new Set(
      policy.routes.filter((route) => switchesOff(route, rule.name)).map((route) => route.model),
    );
    if (models.size > 0) off.set(rule.name, [...models].sort());
  }
  return off;
}

/**
 * Whether the effective rule is this row, field for field, as the postgres
 * store maps a row into one. Anything that differs means the config file
 * supplied the rule under the row's name.
 */
function isRow(row: GuardrailRuleRow, rule: EffectiveRule): boolean {
  return (
    (row.builtin ?? null) === (rule.builtin ?? null) &&
    (row.pattern ?? null) === (rule.pattern ?? null) &&
    row.stage === rule.stage &&
    row.action === rule.action &&
    (row.replacement ?? null) === (rule.replacement ?? null) &&
    row.include_system === rule.include_system
  );
}

/**
 * Resolve what each dashboard row does on the gateway, and which effective
 * rules come from the config file, from the rows and the effective policy.
 *
 * The effective config names no rule's source. A config-file rule wins a name
 * clash, so an effective rule is the file's when no row has its name, when the
 * row with its name is paused (a paused row never reaches the policy), or when
 * it differs from that row. One identical to an enabled row cannot be told
 * apart from it and is credited to the row; enforcement is the same either way
 * (#2249 asks the API to report the source).
 */
export function resolvePolicy(
  rows: GuardrailRuleRow[],
  policy: EffectivePolicy | null,
): PolicyResolution {
  const states = new Map<string, RowState>();
  if (!policy) {
    for (const row of rows) {
      states.set(row.id, row.enabled ? { state: "unknown" } : { state: "paused", clash: null });
    }
    return { policy, rows: states, fileRules: [], offRoutes: new Map() };
  }
  const byName = new Map(policy.rules.map((rule) => [rule.name, rule]));
  const credited = new Set<string>();
  for (const row of rows) {
    const rule = byName.get(row.name) ?? null;
    if (!row.enabled) {
      states.set(row.id, { state: "paused", clash: rule });
    } else if (!rule) {
      // an enabled row always reaches the policy, so its absence means the
      // two reads disagree
      states.set(row.id, { state: "unknown" });
    } else if (!isRow(row, rule)) {
      states.set(row.id, { state: "overridden", by: rule });
    } else {
      credited.add(row.name);
      states.set(row.id, { state: policy.on ? "enforced" : "off" });
    }
  }
  return {
    policy,
    rows: states,
    fileRules: policy.rules.filter((rule) => !credited.has(rule.name)),
    offRoutes: offRoutesFor(policy),
  };
}
