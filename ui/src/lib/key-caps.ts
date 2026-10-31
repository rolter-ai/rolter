// the optional spend and rate caps a virtual key is minted with (#1171). the
// rules mirror `KeyCapsBody::validate` in `crates/rolter-control/src/crud/budgets.rs`
// and `validate_rate_limit_caps`, so an operator learns a refusal before the
// round trip; the server stays the authority and answers a 400 `invalid_field`
import type { KeyBudgetCap, KeyRateCap } from "@/lib/api/virtual-keys";
import type { PeriodKind } from "@/lib/budget-period";

/** the column is `numeric(12,4)`; mirrors `LIMIT_USD_CEILING` (exclusive) */
export const LIMIT_USD_CEILING = 99_999_999.999_95;
/** the largest value the `rpm` / `tpm` columns (`i32`) hold */
export const MAX_RATE_CAP = 2_147_483_647;

/** what the mint sheet holds while the operator decides; text, as typed */
export interface CapDraft {
  budgetOn: boolean;
  limit: string;
  period: PeriodKind;
  rateOn: boolean;
  rpm: string;
  tpm: string;
}

export const EMPTY_CAPS: CapDraft = {
  budgetOn: false,
  limit: "",
  period: "monthly",
  rateOn: false,
  rpm: "",
  tpm: "",
};

/** `true` once the draft would put a cap on the key, which is what "dirty" means here */
export function capsTouched(draft: CapDraft): boolean {
  return draft.budgetOn || draft.rateOn;
}

export type LimitProblem = "blank" | "invalid" | "range";

/** `null` when the text is a cap the control plane would accept */
export function limitProblem(text: string): LimitProblem | null {
  const trimmed = text.trim();
  if (trimmed === "") return "blank";
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return "invalid";
  return Number(trimmed) < LIMIT_USD_CEILING ? null : "range";
}

export type RateProblem = "invalid" | "range";

/** an empty box is no cap on that axis, which is allowed; `null` is fine */
export function rateCapProblem(text: string): RateProblem | null {
  const trimmed = text.trim();
  if (trimmed === "") return null;
  if (!/^\d+$/.test(trimmed)) return "invalid";
  const value = Number(trimmed);
  return value >= 1 && value <= MAX_RATE_CAP ? null : "range";
}

export interface CapProblems {
  limit: LimitProblem | null;
  rpm: RateProblem | null;
  tpm: RateProblem | null;
  /** a rate cap with neither box filled in admits everything */
  rateEmpty: boolean;
}

export function capProblems(draft: CapDraft): CapProblems {
  const rpm = draft.rateOn ? rateCapProblem(draft.rpm) : null;
  const tpm = draft.rateOn ? rateCapProblem(draft.tpm) : null;
  return {
    limit: draft.budgetOn ? limitProblem(draft.limit) : null,
    rpm,
    tpm,
    rateEmpty: draft.rateOn && draft.rpm.trim() === "" && draft.tpm.trim() === "",
  };
}

/** whether the sheet may mint with these caps */
export function capsValid(draft: CapDraft): boolean {
  const p = capProblems(draft);
  return !p.limit && !p.rpm && !p.tpm && !p.rateEmpty;
}

/** the two optional body fields; absent when the cap is switched off */
export function capsToInput(draft: CapDraft): { budget?: KeyBudgetCap; rate_limit?: KeyRateCap } {
  const out: { budget?: KeyBudgetCap; rate_limit?: KeyRateCap } = {};
  if (draft.budgetOn) out.budget = { limit_usd: draft.limit.trim(), period: draft.period };
  if (draft.rateOn) {
    const rate: KeyRateCap = {};
    if (draft.rpm.trim() !== "") rate.rpm = Number(draft.rpm.trim());
    if (draft.tpm.trim() !== "") rate.tpm = Number(draft.tpm.trim());
    out.rate_limit = rate;
  }
  return out;
}
