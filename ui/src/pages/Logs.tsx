import { useQuery } from "@tanstack/react-query";
import {
  ChartNoAxesColumn,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Filter,
  ScrollText,
  X,
} from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

import { CopyButton } from "@/components/CopyButton";
import {
  FilterCheckList,
  FilterPanel,
  FilterSearchList,
  FilterSection,
} from "@/components/ui/filter-panel";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import {
  AnalyticsUnavailableError,
  fetchBusinessUnits,
  fetchCustomers,
  fetchInvocationsPage,
  fetchLoggingSettings,
  fetchModels,
  fetchVirtualKeys,
  type InvocationRow,
} from "@/lib/api";
import { useCan } from "@/lib/can";
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
// how often the live feed asks for the newest page
const POLL_MS = 5000;
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

// the same verdict as `statusTone`, in the badge's own tones for the drawer
function verdictTone(status: number): "success" | "warning" | "danger" {
  if (status === 0 || status >= 500) return "danger";
  if (status >= 400) return "warning";
  return "success";
}

// a row is one request at one instant; polling hands back fresh objects for
// the same rows, so the open row is matched on this rather than on identity
const rowKey = (row: InvocationRow) => `${row.request_id}-${row.ts}`;

function isUnavailable(error: unknown): boolean {
  return error instanceof AnalyticsUnavailableError;
}

const TH =
  "sticky top-0 z-[1] whitespace-nowrap border-b border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-2.5 text-left text-xs font-medium text-muted-foreground";
const TD = "border-b border-[color:var(--border-subtle)] px-3 py-[9px] font-mono text-xs";

// LLM logs from the design prototype: collapsible filter rail, full-height
// streaming request table with sticky headers, and a right detail drawer with
// the raw request/response payloads. `pollMs` is only ever set by a story, so
// a play can watch several polling intervals pass inside the test-runner's
// per-story budget
export default function Logs({ pollMs = POLL_MS }: { pollMs?: number }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const [status, setStatus] = React.useState<StatusFilter>("all");
  const [modelSel, setModelSel] = React.useState<string[]>([]);
  const [unitSel, setUnitSel] = React.useState<string[]>([]);
  const [customerSel, setCustomerSel] = React.useState<string[]>([]);
  // the cursor each page after the first was opened with, oldest first. a
  // stack rather than a page index: the control plane pages on a keyset, so
  // "previous" has to return to a cursor it was handed rather than compute a
  // row offset, which the gateway's writes would shift (#1410, #1411)
  const [cursors, setCursors] = React.useState<string[]>([]);
  const page = cursors.length;
  const [selected, setSelected] = React.useState<InvocationRow | null>(null);
  // below `md` the 248px filter rail would leave the table 127px; below `lg`
  // the 380px detail drawer pushes it off screen entirely (#1203). both become
  // overlays at those widths — the same panels, out of the flow
  const railOverlays = useMediaQuery(BELOW_MD);
  const detailAsSheet = useMediaQuery(BELOW_LG);
  const drawer = useDrawerA11y(selected != null && !detailAsSheet, () => setSelected(null));
  const filterPanel = React.useRef<HTMLDivElement>(null);
  const filterA11y = useModalA11y(filterPanel, {
    open: railOverlays && filtersOpen,
    onEscape: () => setFiltersOpen(false),
  });
  const [streaming, setStreaming] = React.useState(true);
  const errorHeading = React.useId();

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

  React.useEffect(() => setCursors([]), [status, modelSel, unitSel, customerSel]);

  const query = useQuery({
    queryKey: [
      "invocations",
      window.since,
      status,
      modelSel[0] ?? "",
      unitSel.join(","),
      customerSel.join(","),
      cursors[page - 1] ?? "",
    ],
    queryFn: () =>
      fetchInvocationsPage({
        since: window.since,
        model: modelSel[0] || undefined,
        // the rail allows several of each, so the whole selection travels
        business_unit: unitSel.length ? unitSel : undefined,
        customer: customerSel.length ? customerSel : undefined,
        status,
        limit: PAGE_SIZE,
        cursor: cursors[page - 1],
      }),
    retry: (n, error) => !isUnavailable(error) && n < 2,
    placeholderData: (prev) => prev,
    // a query that has never held data goes back to pending on every refetch,
    // which unmounts its error: polling one that failed swapped the alert for
    // a skeleton and back every cycle, and a screen reader heard the alert
    // again each time (#1984). so the feed stops there and waits for the
    // retry button. a failure with rows already on screen keeps its error
    // through a refetch, so that one goes on polling and says it is retrying
    refetchInterval: (q) =>
      streaming && !(q.state.status === "error" && q.state.data === undefined) ? pollMs : false,
  });

  // every filter is applied by the server now (#1247). filtering the page
  // here instead made `limit` mean something else: a unit that served 3 of
  // the last 50 requests showed 3 rows under a full-looking pager, and paging
  // was the only way to find the rest
  const rows = query.data?.data ?? [];
  const nextCursor = query.data?.next_cursor ?? null;
  // a short page is the last one even though it still carries a cursor. while
  // a page is loading the previous one stays on screen as a placeholder, and
  // its cursor is the one just followed: a second click would push it again
  const hasMore = nextCursor != null && rows.length === PAGE_SIZE && !query.isPlaceholderData;
  const toNextPage = () => {
    if (hasMore) setCursors((stack) => [...stack, nextCursor]);
  };
  const unitName = (id: string) => units.data?.find((u) => u.id === id)?.name;
  const customerName = (id: string) => customers.data?.find((c) => c.id === id)?.name;
  // the gateway decided this per request, against the catalogue that applied
  // when it was served. re-deriving it from today's model prices re-judges an
  // old row against a price added or removed after the fact (#1226)
  const isUnpriced = (row: InvocationRow) => num(row.unpriced) === 1;
  const cost = (row: InvocationRow) =>
    isUnpriced(row) ? null : fmt.currency(num(row.cost_usd), currency);
  const filterCount =
    (status === "all" ? 0 : 1) + modelSel.length + unitSel.length + customerSel.length;
  const clearFilters = () => {
    setStatus("all");
    setModelSel([]);
    setUnitSel([]);
    setCustomerSel([]);
  };

  // a deployment with no analytics store is a shape rolter supports, not a
  // failure, so it gets a calm panel naming the setting rather than the red
  // alert a 500 gets (#1236, #1984). no retry, since none can help
  if (isUnavailable(query.error)) {
    return (
      <div className="p-[22px]">
        <AnalyticsUnavailable error={query.error} />
      </div>
    );
  }

  // what the toolbar says the feed is doing, read off the fetch rather than
  // off the pause toggle. it used to pulse green and say "Streaming" through
  // the first load and through every failed refresh, which is exactly when
  // the rows on screen are not live (#1984)
  const failedAt = fmt.time(query.errorUpdatedAt);
  const feed: { dot: string; label: string } = query.isError
    ? {
        dot: "bg-[color:var(--status-danger)]",
        // no rows means the feed stopped polling (see `refetchInterval`), so
        // only a failure with rows on screen and the stream on is retrying
        label:
          query.data === undefined
            ? t("pages.logs.feed.loadFailed", { time: failedAt })
            : streaming
              ? t("pages.logs.feed.refreshFailedRetrying", { time: failedAt })
              : t("pages.logs.feed.refreshFailed", { time: failedAt }),
      }
    : query.isPending
      ? { dot: "bg-[color:var(--text-subtle)]", label: t("pages.logs.feed.loading") }
      : {
          dot: streaming
            ? "rl-pulse bg-[color:var(--status-success)]"
            : "bg-[color:var(--text-subtle)]",
          label: `${streaming ? t("pages.logs.streaming") : t("pages.logs.paused")} · ${t(
            "pages.logs.requests",
            { count: rows.length },
          )}`,
        };

  const statusSelected = status === "all" ? [] : [status];

  const ms = (value: number | string) =>
    t("analytics.ms", { value: fmt.number(Math.round(num(value))) });

  // the same panel content in both shapes: an inline drawer beside the
  // table at `lg`, a sheet over it below that. it reads in the order a
  // failed row is investigated (#1983): the verdict and the ids to quote,
  // then why it failed, then how it was routed, what it cost and who pays
  const detail = selected && (
    <div className="flex flex-col gap-5">
      <Verdict row={selected} />
      {selected.error && (
        <section aria-labelledby={errorHeading} className="flex flex-col gap-2">
          <h3
            id={errorHeading}
            className="flex items-center gap-1.5 text-xs font-medium text-[color:var(--status-danger-text)]"
          >
            <CircleAlert aria-hidden className="h-3.5 w-3.5 flex-none" />
            {t("pages.logs.error")}
          </h3>
          <CodeBlock value={selected.error} language="log" label={t("pages.logs.error")} wrap />
        </section>
      )}
      <DetailSection title={t("pages.logs.detail.routing")}>
        <DetailRow label={t("pages.logs.detail.providerTarget")} mono>
          {selected.provider || selected.target ? (
            t("pages.logs.detail.providerToTarget", {
              provider: selected.provider || "—",
              target: selected.target || "—",
            })
          ) : (
            <Absent />
          )}
        </DetailRow>
        <DetailRow label={t("pages.logs.detail.variant")} mono>
          {selected.variant || <Absent />}
        </DetailRow>
        {/* rolter's own response cache: a hit never reached the upstream. the
            provider's prompt cache is a token count, under usage */}
        <DetailRow label={t("pages.logs.detail.responseCache")}>
          {num(selected.cache_hit) === 1
            ? t("pages.logs.detail.cacheHit")
            : t("pages.logs.detail.cacheMiss")}
        </DetailRow>
        <DetailRow label={t("pages.logs.detail.stream")}>
          {num(selected.stream) === 1
            ? t("pages.logs.detail.streamed")
            : t("pages.logs.detail.notStreamed")}
        </DetailRow>
        <DetailRow label={t("pages.logs.detail.ttft")} mono>
          {num(selected.ttft_ms) > 0 ? ms(selected.ttft_ms) : <Absent />}
        </DetailRow>
        <DetailRow label={t("pages.logs.latency")} mono>
          {ms(selected.latency_ms)}
        </DetailRow>
      </DetailSection>
      <DetailSection title={t("pages.logs.detail.usage")}>
        <DetailRow label={t("pages.logs.tokens")} mono>
          {t("pages.logs.tokensInOut", {
            in: fmt.number(num(selected.prompt_tokens)),
            out: fmt.number(num(selected.completion_tokens)),
          })}
        </DetailRow>
        <DetailRow label={t("pages.logs.detail.promptCache")} mono>
          {t("pages.logs.detail.promptCacheTokens", {
            read: fmt.number(num(selected.cache_read_tokens)),
            write: fmt.number(num(selected.cache_write_tokens)),
          })}
        </DetailRow>
        <DetailRow label={t("pages.logs.cost")} mono>
          <span title={isUnpriced(selected) ? t("analytics.unpricedHint") : undefined}>
            {cost(selected) ?? t("analytics.unpriced")}
          </span>
        </DetailRow>
      </DetailSection>
      <DetailSection title={t("pages.logs.detail.attribution")}>
        <DetailRow label={t("pages.logs.virtualKey")}>
          <KeyIdentity row={selected} />
        </DetailRow>
        {/* where this request's spend was charged; a uuid with no row behind
            it still beats hiding the attribution entirely */}
        <DetailRow label={t("pages.logs.businessUnit")}>
          <NamedId id={selected.business_unit_id} name={unitName(selected.business_unit_id)} />
        </DetailRow>
        <DetailRow label={t("pages.logs.customer")}>
          <NamedId id={selected.customer_id} name={customerName(selected.customer_id)} />
        </DetailRow>
      </DetailSection>
      <PayloadBlock
        label={t("pages.logs.request")}
        raw={selected.request_payload}
        withheld={Number(selected.payload_withheld ?? 0) === 1}
      />
      <PayloadBlock
        label={t("pages.logs.response")}
        raw={selected.response_payload}
        withheld={Number(selected.payload_withheld ?? 0) === 1}
      />
    </div>
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
          <FilterPanel title={t("common.filters")} onHide={() => setFiltersOpen(false)}>
            <FilterSection title={t("pages.logs.status")} defaultOpen count={statusSelected.length}>
              <FilterCheckList
                options={[
                  { value: "success", label: t("pages.logs.statusOk") },
                  { value: "error", label: t("pages.logs.statusErrors") },
                ]}
                selected={statusSelected}
                onChange={(sel) => setStatus(sel.length === 1 ? (sel[0] as StatusFilter) : "all")}
              />
            </FilterSection>
            <FilterSection title={t("pages.logs.model")} defaultOpen count={modelSel.length}>
              <FilterSearchList
                options={(models.data ?? []).map((m) => ({
                  value: m.model,
                  label: m.model,
                }))}
                selected={modelSel}
                onChange={(sel) => setModelSel(sel.slice(-1))}
                placeholder={t("pages.logs.filterModels")}
              />
            </FilterSection>
            {(units.data ?? []).length > 0 && (
              <FilterSection title={t("pages.logs.businessUnit")} count={unitSel.length}>
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
              <FilterSection title={t("pages.logs.customer")} count={customerSel.length}>
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
            {t("common.filters")}
            {filterCount > 0 && ` · ${filterCount}`}
          </button>
          <span className="inline-flex min-w-0 items-center gap-[7px] text-xs text-muted-foreground">
            <span className={cn("h-[7px] w-[7px] flex-none rounded-full", feed.dot)} />
            {feed.label}
          </span>
          <div className="ml-auto flex gap-2">
            <Button size="sm" variant="outline" onClick={() => setStreaming((v) => !v)}>
              {streaming ? t("pages.logs.pause") : t("pages.logs.resume")}
            </Button>
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                title={t("pages.logs.prevPage")}
                aria-label={t("pages.logs.prevPage")}
                disabled={page === 0}
                onClick={() => setCursors((stack) => stack.slice(0, -1))}
                className="flex rounded-md border border-[color:var(--border-subtle)] p-[5px] text-[color:var(--text-subtle)] transition-colors enabled:hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <ChevronLeft className="h-3.5 w-3.5" />
              </button>
              <span className="font-mono text-xs text-muted-foreground">
                {t("pages.logs.pageShort", { page: page + 1 })}
              </span>
              <button
                type="button"
                title={t("pages.logs.nextPage")}
                aria-label={t("pages.logs.nextPage")}
                disabled={!hasMore}
                onClick={toNextPage}
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
                <th scope="col" className={TH}>
                  {t("pages.logs.time")}
                </th>
                <th scope="col" className={TH}>
                  {t("pages.logs.model")}
                </th>
                <th scope="col" className={TH}>
                  {t("common.provider")}
                </th>
                <th scope="col" className={TH}>
                  {t("pages.logs.status")}
                </th>
                <th scope="col" className={cn(TH, "text-right")}>
                  {t("pages.logs.latency")}
                </th>
                <th scope="col" className={cn(TH, "text-right")}>
                  {t("pages.logs.tokens")}
                </th>
                <th scope="col" className={cn(TH, "text-right")}>
                  {t("pages.logs.cost")}
                </th>
                <th scope="col" className={TH}>
                  <span className="sr-only">{t("analytics.details")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const st = num(r.status);
                const tone = statusTone(st);
                // the drawer beside the table has no other tie back to the
                // row it describes, so the open row says so, to the eye and
                // to a screen reader (#1983)
                const isOpen = selected != null && rowKey(selected) === rowKey(r);
                return (
                  <tr
                    key={rowKey(r)}
                    aria-selected={isOpen}
                    onClick={() => setSelected(r)}
                    className={cn(
                      "cursor-pointer transition-colors",
                      isOpen
                        ? "bg-[color:var(--surface-selected)]"
                        : "hover:bg-[color:var(--surface-hover)]",
                    )}
                  >
                    <td className={cn(TD, "truncate whitespace-nowrap")}>{fmt.dateTimeMs(r.ts)}</td>
                    <td className={cn(TD, "[overflow-wrap:anywhere]")}>{r.model}</td>
                    <td
                      className={cn(
                        TD,
                        "truncate whitespace-nowrap text-[color:var(--text-secondary)]",
                      )}
                    >
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
              read looked like a deployment with no traffic (#1180). pending
              rather than loading: a retry parked in a hidden tab is pending
              and not fetching, and is still not an answer (#1984) */}
          {query.isPending && (
            <div className="p-3">
              <ListSkeleton rows={8} />
            </div>
          )}
          {/* a full page can be the last one, so the page after it may come
              back empty. that is the end of the log, not a deployment that
              has served nothing, and the way out is back to the newest.
              both empty states need a successful answer: gated on "not
              loading and no error" instead, a failed first load whose retry
              was parked in a hidden tab said "Nothing logged yet" (#1984) */}
          {query.isSuccess && rows.length === 0 && page > 0 && (
            <EmptyState
              uxTarget="request-logs"
              icon={<ScrollText />}
              title={t("pages.logs.endTitle")}
              description={t("pages.logs.endBody")}
              actions={
                <Button variant="outline" onClick={() => setCursors([])}>
                  {t("pages.logs.firstPage")}
                </Button>
              }
            />
          )}
          {query.isSuccess && rows.length === 0 && page === 0 && (
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
            {/* the request id sits in the verdict, where it can be copied, so
                the header names the model the row was opened by */}
            <SheetHeader
              title={t("analytics.details")}
              subtitle={selected.model}
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
              <h2 className="min-w-0 truncate font-mono text-sm">{selected.model}</h2>
              <button
                type="button"
                aria-label={t("pages.logs.closeDetails")}
                onClick={() => setSelected(null)}
                className="ml-auto flex text-[color:var(--text-subtle)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="p-[18px]">{detail}</div>
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
 * A caller who can read `logging_settings` (a superadmin) can be told which it
 * is; for everyone else the answer would be a 403, so the screen does not ask.
 * The link to the settings follows the same gate (#1984): it used to render for
 * everyone and land a member on a refusal, so a caller the gate refuses is told
 * who owns the setting instead. Either way the reader learns where it lives
 * rather than hunting for it.
 *
 * The fourth reason is the caller's own role (#1820): the server blanks a body
 * a viewer may not read and says so with `payload_withheld`, and that one is
 * certain rather than a guess, so it is stated plainly and points at the
 * project's settings rather than the deployment's log settings.
 */
function PayloadBlock({
  label,
  raw,
  withheld = false,
}: {
  label: string;
  raw: string | undefined;
  withheld?: boolean;
}) {
  const { t } = useTranslation();
  const can = useCan();
  const readsSettings = can("logging_settings", "read");
  const body = pretty(raw);
  const settings = useQuery({
    queryKey: ["logging-settings", "payload-hint"],
    queryFn: fetchLoggingSettings,
    // only asked when the answer is readable, and a failure is not worth
    // surfacing: the generic explanation below is still true
    enabled: readsSettings === true && body === null && !withheld,
    retry: false,
    staleTime: 60_000,
  });

  if (body !== null) {
    return <DrawerBlock label={label} content={body} language={payloadLanguage(raw)} />;
  }

  const captureOff = settings.data ? !settings.data.payload_capture_enabled : undefined;
  const reason = withheld
    ? t("pages.logs.payloadWithheld")
    : captureOff === true
      ? t("pages.logs.payloadCaptureOff")
      : captureOff === false
        ? t("pages.logs.payloadCaptureOnButAbsent", {
            hours: settings.data?.payload_retention_hours ?? 0,
          })
        : t("pages.logs.payloadAbsent");

  return (
    <div>
      <h3 className={cn(GROUP_TITLE, "mb-1.5")}>{label}</h3>
      <div className="rounded-[8px] border border-dashed border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3">
        <p className="text-xs leading-relaxed text-muted-foreground">{reason}</p>
        {/* the deployment's log settings cannot change a role, so a withheld
            body has nowhere there to point. only an explicit "no" hides the
            link, the rule the rail follows for the same screen */}
        {!withheld &&
          (readsSettings === false ? (
            <p className="mt-2 text-xs leading-relaxed text-muted-foreground">
              {t("pages.logs.payloadSettingsOwner")}
            </p>
          ) : (
            <Link
              to="/logs-settings"
              className="mt-2 inline-block text-xs font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {t("pages.logs.payloadSettingsLink")}
            </Link>
          ))}
      </div>
    </div>
  );
}

/**
 * What the screen shows on a deployment with no analytics store (#1984).
 *
 * That deployment answered, and the answer will not change until someone sets
 * `CLICKHOUSE_URL`: it is a configuration rolter supports, not an outage. It
 * used to render `LoadError`, whose red `role="alert"` put it in the same voice
 * as a 500 and had a screen reader announce it as urgent on every visit. This
 * is the same information, stated calmly as a `status`: the cause, the setting
 * in monospace, and the control plane's own words under it (#962). There is
 * no retry, because no retry can help.
 */
function AnalyticsUnavailable({ error }: { error: unknown }) {
  const { t } = useTranslation();
  const detail = error instanceof Error ? error.message : null;
  return (
    <div
      role="status"
      className="flex max-w-[72ch] items-start gap-3 rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-3.5"
    >
      <ChartNoAxesColumn
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-info-text)]"
      />
      <div className="flex min-w-0 flex-col gap-2">
        <p className="text-sm font-medium text-foreground">{t("pages.logs.noAnalytics.title")}</p>
        <p className="text-sm leading-relaxed text-muted-foreground">
          <Trans
            i18nKey="pages.logs.noAnalytics.body"
            components={[<code key="env" className="font-mono text-xs text-foreground" />]}
          />
        </p>
        {detail && (
          <p className="break-words font-mono text-xs text-[color:var(--text-subtle)]">{detail}</p>
        )}
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

/**
 * The top of the detail drawer: what happened, when, and the two ids someone
 * quotes to find this request again (#1983).
 *
 * The request id is what a client got back in `x-request-id`; the trace id is
 * the caller's own W3C trace, the hop into a tracing backend. Both are copied
 * far more often than read, so each carries a copy control. A trace id is empty
 * when the caller sent no `traceparent`, and the drawer says so rather than
 * offering to copy nothing.
 */
function Verdict({ row }: { row: InvocationRow }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const status = num(row.status);
  return (
    <div className="flex flex-col gap-3">
      <p className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
        <span className="sr-only">{t("pages.logs.status")}</span>
        {/* status 0 is a request the gateway never got an answer to: a
            refused connection or a timeout, with no http status to show */}
        <Badge
          tone={verdictTone(status)}
          className={cn("text-[0.6875rem] font-semibold", status !== 0 && "font-mono")}
        >
          {status === 0 ? t("pages.logs.detail.noResponse") : status}
        </Badge>
        <time dateTime={row.ts} className="font-mono text-xs text-foreground">
          {fmt.dateTimeMs(row.ts)}
        </time>
      </p>
      <dl className={DETAIL_GRID}>
        <DetailId
          label={t("pages.logs.detail.requestId")}
          value={row.request_id}
          copyLabel={t("pages.logs.detail.copyRequestId")}
          absent={<Absent />}
        />
        <DetailId
          label={t("pages.logs.detail.traceId")}
          value={row.trace_id}
          copyLabel={t("pages.logs.detail.copyTraceId")}
          absent={
            <span className="text-[color:var(--text-subtle)]">
              {t("pages.logs.detail.traceNotSent")}
            </span>
          }
        />
      </dl>
    </div>
  );
}

// label, then value. the values get the wider column, since an id or a
// `provider → target` pair is longer than any label; a long russian label
// wraps onto a second line instead
const DETAIL_GRID =
  "grid grid-cols-[minmax(0,1fr)_minmax(0,2fr)] items-baseline gap-x-3 gap-y-1.5 text-xs";

// the overline every group in the drawer is titled with, payloads included
const GROUP_TITLE = "text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]";

function DetailId({
  label,
  value,
  copyLabel,
  absent,
}: {
  label: string;
  value: string;
  copyLabel: string;
  absent: React.ReactNode;
}) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      {/* wrapped rather than truncated: an id is compared by eye against the
          one a client quoted, and a cut-off one cannot be */}
      <dd className="flex min-w-0 items-start gap-0.5">
        {value ? (
          <>
            <code className="min-w-0 font-mono text-foreground [overflow-wrap:anywhere]">
              {value}
            </code>
            {/* lifted by the difference between the 24px button and the 16px
                line, so its icon sits on the id's first line */}
            <CopyButton value={value} label={copyLabel} className="-mt-1 h-6 flex-none px-1" />
          </>
        ) : (
          absent
        )}
      </dd>
    </>
  );
}

/** A titled group of label/value rows in the detail drawer. */
function DetailSection({ title, children }: { title: string; children: React.ReactNode }) {
  const heading = React.useId();
  return (
    <section aria-labelledby={heading} className="flex flex-col gap-2">
      <h3 id={heading} className={GROUP_TITLE}>
        {title}
      </h3>
      <dl className={DETAIL_GRID}>{children}</dl>
    </section>
  );
}

/**
 * One label/value row. `mono` is for a value someone might paste somewhere —
 * an id, a slug, a number, money — and never for a word like "Hit".
 */
function DetailRow({
  label,
  mono = false,
  children,
}: {
  label: string;
  mono?: boolean;
  children: React.ReactNode;
}) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className={cn("min-w-0 text-foreground [overflow-wrap:anywhere]", mono && "font-mono")}>
        {children}
      </dd>
    </>
  );
}

/** An empty field, quieter than a value so it is not read as one. */
function Absent() {
  return <span className="text-[color:var(--text-subtle)]">—</span>;
}

/** A governance id by its name, falling back to the id itself when no row names it. */
function NamedId({ id, name }: { id: string; name: string | undefined }) {
  if (!id) return <Absent />;
  if (name) return <>{name}</>;
  return <span className="font-mono">{id}</span>;
}

/**
 * The virtual key a request was made with, by its name and prefix (#1983).
 *
 * The row only carries the key's id, which nobody recognises. The project's
 * keys are a viewer's read, so they are asked for; a caller the gate refuses,
 * a key since deleted, or a failed read all fall back to the id, which is still
 * the true answer.
 */
function KeyIdentity({ row }: { row: InvocationRow }) {
  const can = useCan();
  const keys = useQuery({
    queryKey: ["virtual-keys", row.project_id],
    queryFn: () => fetchVirtualKeys(row.project_id),
    enabled: can("virtual_key", "read") !== false && !!row.project_id && !!row.virtual_key_id,
    retry: false,
    staleTime: 60_000,
  });
  if (!row.virtual_key_id) return <Absent />;
  const key = keys.data?.find((k) => k.id === row.virtual_key_id);
  if (!key) return <span className="font-mono">{row.virtual_key_id}</span>;
  return (
    <span className="flex min-w-0 flex-col">
      {key.name && <span className="truncate">{key.name}</span>}
      <code className="font-mono text-[color:var(--text-secondary)]">{key.key_prefix}…</code>
    </span>
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
      <h3 className={cn(GROUP_TITLE, "mb-1.5")}>{label}</h3>
      {/* the payload is the reason the drawer was opened: it reads through the
          shared code block, so a malformed field is visible rather than hidden
          in a wall of monospace (#949). soft-wrapped, because the drawer is
          narrow and a body line is long */}
      <CodeBlock value={content} language={language} label={label} wrap />
    </div>
  );
}
