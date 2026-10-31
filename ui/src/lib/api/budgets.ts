// budgets and rate limits (crates/rolter-control/src/crud/budgets.rs and rate_limits.rs)
import { getJson, sendJson } from "./request";

// every scope a budget or rate limit may hang off, in the order of the
// `SCOPE_TYPES` allowlist in `crates/rolter-control/src/crud/budgets.rs`. business unit
// and customer are the governance dimensions a key's spend rolls up to (#539);
// leaving them out made those budgets uncreatable from the dashboard
export const SCOPE_TYPES = [
  "org",
  "team",
  "project",
  "virtual_key",
  "business_unit",
  "customer",
  "user",
] as const;

/**
 * A budget or rate limit that counts every key one person minted (#1830). The
 * Limits screen creates one by picking the person and, optionally, the org or
 * project to narrow it to (#3282).
 */
export const USER_SCOPE = "user";

/** the narrowing a user-scope row carries; both null counts every key the person minted */
export interface UserScopeNarrowing {
  within_org_id?: string | null;
  within_project_id?: string | null;
}

export type ScopeType = (typeof SCOPE_TYPES)[number];

/**
 * A budget's own answer to traffic against a model with no price row, which
 * accrues zero spend and so can never exhaust the budget. `null` inherits the
 * deployment-wide setting; an override can only tighten it, because the gateway
 * resolves most-restrictive-wins across the scope chain (#996).
 */
export const UNPRICED_POLICIES = ["ignore", "warn", "block"] as const;

export type UnpricedPolicy = (typeof UNPRICED_POLICIES)[number];

export interface BudgetRow extends UserScopeNarrowing {
  id: string;
  scope_type: string;
  scope_id: string;
  /// decimal, returned as text
  limit_usd: string;
  period: string;
  unpriced_policy: UnpricedPolicy | null;
  created_at: string;
  /// the virtual key's name (its prefix when unnamed) on an org-wide read; null
  /// on other scopes, and absent on a per-scope read (#3128)
  scope_name?: string | null;
}

export interface CreateBudgetInput extends UserScopeNarrowing {
  scope_type: string;
  scope_id: string;
  limit_usd: string;
  period?: string;
  unpriced_policy?: UnpricedPolicy | null;
}

export function fetchBudgets(scopeType: string, scopeId: string): Promise<BudgetRow[]> {
  return getJson<BudgetRow[]>(
    `/api/v1/budgets?scope_type=${encodeURIComponent(scopeType)}&scope_id=${encodeURIComponent(scopeId)}`,
  );
}

/**
 * Every budget in the org the caller may read, at any scope (#2093). Each row
 * carries its own `scope_type` and `scope_id`; the rows are the ones the
 * per-scope read would have answered, never more.
 */
export function fetchOrgBudgets(orgId: string): Promise<BudgetRow[]> {
  return getJson<BudgetRow[]>(`/api/v1/orgs/${encodeURIComponent(orgId)}/budgets`);
}

/// how the control plane answered a spend read (#2091)
export type BudgetSpendStatus = "ok" | "not_configured" | "unavailable";

export interface BudgetSpendRow {
  /// the budget row's id
  id: string;
  /// the gateway's counter for the current window as text, or null when it
  /// could not be read; null is never a zero
  spent_usd: string | null;
  /// start of the next window, null for a budget that never resets
  resets_at: string | null;
}

export interface BudgetSpendReport {
  status: BudgetSpendStatus;
  budgets: BudgetSpendRow[];
}

/**
 * The spend so far against each budget on a scope. A body that is not a report
 * reads as unavailable rather than as an empty one, so a malformed answer can
 * never show a budget as untouched.
 */
export async function fetchBudgetSpend(
  scopeType: string,
  scopeId: string,
): Promise<BudgetSpendReport> {
  const body = await getJson<Partial<BudgetSpendReport> | null>(
    `/api/v1/budgets/spend?scope_type=${encodeURIComponent(scopeType)}&scope_id=${encodeURIComponent(scopeId)}`,
  );
  if (!body || !Array.isArray(body.budgets)) return { status: "unavailable", budgets: [] };
  return { status: body.status ?? "unavailable", budgets: body.budgets };
}

/**
 * The spend so far against every budget in the org the caller may read (#3128),
 * the batch form of `fetchBudgetSpend`; same body, same malformed-is-unavailable rule.
 */
export async function fetchOrgBudgetSpend(orgId: string): Promise<BudgetSpendReport> {
  const body = await getJson<Partial<BudgetSpendReport> | null>(
    `/api/v1/orgs/${encodeURIComponent(orgId)}/budgets/spend`,
  );
  if (!body || !Array.isArray(body.budgets)) return { status: "unavailable", budgets: [] };
  return { status: body.status ?? "unavailable", budgets: body.budgets };
}

export function createBudget(input: CreateBudgetInput): Promise<BudgetRow> {
  return sendJson<BudgetRow>("POST", "/api/v1/budgets", input);
}

/**
 * A partial edit of a budget (#1285). An omitted field is left as it is;
 * `unpriced_policy: null` drops the override so the budget inherits the
 * deployment-wide setting again. The scope is not editable.
 */
export interface UpdateBudgetInput {
  limit_usd?: string;
  period?: string;
  unpriced_policy?: UnpricedPolicy | null;
}

export function updateBudget(id: string, input: UpdateBudgetInput): Promise<BudgetRow> {
  return sendJson<BudgetRow>("PATCH", `/api/v1/budgets/${id}`, input);
}

export function deleteBudget(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/budgets/${id}`);
}

export interface RateLimitRow extends UserScopeNarrowing {
  id: string;
  scope_type: string;
  scope_id: string;
  rpm?: number | null;
  tpm?: number | null;
  created_at: string;
  /// see `BudgetRow.scope_name`
  scope_name?: string | null;
}

export interface CreateRateLimitInput extends UserScopeNarrowing {
  scope_type: string;
  scope_id: string;
  rpm?: number;
  tpm?: number;
}

export function fetchRateLimits(scopeType: string, scopeId: string): Promise<RateLimitRow[]> {
  return getJson<RateLimitRow[]>(
    `/api/v1/rate-limits?scope_type=${encodeURIComponent(scopeType)}&scope_id=${encodeURIComponent(scopeId)}`,
  );
}

/** Every rate limit in the org the caller may read, at any scope; see `fetchOrgBudgets`. */
export function fetchOrgRateLimits(orgId: string): Promise<RateLimitRow[]> {
  return getJson<RateLimitRow[]>(`/api/v1/orgs/${encodeURIComponent(orgId)}/rate-limits`);
}

export function createRateLimit(input: CreateRateLimitInput): Promise<RateLimitRow> {
  return sendJson<RateLimitRow>("POST", "/api/v1/rate-limits", input);
}

/**
 * A partial edit of a rate limit (#1285): an omitted cap is left alone and
 * `null` lifts it. The control plane refuses an edit that would leave neither
 * cap, since such a limit admits everything.
 */
export interface UpdateRateLimitInput {
  rpm?: number | null;
  tpm?: number | null;
}

export function updateRateLimit(id: string, input: UpdateRateLimitInput): Promise<RateLimitRow> {
  return sendJson<RateLimitRow>("PATCH", `/api/v1/rate-limits/${id}`, input);
}

export function deleteRateLimit(id: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/rate-limits/${id}`);
}
