// the savings advisor (crates/rolter-control/src/savings.rs, #1581)
import { getAnalytics, type AnalyticsWindow } from "./analytics";

/** a projected figure as the pessimistic and the optimistic end of a range, never one number */
export interface MoneyRange {
  low: number;
  high: number;
}

/** requests and what they cost, in the report's currency */
export interface SavingsSlice {
  requests: number;
  cost_usd: number;
}

export interface SavingsSpendRow extends SavingsSlice {
  /** requests that ran against a model with no price, so `cost_usd` is not a cost for them */
  unpriced_requests: number;
}

export interface SavingsSpend extends SavingsSpendRow {
  /** requests that took more than one upstream attempt (retries and failovers together) */
  multi_attempt: SavingsSlice;
  /** requests that ended in an error, and what was spent on them anyway */
  failed: SavingsSlice;
  cache: { hits: number; misses: number; miss_cost_usd: number };
  by_route: (SavingsSpendRow & { route: string })[];
  by_model: (SavingsSpendRow & { model: string })[];
  by_caller: (SavingsSpendRow & {
    project_id: string;
    virtual_key_id: string;
    /**
     * the name given to the key, read by the control plane for every caller
     * whichever project it is in (#3499); `null` for a key with no name or one
     * that is gone, and absent from a control plane that predates it
     */
    key_name?: string | null;
  })[];
}

export interface SavingsOption {
  /** another model in the same route's pool, with a known price */
  model: string;
  projected_cost_usd: MoneyRange;
  /** the recorded cost minus the projection, so `low` is the pessimistic saving and can be negative */
  savings_usd: MoneyRange;
}

export interface SavingsCandidate {
  route: string;
  current_model: string;
  requests: number;
  current_cost_usd: number;
  candidates: SavingsOption[];
}

export interface SavingsReport {
  /** the base currency every money figure is in */
  currency: string;
  /** what the report says about itself; the control plane's own words */
  limits: string[];
  assumptions: {
    max_prompt_tokens: number;
    max_completion_tokens: number;
    output_margin: number;
  };
  spend: SavingsSpend;
  substitutable: {
    requests: number;
    current_cost_usd: number;
    projected_savings_usd: MoneyRange;
  };
  candidates: SavingsCandidate[];
  excluded: {
    /** requests left out, by reason */
    requests: Record<string, number>;
    /** pool models that could not be considered because they have no price */
    unknown_candidate_prices: string[];
  };
}

/** the scope and thresholds the endpoint accepts; every one is optional */
export interface SavingsQuery extends AnalyticsWindow {
  org?: string;
  project?: string;
  route?: string;
  key?: string;
  max_prompt_tokens?: number;
  max_completion_tokens?: number;
  output_margin?: number;
}

export function fetchSavings(query: SavingsQuery = {}): Promise<SavingsReport> {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value !== undefined && value !== "") params.set(name, String(value));
  }
  const qs = params.toString();
  return getAnalytics<SavingsReport>(`/api/v1/analytics/savings${qs ? `?${qs}` : ""}`);
}
