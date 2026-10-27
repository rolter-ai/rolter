import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { SettingsPanel } from "@/components/ui/settings-panel";
import {
  fetchMyBudgets,
  isOpenModeNoSession,
  type MyBudgetRow,
  type MyRateLimitRow,
} from "@/lib/api";
import { periodKind } from "@/lib/budget-period";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { capReach, myBudgetFigures } from "@/lib/my-budget";

/**
 * The member's own budget (#1830): the caps that count every key they minted,
 * and what is left of each.
 *
 * Read-only and hidden when the account has no such cap, since most members
 * have none and an empty "Your budget" panel would suggest they could set one
 * here. Spend the control plane could not read is "unknown", never zero: a
 * budget drawn as untouched is the one failure this panel must not have.
 */
export const MY_BUDGETS_KEY = "my-budgets";

export function MyBudgets() {
  const { t } = useTranslation();
  const report = useQuery({ queryKey: [MY_BUDGETS_KEY], queryFn: fetchMyBudgets, retry: false });

  // open mode has no account, and the keys panel already says so
  if (isOpenModeNoSession(report.error)) return null;
  if (report.isLoading) {
    return <PanelSkeleton panels={1} height={132} className="w-full" />;
  }
  if (report.error) {
    return (
      // load-error-allow: a panel that is absent when the account has no caps; there is no list to be empty
      <LoadError
        error={report.error}
        resource={t("errors.resources.yourBudget")}
        onRetry={() => report.refetch()}
        target="own-budget"
      />
    );
  }
  const {
    status,
    budgets,
    rate_limits: rateLimits,
  } = report.data ?? {
    status: "unavailable" as const,
    budgets: [],
    rate_limits: [],
  };
  if (budgets.length === 0 && rateLimits.length === 0) return null;

  return (
    <SettingsPanel
      title={t("account.budget.title")}
      description={t("account.budget.description")}
      data-testid="own-budget"
    >
      {budgets.length > 0 && (
        <section aria-labelledby="own-budget-spend" className="space-y-2">
          <h3 id="own-budget-spend" className="text-sm font-medium text-muted-foreground">
            {t("account.budget.spendHeading")}
          </h3>
          <ul className="space-y-2">
            {budgets.map((budget) => (
              <BudgetItem key={budget.id} budget={budget} status={status} />
            ))}
          </ul>
        </section>
      )}
      {rateLimits.length > 0 && (
        <section aria-labelledby="own-budget-rate" className="space-y-2">
          <h3 id="own-budget-rate" className="text-sm font-medium text-muted-foreground">
            {t("account.budget.rateHeading")}
          </h3>
          <ul className="space-y-2">
            {rateLimits.map((limit) => (
              <RateItem key={limit.id} limit={limit} />
            ))}
          </ul>
        </section>
      )}
    </SettingsPanel>
  );
}

function BudgetItem({
  budget,
  status,
}: {
  budget: MyBudgetRow;
  status: "ok" | "not_configured" | "unavailable";
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const money = (value: number) => fmt.currency(value, currency);
  const kind = periodKind(budget.period);
  // a period the dashboard has no name for is shown as stored
  const period = kind ? t(`pages.limits.periods.${kind}`) : budget.period;
  const figures = myBudgetFigures(budget, status);

  return (
    <li className="space-y-1 rounded-lg border border-[color:var(--border-subtle)] px-3.5 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="font-mono text-lg font-medium">{money(Number(budget.limit_usd))}</span>
        <Badge tone="outline">{period}</Badge>
        {figures?.tone === "danger" && <Badge tone="danger">{t("account.budget.exhausted")}</Badge>}
      </div>
      <p className="text-sm" data-spend={figures ? figures.tone : "unknown"}>
        {figures ? (
          <>
            {t("account.budget.spent", { spent: money(figures.spent) })}
            {" · "}
            {t("account.budget.left", { remaining: money(figures.remaining) })}
            {budget.resets_at &&
              ` · ${t("account.budget.resets", { when: fmt.dateTime(budget.resets_at) })}`}
          </>
        ) : (
          <span className="text-muted-foreground">{t("account.budget.unknown")}</span>
        )}
      </p>
      <p className="text-xs text-muted-foreground">
        {t(`account.budget.reach.${capReach(budget)}`)}
      </p>
    </li>
  );
}

function RateItem({ limit }: { limit: MyRateLimitRow }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const caps =
    [
      limit.rpm != null ? `${fmt.number(limit.rpm)} ${t("pages.limits.units.rpm")}` : null,
      limit.tpm != null ? `${fmt.number(limit.tpm)} ${t("pages.limits.units.tpm")}` : null,
    ]
      .filter(Boolean)
      .join(" · ") || t("pages.limits.noCaps");
  return (
    <li className="space-y-1 rounded-lg border border-[color:var(--border-subtle)] px-3.5 py-3">
      <span className="font-mono text-sm font-medium">{caps}</span>
      <p className="text-xs text-muted-foreground">
        {t(`account.budget.reach.${capReach(limit)}`)}
      </p>
    </li>
  );
}
