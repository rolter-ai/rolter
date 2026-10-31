import type { MyBudgetRow, UserScopeNarrowing } from "@/lib/api";
import { spendFigures, type SpendFigures } from "@/lib/budget-spend";

/** which of a person's keys a user-scope cap counts */
export type CapReach = "all" | "org" | "project";

/**
 * What a user-scope budget or rate limit covers: every key the person minted,
 * or only those minted in one org or one project. The project is the narrower
 * of the two, so it wins if a row carries both.
 */
export function capReach(row: UserScopeNarrowing): CapReach {
  if (row.within_project_id) return "project";
  if (row.within_org_id) return "org";
  return "all";
}

/**
 * The figures behind one of the member's own budgets, or null when the spend is
 * not known: the report was not `ok`, or the counter could not be read. Unknown
 * is never zero, so a budget with no figures must not be drawn as untouched.
 */
export function myBudgetFigures(
  budget: MyBudgetRow,
  status: "ok" | "not_configured" | "unavailable",
): SpendFigures | null {
  if (status !== "ok") return null;
  return spendFigures(budget, budget.limit_usd);
}
