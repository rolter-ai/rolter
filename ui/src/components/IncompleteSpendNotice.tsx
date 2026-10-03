import { CircleDollarSign } from "lucide-react";
import { useTranslation } from "react-i18next";

/**
 * Says that a spend figure is a floor rather than a total (#969).
 *
 * Traffic against a model with no price row contributes zero, which reads
 * exactly like traffic that genuinely cost nothing. An operator can run a fleet
 * for a month, see $0.00, and conclude spend is under control — there is
 * nothing in the number itself to prompt a second look. This is that prompt.
 */
export function IncompleteSpendNotice({
  requests,
  models,
}: {
  /** requests in the window recorded with no price */
  requests: number;
  /** how many distinct models those requests hit */
  models: number;
}) {
  const { t } = useTranslation();
  if (requests <= 0) return null;
  return (
    <div
      role="status"
      className="flex items-start gap-2.5 rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--red-tint)] px-4 py-3"
    >
      <CircleDollarSign
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-warning-text)]"
      />
      <p className="min-w-0 text-sm text-muted-foreground">
        <span className="font-medium text-foreground">
          {t("pages.dashboard.unpriced.title", { count: requests })}
        </span>{" "}
        {t("pages.dashboard.unpriced.body", { count: models })}
      </p>
    </div>
  );
}
