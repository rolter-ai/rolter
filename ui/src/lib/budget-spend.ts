import type { BudgetSpendRow } from "@/lib/api";

/** The share of the cap at which the bar turns to a warning. */
export const SPEND_WARN_AT = 0.8;

export type SpendTone = "ok" | "warning" | "danger";

export interface SpendFigures {
  spent: number;
  limit: number;
  /** what is left of the cap, never negative */
  remaining: number;
  /** spent over the cap, clamped to 0..1 for the bar */
  fraction: number;
  /** the same share unclamped, for the text: 112% is worth saying */
  ratio: number;
  tone: SpendTone;
}

/**
 * What the card shows for a counter, or null when there is no figure to show:
 * the counter was unreadable, or the row carries a number that is not one.
 * A cap of zero is spent by definition, since the gateway refuses at
 * `spend >= limit`.
 */
export function spendFigures(
  row: BudgetSpendRow | undefined,
  limitText: string,
): SpendFigures | null {
  if (!row || row.spent_usd === null) return null;
  const spent = Number(row.spent_usd);
  const limit = Number(limitText);
  if (!Number.isFinite(spent) || !Number.isFinite(limit)) return null;
  const ratio = limit > 0 ? spent / limit : 1;
  const tone: SpendTone = spent >= limit ? "danger" : ratio >= SPEND_WARN_AT ? "warning" : "ok";
  return {
    spent,
    limit,
    remaining: Math.max(0, limit - spent),
    fraction: Math.min(1, Math.max(0, ratio)),
    ratio,
    tone,
  };
}
