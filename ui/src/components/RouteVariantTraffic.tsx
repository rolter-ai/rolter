import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import {
  ListCell,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListTable,
  primaryColumn,
} from "@/components/screen";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { FieldLabel } from "@/components/ui/field-label";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsByVariant,
  type AnalyticsByVariantRow,
  type RouteVariantRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { trafficSplit } from "@/lib/route-variants";
import { useTimeWindowOptions, windowBounds, type TimeWindow } from "@/lib/time-window";

const GRID = `${primaryColumn(1.4)} 84px 84px 84px 70px 112px`;

/** one row of the table: a configured variant, or traffic no configured variant explains */
interface TrafficLine {
  name: string;
  /** the share the saved weights give it; null for a variant no longer configured */
  configured: number | null;
  row: AnalyticsByVariantRow | undefined;
}

const num = (v: number | string | undefined) => Number(v ?? 0);

/**
 * What each saved variant of a route actually served (#3342), beside the split
 * its weights configure.
 *
 * The observed share is of the requests a variant served, so traffic from
 * before the route had variants (the empty variant) is said apart instead of
 * diluting every share. It counts first attempts and failovers alike, because
 * the log records the variant that answered, so a variant that is failed over
 * to reads higher than its weight; the note under the table says so.
 *
 * The configured split is the saved one. A draft the operator has not saved is
 * not compared with traffic it never served.
 */
export function RouteVariantTraffic({
  model,
  saved,
}: {
  /** the route's public name, which the log records as `model` */
  model: string;
  saved: RouteVariantRow[];
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const [timeWindow, setTimeWindow] = React.useState<TimeWindow>("7d");
  const windowOptions = useTimeWindowOptions();
  const fid = React.useId();

  const read = useQuery({
    queryKey: ["analytics-by-variant", model, timeWindow],
    // the bounds are worked out as the read leaves, so a sheet left open reads the window as it is now
    queryFn: () => fetchAnalyticsByVariant(model, windowBounds(timeWindow)),
    enabled: model !== "" && saved.length > 0,
    retry: false,
  });
  if (saved.length === 0) return null;

  const rows = read.data ?? [];
  const byName = new Map(rows.map((row) => [row.variant, row]));
  const split = trafficSplit(saved.map((v) => v.weight));
  const lines: TrafficLine[] = [
    ...saved.map((v, i) => ({
      name: v.name,
      configured: split[i] ?? null,
      row: byName.get(v.name),
    })),
    // a variant that served traffic and is no longer configured is still a share of what happened
    ...rows
      .filter((row) => row.variant !== "" && !saved.some((v) => v.name === row.variant))
      .map((row) => ({ name: row.variant, configured: null, row })),
  ];
  const served = lines.reduce((sum, line) => sum + num(line.row?.requests), 0);
  const unattributed = num(byName.get("")?.requests);
  const unavailable = read.error instanceof AnalyticsUnavailableError ? read.error : null;

  return (
    <div className="space-y-2" data-testid="variant-traffic">
      <div className="flex items-end justify-between gap-2">
        <FieldLabel label={t("routeSheet.variants.observed.title")} id={`${fid}-label`} />
        <Combobox
          className="w-44"
          aria-label={t("common.timeWindow.label")}
          value={timeWindow}
          onChange={(next) => setTimeWindow(next as TimeWindow)}
          options={windowOptions}
        />
      </div>
      {unavailable ? (
        <AnalyticsUnavailable
          error={unavailable}
          i18nKey="routeSheet.variants.observed.noAnalytics"
        />
      ) : read.isLoading ? (
        <TableSkeleton rows={Math.min(saved.length, 4)} />
      ) : read.isError ? (
        <LoadError
          error={read.error}
          resource={t("errors.resources.variantTraffic")}
          onRetry={() => void read.refetch()}
          target="route-variant-traffic"
        />
      ) : served === 0 ? (
        <EmptyState
          inline
          uxTarget="route-variant-traffic"
          description={t("routeSheet.variants.observed.empty", {
            window: windowOptions.find((o) => o.value === timeWindow)?.label ?? timeWindow,
          })}
        />
      ) : (
        <>
          <ListTable label={t("routeSheet.variants.observed.title")}>
            <ListHeader grid={GRID}>
              <ListHeaderCell>{t("routeSheet.variants.observed.variant")}</ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.configured")}
              </ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.share")}
              </ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.requests")}
              </ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.errors")}
              </ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.latency")}
              </ListHeaderCell>
            </ListHeader>
            {lines.map((line) => {
              const requests = num(line.row?.requests);
              return (
                <ListRow key={line.name} grid={GRID}>
                  <ListCell className="truncate font-mono text-xs">{line.name}</ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {line.configured === null ? "—" : fmt.percent(line.configured, 0)}
                  </ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {fmt.percent(requests / served, 0)}
                  </ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {fmt.number(requests)}
                  </ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {requests > 0 ? fmt.percent(num(line.row?.errors) / requests, 1) : "—"}
                  </ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {line.row
                      ? t("routeSheet.variants.observed.latencyValue", {
                          p50: fmt.number(Math.round(num(line.row.p50_latency_ms))),
                          p95: fmt.number(Math.round(num(line.row.p95_latency_ms))),
                        })
                      : "—"}
                  </ListCell>
                </ListRow>
              );
            })}
          </ListTable>
          <p className="text-xs leading-snug text-muted-foreground">
            {t("routeSheet.variants.observed.note")}
            {unattributed > 0 && (
              <>
                {" "}
                {t("routeSheet.variants.observed.unattributed", {
                  count: unattributed,
                  requests: fmt.number(unattributed),
                })}
              </>
            )}
          </p>
        </>
      )}
    </div>
  );
}
