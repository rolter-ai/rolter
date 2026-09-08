import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight, Filter, ScrollText, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import {
  FilterCheckList,
  FilterPanel,
  FilterSearchList,
  FilterSection,
} from "@/components/ui/filter-panel";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import {
  AnalyticsUnavailableError,
  fetchBusinessUnits,
  fetchCustomers,
  fetchInvocations,
  fetchLoggingSettings,
  fetchModels,
  type InvocationRow,
} from "@/lib/api";
import { useIsSuperadmin } from "@/lib/can";
import type { CodeLanguage } from "@/lib/code";
import { useCurrencyCode } from "@/lib/currency";
import { useScope } from "@/lib/scope";
import { useFormat } from "@/lib/i18n/format";
import { useModalA11y } from "@/lib/modal-a11y";
import { useDrawerA11y } from "@/lib/use-drawer-a11y";
import { BELOW_LG, BELOW_MD, useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PAGE_SIZE = 50;
type StatusFilter = "all" | "error" | "success";

const num = (v: number | string | undefined): number => {
  const n = Number(v ?? 0);
  return Number.isFinite(n) ? n : 0;
};

function statusTone(status: number): [string, string] {
  if (status === 0 || status >= 500) return ["var(--status-danger-text)", "rgba(229,57,53,.14)"];
  if (status === 429) return ["var(--status-warning-text)", "rgba(245,158,11,.14)"];
  if (status >= 400) return ["var(--status-warning-text)", "rgba(245,158,11,.14)"];
  return ["var(--status-success-text)", "rgba(22,163,74,.14)"];
}

function isUnavailable(error: unknown): boolean {
  return error instanceof AnalyticsUnavailableError;
}

const TH =
  "sticky top-0 z-[1] whitespace-nowrap border-b border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-2.5 text-left text-xs font-medium text-muted-foreground";
const TD =
  "border-b border-[color:var(--border-subtle)] px-3 py-[9px] font-mono text-xs";

// LLM logs from the design prototype: collapsible filter rail, full-height
// streaming request table with sticky headers, and a right detail drawer with
// the raw request/response payloads
export default function Logs() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const [status, setStatus] = React.useState<StatusFilter>("all");
  const [modelSel, setModelSel] = React.useState<string[]>([]);
  const [unitSel, setUnitSel] = React.useState<string[]>([]);
  const [customerSel, setCustomerSel] = React.useState<string[]>([]);
  const [page, setPage] = React.useState(0);
  const [selected, setSelected] = React.useState<InvocationRow | null>(null);
  // below `md` the 248px filter rail would leave the table 127px; below `lg`
  // the 380px detail drawer pushes it off screen entirely (#1203). both become
  // overlays at those widths — the same panels, out of the flow
  const railOverlays = useMediaQuery(BELOW_MD);
  const detailAsSheet = useMediaQuery(BELOW_LG);
  const drawer = useDrawerA11y(selected != null && !detailAsSheet, () =>
    setSelected(null),
  );
  const filterPanel = React.useRef<HTMLDivElement>(null);
  const filterA11y = useModalA11y(filterPanel, {
    open: railOverlays && filtersOpen,
    onEscape: () => setFiltersOpen(false),
  });
  const [streaming, setStreaming] = React.useState(true);

  const window = React.useMemo(
    () => ({ since: new Date(Date.now() - 24 * 3600_000).toISOString() }),
    [],
  );

  const scope = useScope();
  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels });
  // the two governance dimensions a row can be attributed to. named here so
  // the rail and the drawer both show "Platform Engineering" rather than the
  // uuid ClickHouse actually stores
  const units = useQuery({
    queryKey: ["business-units", scope.orgId],
    queryFn: () => fetchBusinessUnits(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const customers = useQuery({
    queryKey: ["customers", scope.orgId],
    queryFn: () => fetchCustomers(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `models` is the query the user is actually waiting on for this screen

  useScreenReady(!models.isLoading);

  useErrorState(!!models.error, "logs");

  React.useEffect(
    () => setPage(0),
    [status, modelSel, unitSel, customerSel],
  );

  const query = useQuery({
    queryKey: [
      "invocations",
      window.since,
      status,
      modelSel[0] ?? "",
      unitSel.join(","),
      customerSel.join(","),
      page,
    ],
    queryFn: () =>
      fetchInvocations({
        since: window.since,
        model: modelSel[0] || undefined,
        // the rail allows several of each, so the whole selection travels
        business_unit: unitSel.length ? unitSel : undefined,
        customer: customerSel.length ? customerSel : undefined,
        status,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      }),
    retry: (n, error) => !isUnavailable(error) && n < 2,
    placeholderData: (prev) => prev,
    refetchInterval: streaming ? 5000 : false,
  });

  // every filter is applied by the server now (#1247). filtering the page
  // here instead made `limit` mean something else: a unit that served 3 of
  // the last 50 requests showed 3 rows under a full-looking pager, and paging
  // was the only way to find the rest
  const rows = query.data ?? [];
  const hasMore = rows.length === PAGE_SIZE;
  const unitName = (id: string) => units.data?.find((u) => u.id === id)?.name;
  const customerName = (id: string) =>
    customers.data?.find((c) => c.id === id)?.name;
  // the gateway decided this per request, against the catalogue that applied
  // when it was served. re-deriving it from today's model prices re-judges an
  // old row against a price added or removed after the fact (#1226)
  const isUnpriced = (row: InvocationRow) => num(row.unpriced) === 1;
  const cost = (row: InvocationRow) =>
    isUnpriced(row) ? null : fmt.currency(num(row.cost_usd), currency);
  const filterCount =
    (status === "all" ? 0 : 1) +
    modelSel.length +
    unitSel.length +
    customerSel.length;
  const clearFilters = () => {
    setStatus("all");
    setModelSel([]);
    setUnitSel([]);
    setCustomerSel([]);
  };

  // a deployment with no analytics store is a load state like any other, not a
  // grey paragraph of its own: LoadError names the cause, says which setting
  // changes it, and withholds the retry that cannot work (#1236)
  if (isUnavailable(query.error)) {
    return (
      <div className="p-[22px]">
        <LoadError error={query.error} resource={t("errors.resources.requestLogs")} />
      </div>
    );
  }

  const statusSelected = status === "all" ? [] : [status];

  // the same panel content in both shapes: an inline drawer beside the
  // table at `lg`, a sheet over it below that
  const detail = selected && (
    <>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <DrawerStat label="Model" value={selected.model} />
        <DrawerStat label="Provider" value={selected.provider || "—"} />
        <DrawerStat
          label="Latency"
          value={t("analytics.ms", {
            value: fmt.number(Math.round(num(selected.latency_ms))),
          })}
        />
        <DrawerStat
          label="Cost"
          value={cost(selected) ?? t("analytics.unpriced")}
          title={isUnpriced(selected) ? t("analytics.unpricedHint") : undefined}
        />
        <DrawerStat
          label="Tokens"
          value={`${num(selected.prompt_tokens)} in · ${num(selected.completion_tokens)} out`}
        />
        <DrawerStat label="Virtual key" value={selected.virtual_key_id || "—"} />
        {/* where this request's spend was charged; a uuid with no row behind
            it still beats hiding the attribution entirely */}
        <DrawerStat
          label={t("pages.logs.businessUnit")}
          value={
            selected.business_unit_id
              ? (unitName(selected.business_unit_id) ?? selected.business_unit_id)
              : "—"
          }
        />
        <DrawerStat
          label={t("pages.logs.customer")}
          value={
            selected.customer_id
              ? (customerName(selected.customer_id) ?? selected.customer_id)
              : "—"
          }
        />
      </div>
      {selected.error && (
        <DrawerBlock label="Error" content={selected.error} language="log" />
      )}
      <PayloadBlock label="Request" raw={selected.request_payload} />
      <PayloadBlock label="Response" raw={selected.response_payload} />
    </>
  );

  return (
    <div className="flex h-full min-h-0">
      {filtersOpen && (
        <div
          ref={railOverlays ? filterPanel : undefined}
          role={railOverlays ? "dialog" : undefined}
          aria-modal={railOverlays ? true : undefined}
          aria-label={railOverlays ? t("common.filters") : undefined}
          className={cn(
            "overflow-y-auto border-r border-[color:var(--border-subtle)]",
            railOverlays
              ? "rl-drawer-in fixed inset-y-0 left-0 z-50 w-[min(300px,85vw)] bg-[color:var(--surface-app)] shadow-[14px_0_44px_rgba(0,0,0,0.42)] focus-visible:outline-none"
              : "w-[248px] flex-none",
          )}
          {...(railOverlays ? filterA11y : {})}
        >
          <FilterPanel title="Filters" onHide={() => setFiltersOpen(false)}>
            <FilterSection title="Status" defaultOpen count={statusSelected.length}>
              <FilterCheckList
                options={[
                  { value: "success", label: "2xx OK" },
                  { value: "error", label: "Errors" },
                ]}
                selected={statusSelected}
                onChange={(sel) =>
                  setStatus(sel.length === 1 ? (sel[0] as StatusFilter) : "all")
                }
              />
            </FilterSection>
            <FilterSection title="Model" defaultOpen count={modelSel.length}>
              <FilterSearchList
                options={(models.data ?? []).map((m) => ({
                  value: m.model,
                  label: m.model,
                }))}
                selected={modelSel}
                onChange={(sel) => setModelSel(sel.slice(-1))}
                placeholder="Filter models"
              />
            </FilterSection>
            {(units.data ?? []).length > 0 && (
              <FilterSection
                title={t("pages.logs.businessUnit")}
                count={unitSel.length}
              >
                <FilterSearchList
                  options={(units.data ?? []).map((u) => ({
                    value: u.id,
                    label: u.name,
                  }))}
                  selected={unitSel}
                  onChange={setUnitSel}
                  placeholder={t("pages.logs.filterBusinessUnits")}
                />
              </FilterSection>
            )}
            {(customers.data ?? []).length > 0 && (
              <FilterSection
                title={t("pages.logs.customer")}
                count={customerSel.length}
              >
                <FilterSearchList
                  options={(customers.data ?? []).map((c) => ({
                    value: c.id,
                    label: c.name,
                  }))}
                  selected={customerSel}
                  onChange={setCustomerSel}
                  placeholder={t("pages.logs.filterCustomers")}
                />
              </FilterSection>
            )}
          </FilterPanel>
        </div>
      )}
      {filtersOpen && railOverlays && (
        <div
          className="rl-fade-in fixed inset-0 z-40 bg-black/50"
          onClick={() => setFiltersOpen(false)}
          aria-hidden
        />
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-none items-center gap-2.5 border-b border-[color:var(--border-subtle)] px-[18px] py-3">
          <button
            type="button"
            onClick={() => setFiltersOpen((v) => !v)}
            className={cn(
              "inline-flex h-8 items-center gap-[7px] rounded-md border border-[color:var(--border-subtle)] px-3 text-sm text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
              filtersOpen && "bg-[color:var(--surface-subtle)]",
            )}
          >
            <Filter className="h-3.5 w-3.5" />
            Filters
            {filterCount > 0 && ` · ${filterCount}`}
          </button>
          <span className="inline-flex items-center gap-[7px] text-xs text-muted-foreground">
            <span
              className={cn(
                "h-[7px] w-[7px] rounded-full",
                streaming ? "rl-pulse bg-[color:var(--status-success)]" : "bg-[color:var(--text-subtle)]",
              )}
            />
            {streaming ? "Streaming" : "Paused"} · {rows.length} requests
          </span>
          <div className="ml-auto flex gap-2">
            <Button size="sm" variant="outline" onClick={() => setStreaming((v) => !v)}>
              {streaming ? "Pause" : "Resume"}
            </Button>
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                title="Previous page"
                aria-label="Previous page"
                disabled={page === 0}
                onClick={() => setPage((p) => Math.max(0, p - 1))}
                className="flex rounded-md border border-[color:var(--border-subtle)] p-[5px] text-[color:var(--text-subtle)] transition-colors enabled:hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <ChevronLeft className="h-3.5 w-3.5" />
              </button>
              <span className="font-mono text-xs text-muted-foreground">p{page + 1}</span>
              <button
                type="button"
                title="Next page"
                aria-label="Next page"
                disabled={!hasMore}
                onClick={() => setPage((p) => p + 1)}
                className="flex rounded-md border border-[color:var(--border-subtle)] p-[5px] text-[color:var(--text-subtle)] transition-colors enabled:hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <ChevronRight className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-auto">
          <table className="w-full min-w-[880px] table-fixed border-collapse text-sm">
            <colgroup>
              <col style={{ width: "16%" }} />
              <col style={{ width: "20%" }} />
              <col style={{ width: "14%" }} />
              <col style={{ width: "10%" }} />
              <col style={{ width: "11%" }} />
              <col style={{ width: "12%" }} />
              <col style={{ width: "11%" }} />
              <col style={{ width: "36px" }} />
            </colgroup>
            <thead>
              <tr>
                <th scope="col" className={TH}>Time</th>
                <th scope="col" className={TH}>Model</th>
                <th scope="col" className={TH}>Provider</th>
                <th scope="col" className={TH}>Status</th>
                <th scope="col" className={cn(TH, "text-right")}>Latency</th>
                <th scope="col" className={cn(TH, "text-right")}>Tokens</th>
                <th scope="col" className={cn(TH, "text-right")}>Cost</th>
                <th scope="col" className={TH}>
                  <span className="sr-only">{t("analytics.details")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const st = num(r.status);
                const tone = statusTone(st);
                return (
                  <tr
                    key={`${r.request_id}-${r.ts}`}
                    onClick={() => setSelected(r)}
                    className="cursor-pointer transition-colors hover:bg-[color:var(--surface-hover)]"
                  >
                    <td className={cn(TD, "truncate whitespace-nowrap")}>
                      {fmt.dateTimeMs(r.ts)}
                    </td>
                    <td className={cn(TD, "[overflow-wrap:anywhere]")}>{r.model}</td>
                    <td className={cn(TD, "truncate whitespace-nowrap text-[color:var(--text-secondary)]")}>
                      {r.provider || "—"}
                    </td>
                    <td className={TD}>
                      <span
                        className="inline-flex items-center rounded-[6px] px-[7px] py-0.5 font-mono text-[11px] font-semibold"
                        style={{ color: tone[0], background: tone[1] }}
                      >
                        {st || "ERR"}
                      </span>
                    </td>
                    <td className={cn(TD, "text-right text-[color:var(--text-secondary)]")}>
                      {t("analytics.ms", { value: fmt.number(Math.round(num(r.latency_ms))) })}
                    </td>
                    <td className={cn(TD, "text-right text-[color:var(--text-secondary)]")}>
                      {fmt.number(num(r.total_tokens))}
                    </td>
                    <td className={cn(TD, "text-right text-[color:var(--text-secondary)]")}>
                      {cost(r) ?? (
                        <span
                          className="text-[color:var(--text-subtle)]"
                          title={t("analytics.unpricedHint")}
                        >
                          {t("analytics.unpriced")}
                        </span>
                      )}
                    </td>
                    <td className={cn(TD, "pr-2.5 text-right")}>
                      {/* the row's click target is a mouse convenience; this
                          button is the keyboard's and the screen reader's way
                          into the same drawer */}
                      <button
                        type="button"
                        aria-label={t("analytics.openDetails", { model: r.model })}
                        onClick={(e) => {
                          e.stopPropagation();
                          setSelected(r);
                        }}
                        className="ml-auto flex rounded-sm text-[color:var(--text-subtle)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                      >
                        <ChevronRight className="h-[15px] w-[15px]" />
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {query.error && (
            <div className="p-4">
              <LoadError
                error={query.error}
                resource={t("errors.resources.requestLogs")}
                onRetry={() => void query.refetch()}
              />
            </div>
          )}
          {/* the screen had no loading indicator at all, so a slow clickhouse
              read looked like a deployment with no traffic (#1180) */}
          {query.isLoading && (
            <div className="p-3">
              <ListSkeleton rows={8} />
            </div>
          )}
          {!query.isLoading && !query.error && rows.length === 0 && (
            <EmptyState
              uxTarget="request-logs"
              icon={<ScrollText />}
              title={filterCount ? t("pages.logs.noMatchTitle") : t("pages.logs.emptyTitle")}
              description={filterCount ? t("pages.logs.noMatchBody") : t("pages.logs.emptyBody")}
              actions={
                filterCount ? (
                  <Button variant="outline" onClick={clearFilters}>
                    {t("common.clearSearch")}
                  </Button>
                ) : undefined
              }
            />
          )}
        </div>
      </div>

      {selected &&
        (detailAsSheet ? (
          <Sheet open onOpenChange={(next) => !next && setSelected(null)}>
            <SheetHeader
              title={t("analytics.details")}
              subtitle={selected.request_id || "request"}
              onClose={() => setSelected(null)}
            />
            <SheetBody>{detail}</SheetBody>
          </Sheet>
        ) : (
          <aside
            {...drawer}
            aria-label={t("analytics.details")}
            className="w-[380px] flex-none overflow-y-auto border-l border-[color:var(--border-subtle)] bg-background focus-visible:outline-none"
          >
            <div className="flex items-center gap-2.5 border-b border-[color:var(--border-subtle)] px-[18px] py-3.5">
              <span className="truncate font-mono text-sm">
                {selected.request_id || "request"}
              </span>
              <button
                type="button"
                aria-label="Close details"
                onClick={() => setSelected(null)}
                className="ml-auto flex text-[color:var(--text-subtle)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="flex flex-col gap-3.5 p-[18px]">{detail}</div>
          </aside>
        ))}
    </div>
  );
}

/**
 * One request or response body in the detail drawer, and — when there is none —
 * why (#954).
 *
 * An empty panel used to say "payload logging is off", which is one of three
 * possible reasons and was often the wrong one. A payload is absent when
 * capture is off, when it is on but this request's model or key falls outside
 * the allow-list, or when the payload's retention window (shorter than the log
 * row's, by design) has already elapsed. The row itself does not record which,
 * so the copy names all three rather than asserting one.
 *
 * A superadmin can be told which it is, because they can read the setting; for
 * everyone else the answer would be a 403, so the screen does not ask. Either
 * way the text says where the setting lives instead of leaving the reader to
 * hunt for it.
 */
function PayloadBlock({ label, raw }: { label: string; raw: string | undefined }) {
  const { t } = useTranslation();
  const isSuperadmin = useIsSuperadmin();
  const body = pretty(raw);
  const settings = useQuery({
    queryKey: ["logging-settings", "payload-hint"],
    queryFn: fetchLoggingSettings,
    // only asked when the answer is readable, and a failure is not worth
    // surfacing: the generic explanation below is still true
    enabled: isSuperadmin === true && body === null,
    retry: false,
    staleTime: 60_000,
  });

  if (body !== null) {
    return <DrawerBlock label={label} content={body} language={payloadLanguage(raw)} />;
  }

  const captureOff = settings.data ? !settings.data.payload_capture_enabled : undefined;
  const reason =
    captureOff === true
      ? t("pages.logs.payloadCaptureOff")
      : captureOff === false
        ? t("pages.logs.payloadCaptureOnButAbsent", {
            hours: settings.data?.payload_retention_hours ?? 0,
          })
        : t("pages.logs.payloadAbsent");

  return (
    <div className="mt-4">
      <div className="mb-1.5 text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="rounded-[8px] border border-dashed border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3">
        <p className="text-xs leading-relaxed text-muted-foreground">{reason}</p>
        <a
          href="/logs-settings"
          className="mt-2 inline-block text-xs font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          {t("pages.logs.payloadSettingsLink")}
        </a>
      </div>
    </div>
  );
}

function pretty(raw: string | undefined): string | null {
  if (!raw) return null;
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

// a logged payload is JSON when it parses as JSON, and opaque text when it does
// not — a multipart upload, a truncated body, or the "logging is off" notice.
// highlighting the second as JSON would invent structure that is not there
function payloadLanguage(raw: string | undefined): CodeLanguage {
  if (!raw) return "text";
  try {
    JSON.parse(raw);
    return "json";
  } catch {
    return "text";
  }
}

function DrawerStat({
  label,
  value,
  title,
}: {
  label: string;
  value: string;
  title?: string;
}) {
  return (
    <div>
      <div className="mb-[3px] text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="truncate font-mono text-sm" title={title}>
        {value}
      </div>
    </div>
  );
}

function DrawerBlock({
  label,
  content,
  language,
}: {
  label: string;
  content: string;
  language: CodeLanguage;
}) {
  return (
    <div>
      <div className="mb-1.5 text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      {/* the payload is the reason the drawer was opened: it reads through the
          shared code block, so a malformed field is visible rather than hidden
          in a wall of monospace (#949). soft-wrapped, because the drawer is
          narrow and a body line is long */}
      <CodeBlock value={content} language={language} label={label} wrap />
    </div>
  );
}
