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

// the sampled, assigned and first-attempt shares each add a column when the
// read carries them
const grid = (sampled: boolean, assigned: boolean, first: boolean) =>
  [
    primaryColumn(1.4),
    "84px",
    ...(sampled ? ["84px"] : []),
    ...(assigned ? ["84px"] : []),
    "84px",
    ...(first ? ["84px"] : []),
    "84px 70px 112px",
  ].join(" ");

// the width the grid above needs before it squeezes a column: the primary
// column's floor (11rem), the fixed columns, a 12px gap between columns and
// the row's 16px side padding. the list table's default floor (760px) is
// narrower than the six-column grid, so every case passes its own
const minWidth = (sampled: boolean, assigned: boolean, first: boolean) => {
  const shares = 2 + Number(sampled) + Number(assigned) + Number(first);
  const columns = 1 + shares + 3;
  return 176 + shares * 84 + 84 + 70 + 112 + (columns - 1) * 12 + 32;
};

/** one row of the table: a configured variant, or traffic no configured variant explains */
interface TrafficLine {
  name: string;
  /** the share the saved weights give it; null for a variant no longer configured */
  configured: number | null;
  row: AnalyticsByVariantRow | undefined;
}

const num = (v: number | string | null | undefined) => Number(v ?? 0);

/** a by-variant row's key: two organizations may each have a variant of one name (#3548) */
const rowKey = (orgId: string, variant: string) => `${orgId}\u0000${variant}`;

/** whether some line carries a figure for `field`: a null or absent one is unknown */
const carries = (
  lines: TrafficLine[],
  field: "first_attempt_requests" | "assigned_requests" | "sampled_requests",
) => lines.some((line) => line.row?.[field] !== undefined && line.row[field] !== null);

/**
 * What each saved variant of a route actually served (#3342), beside the split
 * its weights configure.
 *
 * The observed share is of the requests a variant served, so traffic from
 * before the route had variants (the empty variant) is said apart instead of
 * diluting every share. It counts first attempts and failovers alike, because
 * the log records the variant that answered, so a variant that is failed over
 * to reads higher than its weight; the note under the table says so. Beside
 * it, the first-attempt share counts only the requests a variant answered
 * with no failover (#3409). The assigned share (#3505) counts the requests
 * each variant was dialled to first, failed over or not, so a variant that
 * often fails over keeps its assignments there, and a variant that answered
 * nothing still has a line. It is not the sampled split: a variant the
 * gateway skips while its breaker is open or its targets are parked is
 * dialled first by none of the requests sampled for it. The sampled share
 * (#3547) is the split itself, the requests the weights picked each variant
 * for before any skipping, so it sits next to the configured share, and a
 * variant the gateway routes around reads above its assigned share there. A
 * ClickHouse that cannot tell a first attempt from a
 * failover, or that does not record the first variant, sends no count for
 * that column, and the column is left out rather than drawn as a guess.
 *
 * The configured split is the saved one. A draft the operator has not saved is
 * not compared with traffic it never served.
 */
export function RouteVariantTraffic({
  model,
  projectId,
  saved,
}: {
  /** the route's public name, which the log records as `model` */
  model: string;
  /** the route's project: a name is unique per project only, so the read is narrowed to it */
  projectId: string;
  saved: RouteVariantRow[];
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const [timeWindow, setTimeWindow] = React.useState<TimeWindow>("7d");
  const windowOptions = useTimeWindowOptions();
  const fid = React.useId();

  const read = useQuery({
    queryKey: ["analytics-by-variant", projectId, model, timeWindow],
    // the bounds are worked out as the read leaves, so a sheet left open reads the window as it is now
    queryFn: () =>
      fetchAnalyticsByVariant(model, { ...windowBounds(timeWindow), project: projectId }),
    enabled: model !== "" && saved.length > 0,
    retry: false,
  });
  if (saved.length === 0) return null;

  const all = read.data ?? [];
  // rows are keyed on the organization too (#3548). taking the first row's
  // org is right only because the read is narrowed by `project: projectId`
  // and a project belongs to exactly one org, so every row names the route's
  // own; a control plane older than the field sends none, which keys every
  // row on the empty string
  const orgId = all[0]?.org_id ?? "";
  const rows = all.filter((row) => (row.org_id ?? "") === orgId);
  const byKey = new Map(rows.map((row) => [rowKey(orgId, row.variant), row]));
  const split = trafficSplit(saved.map((v) => v.weight));
  const lines: TrafficLine[] = [
    ...saved.map((v, i) => ({
      name: v.name,
      configured: split[i] ?? null,
      row: byKey.get(rowKey(orgId, v.name)),
    })),
    // a variant that served traffic and is no longer configured is still a share of what happened
    ...rows
      .filter((row) => row.variant !== "" && !saved.some((v) => v.name === row.variant))
      .map((row) => ({ name: row.variant, configured: null, row })),
  ];
  const served = lines.reduce((sum, line) => sum + num(line.row?.requests), 0);
  // known only when the read carries a count for some variant
  const splitKnown = carries(lines, "first_attempt_requests");
  const assignedKnown = carries(lines, "assigned_requests");
  const assignedTotal = lines.reduce((sum, line) => sum + num(line.row?.assigned_requests), 0);
  const sampledKnown = carries(lines, "sampled_requests");
  const sampledTotal = lines.reduce((sum, line) => sum + num(line.row?.sampled_requests), 0);
  const gridColumns = grid(sampledKnown, assignedKnown, splitKnown);
  const tableWidth = minWidth(sampledKnown, assignedKnown, splitKnown);
  const firstServed = lines.reduce((sum, line) => sum + num(line.row?.first_attempt_requests), 0);
  const unattributed = num(byKey.get(rowKey(orgId, ""))?.requests);
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
      ) : served === 0 && assignedTotal === 0 && sampledTotal === 0 ? (
        <EmptyState
          inline
          uxTarget="route-variant-traffic"
          description={t("routeSheet.variants.observed.empty", {
            window: windowOptions.find((o) => o.value === timeWindow)?.label ?? timeWindow,
          })}
        />
      ) : (
        <>
          <ListTable label={t("routeSheet.variants.observed.title")} minWidth={tableWidth}>
            <ListHeader grid={gridColumns}>
              <ListHeaderCell>{t("routeSheet.variants.observed.variant")}</ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.configured")}
              </ListHeaderCell>
              {sampledKnown && (
                <ListHeaderCell className="text-right">
                  {t("routeSheet.variants.observed.sampled")}
                </ListHeaderCell>
              )}
              {assignedKnown && (
                <ListHeaderCell className="text-right">
                  {t("routeSheet.variants.observed.assigned")}
                </ListHeaderCell>
              )}
              <ListHeaderCell className="text-right">
                {t("routeSheet.variants.observed.share")}
              </ListHeaderCell>
              {splitKnown && (
                <ListHeaderCell className="text-right">
                  {t("routeSheet.variants.observed.firstAttempt")}
                </ListHeaderCell>
              )}
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
                <ListRow key={rowKey(orgId, line.name)} grid={gridColumns}>
                  <ListCell className="truncate font-mono text-xs">{line.name}</ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {line.configured === null ? "—" : fmt.percent(line.configured, 0)}
                  </ListCell>
                  {sampledKnown && (
                    <ListCell className="text-right font-mono text-xs">
                      {sampledTotal > 0
                        ? fmt.percent(num(line.row?.sampled_requests) / sampledTotal, 0)
                        : "—"}
                    </ListCell>
                  )}
                  {assignedKnown && (
                    <ListCell className="text-right font-mono text-xs">
                      {assignedTotal > 0
                        ? fmt.percent(num(line.row?.assigned_requests) / assignedTotal, 0)
                        : "—"}
                    </ListCell>
                  )}
                  <ListCell className="text-right font-mono text-xs">
                    {served > 0 ? fmt.percent(requests / served, 0) : "—"}
                  </ListCell>
                  {splitKnown && (
                    <ListCell className="text-right font-mono text-xs">
                      {firstServed > 0
                        ? fmt.percent(num(line.row?.first_attempt_requests) / firstServed, 0)
                        : "—"}
                    </ListCell>
                  )}
                  <ListCell className="text-right font-mono text-xs">
                    {fmt.number(requests)}
                  </ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {requests > 0 ? fmt.percent(num(line.row?.errors) / requests, 1) : "—"}
                  </ListCell>
                  <ListCell className="text-right font-mono text-xs">
                    {/* a variant that answered nothing has no latency, only its assignments */}
                    {line.row && requests > 0
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
            {t(
              assignedKnown
                ? "routeSheet.variants.observed.noteWithAssigned"
                : splitKnown
                  ? "routeSheet.variants.observed.noteWithFirstAttempt"
                  : "routeSheet.variants.observed.note",
            )}
            {sampledKnown && <> {t("routeSheet.variants.observed.noteSampled")}</>}
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
