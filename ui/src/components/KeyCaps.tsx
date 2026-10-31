import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import {
  fetchBudgetSpend,
  fetchBudgets,
  fetchRateLimits,
  type BudgetRow,
  type BudgetSpendReport,
  type RateLimitRow,
} from "@/lib/api";
import { periodKind } from "@/lib/budget-period";
import { spendFigures } from "@/lib/budget-spend";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";

// what a key's cap is and what is left of it (#1171). the caps are ordinary
// budgets and rate limits scoped to the key (`scope_type: "virtual_key"`), so
// the figures come from the budgets API the Limits screen reads

/**
 * "loading" while the spend read is in flight, null when it failed outright.
 * The key list reads one org-wide report for every row (#3128)
 */
export type KeySpend = BudgetSpendReport | "loading" | null;

/** the rows of an org-wide read that belong to one key */
export function capsOfKey<T extends { scope_type: string; scope_id: string }>(
  rows: T[] | undefined,
  keyId: string,
): T[] {
  return (Array.isArray(rows) ? rows : []).filter(
    (r) => r.scope_type === "virtual_key" && r.scope_id === keyId,
  );
}

export function KeyCaps({
  budgets,
  rateLimits,
  spend,
}: {
  budgets: BudgetRow[];
  rateLimits: RateLimitRow[];
  spend: KeySpend;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  if (budgets.length === 0 && rateLimits.length === 0) return null;
  const money = (value: number) => fmt.currency(value, currency);
  return (
    <ul className="space-y-0.5 text-2xs text-muted-foreground" data-key-caps>
      {budgets.map((budget) => {
        const kind = periodKind(budget.period);
        const period = kind ? t(`pages.limits.periods.${kind}`) : budget.period;
        const row =
          typeof spend === "object" ? spend?.budgets.find((b) => b.id === budget.id) : undefined;
        const figures = spendFigures(row, budget.limit_usd);
        return (
          <li key={budget.id}>
            <span className="text-foreground">
              {t("keyCaps.spend", { amount: money(Number(budget.limit_usd)), period })}
            </span>
            {spend !== "loading" && (
              <>
                {" · "}
                {figures ? (
                  <span data-spend={figures.tone}>
                    {figures.tone === "danger"
                      ? t("keyCaps.exhausted", { spent: money(figures.spent) })
                      : t("keyCaps.left", {
                          spent: money(figures.spent),
                          remaining: money(figures.remaining),
                        })}
                    {row?.resets_at &&
                      ` · ${t("keyCaps.resets", { when: fmt.dateTime(row.resets_at) })}`}
                  </span>
                ) : (
                  <span data-spend="unknown">{t("keyCaps.unknown")}</span>
                )}
              </>
            )}
          </li>
        );
      })}
      {rateLimits.map((limit) => (
        <li key={limit.id}>
          <span className="text-foreground">
            {t("keyCaps.rate", {
              caps:
                [
                  limit.rpm != null
                    ? `${fmt.number(limit.rpm)} ${t("pages.limits.units.rpm")}`
                    : null,
                  limit.tpm != null
                    ? `${fmt.number(limit.tpm)} ${t("pages.limits.units.tpm")}`
                    : null,
                ]
                  .filter(Boolean)
                  .join(" · ") || t("pages.limits.noCaps"),
            })}
          </span>
        </li>
      ))}
    </ul>
  );
}

/**
 * The caps of one key the signed-in account owns, read for that key alone.
 *
 * Own keys are few, so three small reads per card are cheaper than guessing an
 * org the self-service panel does not know. A read that fails (a member without
 * budget access gets a 403 that asking again will not improve) draws nothing
 * rather than an error on every card, and a spend read that comes back
 * `not_configured` or `unavailable` is "unknown", never zero.
 */
export function OwnedKeyCaps({ keyId }: { keyId: string }) {
  const budgets = useQuery({
    queryKey: ["budgets", "virtual_key", keyId],
    queryFn: () => fetchBudgets("virtual_key", keyId),
    retry: false,
  });
  const rateLimits = useQuery({
    queryKey: ["rate-limits", "virtual_key", keyId],
    queryFn: () => fetchRateLimits("virtual_key", keyId),
    retry: false,
  });
  const hasBudget = (budgets.data?.length ?? 0) > 0;
  const spend = useQuery({
    queryKey: ["budgets", "virtual_key", keyId, "spend"],
    queryFn: () => fetchBudgetSpend("virtual_key", keyId),
    enabled: hasBudget,
    retry: false,
  });
  if (!budgets.data && !rateLimits.data) return null;
  return (
    <KeyCaps
      budgets={capsOfKey(budgets.data, keyId)}
      rateLimits={capsOfKey(rateLimits.data, keyId)}
      spend={spend.isPending ? "loading" : spend.isError ? null : spend.data}
    />
  );
}
