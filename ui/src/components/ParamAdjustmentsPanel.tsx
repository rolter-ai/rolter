import { useQuery } from "@tanstack/react-query";
import { SlidersHorizontal } from "lucide-react";
import { Trans, useTranslation } from "react-i18next";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import {
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListTable,
  primaryColumn,
} from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { AnalyticsUnavailableError, fetchParamAdjustments } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { dispositionLabel } from "@/lib/param-adjustments";

// route, provider, parameter, what happened to it, share of the route's traffic
const GRID = `${primaryColumn(1.4)} minmax(8rem, 1fr) minmax(8rem, 1fr) minmax(7rem, 0.8fr) minmax(9rem, 0.9fr)`;

/**
 * Which request parameters each route's translation did not forward as sent,
 * and on how much of its traffic (#3323). The gateway records each one per
 * request (#1586); this is the rollup an operator reads to see "this route
 * drops `top_k` on 40% of its traffic" instead of each caller finding out
 * alone. Read-only, over the control plane's default window (the last 7 days).
 */
export function ParamAdjustmentsPanel() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const read = useQuery({
    queryKey: ["analytics", "param-adjustments"],
    queryFn: () => fetchParamAdjustments(),
    retry: (n, error) => !(error instanceof AnalyticsUnavailableError) && n < 2,
  });
  const rows = read.data?.data ?? [];
  const title = t("pages.compatibility.adjustments.title");

  const body = () => {
    if (read.error instanceof AnalyticsUnavailableError) {
      return (
        <AnalyticsUnavailable
          error={read.error}
          i18nKey="pages.compatibility.adjustments.noAnalytics"
        />
      );
    }
    if (read.isError) {
      return (
        <LoadError
          error={read.error}
          resource={t("errors.resources.paramAdjustments")}
          onRetry={() => void read.refetch()}
          target="param-adjustments"
        />
      );
    }
    return (
      <ListTable label={title}>
        <ListHeader grid={GRID}>
          <ListHeaderCell>{t("pages.compatibility.adjustments.colRoute")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.compatibility.adjustments.colProvider")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.compatibility.adjustments.colParam")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.compatibility.adjustments.colDisposition")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.compatibility.adjustments.colShare")}</ListHeaderCell>
        </ListHeader>
        <ListLoadingRow read={read}>
          <ListSkeleton rows={3} className="p-3" />
        </ListLoadingRow>
        {rows.map((row) => (
          <ListRow key={`${row.route}|${row.provider}|${row.disposition}|${row.param}`} grid={GRID}>
            <ListCell className="truncate font-mono text-xs">{row.route}</ListCell>
            <ListCell className="truncate font-mono text-xs">{row.provider}</ListCell>
            <ListCell className="truncate font-mono text-xs">{row.param}</ListCell>
            <ListCell className="text-xs">{dispositionLabel(t, row.disposition)}</ListCell>
            <ListCell className="text-xs tabular-nums">
              {t("pages.compatibility.adjustments.share", {
                share: fmt.percent(Number(row.share)),
                requests: fmt.number(Number(row.requests)),
                total: fmt.number(Number(row.route_requests)),
              })}
            </ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={read} rows={rows.length}>
          {read.data?.recorded === false ? (
            <EmptyState
              uxTarget="param-adjustments-unrecorded"
              icon={<SlidersHorizontal />}
              title={t("pages.compatibility.adjustments.notRecordedTitle")}
              description={
                <Trans
                  i18nKey="pages.compatibility.adjustments.notRecordedBody"
                  components={[<code key="file" className="font-mono text-xs" />]}
                />
              }
            />
          ) : (
            <EmptyState
              uxTarget="param-adjustments"
              icon={<SlidersHorizontal />}
              title={t("pages.compatibility.adjustments.emptyTitle")}
              description={t("pages.compatibility.adjustments.emptyBody")}
            />
          )}
        </ListEmptyRow>
      </ListTable>
    );
  };

  return (
    <SettingsPanel title={title} description={t("pages.compatibility.adjustments.desc")}>
      {body()}
    </SettingsPanel>
  );
}
