import { CircleHelp, ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";

import { SpendNotice } from "@/components/SpendNotice";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";

/** LLM Logs over the figures' own window and filters, narrowed by one more flag. */
function logsFor(search: string, flag: "withheld" | "usage_unknown") {
  const params = new URLSearchParams(search);
  params.set(flag, "true");
  return { pathname: "/logs", search: `?${params.toString()}` };
}

/**
 * Says how much of the window's spend bought a response nobody received, and
 * how many successful requests have a cost that is unknown rather than zero
 * (#3302). Both are in the figures above it, which would otherwise read as
 * clean: a withheld response is billed, and a request with no reported usage
 * counts as free. Each notice shows only while its count is non-zero, and links
 * to LLM Logs over the same window and filters, narrowed to those rows.
 */
export function WithheldSpendNotice({
  withheld,
  withheldCost,
  unknownUsage,
  search,
}: {
  /** requests in the window whose response was withheld */
  withheld: number;
  /** what the provider billed for them, in the deployment's currency; part of the spend */
  withheldCost: number;
  /** successful requests whose upstream reported no usage */
  unknownUsage: number;
  /** the dashboard's address query: the window and the filters the figures were read with */
  search: string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  return (
    <>
      {withheld > 0 && (
        <SpendNotice
          icon={ShieldAlert}
          title={t("pages.dashboard.withheld.title", { count: withheld })}
          body={t("pages.dashboard.withheld.body", { cost: fmt.currency(withheldCost, currency) })}
          to={logsFor(search, "withheld")}
          link={t("pages.dashboard.withheld.link")}
        />
      )}
      {unknownUsage > 0 && (
        <SpendNotice
          icon={CircleHelp}
          title={t("pages.dashboard.usageUnknown.title", { count: unknownUsage })}
          body={t("pages.dashboard.usageUnknown.body")}
          to={logsFor(search, "usage_unknown")}
          link={t("pages.dashboard.usageUnknown.link")}
        />
      )}
    </>
  );
}
