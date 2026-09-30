/** The window a budget resets on, as the dashboard names it. */
export type PeriodKind = "daily" | "monthly" | "total";

// a budget's `period` is free text, and these are the spellings the gateway
// reads: `parse_period` in rolter-store takes `daily`, `1d` and `24h` as a day,
// `total`, `lifetime` and `all` as a lifetime cap, and everything else as a
// month. `30d` is the dashboard's own default for that last case, so it is
// named here too. anything further is left to the caller to print as stored —
// `7d` is enforced as a month but was never a month's name (#1902)
const KINDS = new Map<string, PeriodKind>([
  ["daily", "daily"],
  ["1d", "daily"],
  ["24h", "daily"],
  ["monthly", "monthly"],
  ["30d", "monthly"],
  ["total", "total"],
  ["lifetime", "total"],
  ["all", "total"],
]);

/** The window `period` names, or `null` for text the dashboard has no name for. */
export function periodKind(period: string): PeriodKind | null {
  return KINDS.get(period.trim().toLowerCase()) ?? null;
}
