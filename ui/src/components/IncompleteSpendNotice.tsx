import { CircleDollarSign } from "lucide-react";
import { useTranslation } from "react-i18next";

import { SpendNotice } from "@/components/SpendNotice";

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
    <SpendNotice
      icon={CircleDollarSign}
      title={t("pages.dashboard.unpriced.title", { count: requests })}
      body={t("pages.dashboard.unpriced.body", { count: models })}
      to="/pricing-overrides"
      link={t("pages.dashboard.unpriced.link")}
    />
  );
}
