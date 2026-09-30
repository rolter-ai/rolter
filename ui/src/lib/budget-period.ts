/** The window a budget resets on, as the dashboard names it. */
export type PeriodKind = "daily" | "monthly" | "total";

/**
 * The windows the gateway has, in the order the picker offers them. They are
 * calendar windows in UTC: there are no rolling windows, so a budget cannot
 * count "the last 7 days" (#1902).
 */
export const PERIOD_KINDS: readonly PeriodKind[] = ["daily", "monthly", "total"];

// every spelling of a period the gateway reads, mirroring
// `BudgetPeriod::SPELLINGS` in `crates/rolter-core/src/config.rs`, which is
// also what the control plane accepts on create and edit. the shorthands are
// what budgets stored through the api have always used, `30d` being the column
// default; it means the calendar month, not thirty rolling days.
// `budget-period.test.ts` fails when the two tables disagree
export const PERIOD_SPELLINGS: ReadonlyMap<string, PeriodKind> = new Map([
  ["daily", "daily"],
  ["1d", "daily"],
  ["24h", "daily"],
  ["monthly", "monthly"],
  ["30d", "monthly"],
  ["total", "total"],
  ["lifetime", "total"],
  ["all", "total"],
]);

/**
 * The window `period` names, or `null` for text the gateway does not recognise.
 *
 * The control plane refuses such a value now, but a budget stored before it did
 * keeps it, and the gateway enforces that budget as monthly whatever the row
 * says. `7d` is the example: it was never a month's name, so the caller prints
 * it as stored and says what it is enforced as.
 */
export function periodKind(period: string): PeriodKind | null {
  return PERIOD_SPELLINGS.get(period.trim().toLowerCase()) ?? null;
}
