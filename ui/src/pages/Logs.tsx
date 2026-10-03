import { useQuery } from "@tanstack/react-query";
import {
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Filter,
  FilterX,
  ScrollText,
  Search,
  SearchX,
  X,
} from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import {
  FilterCheckList,
  FilterPanel,
  FilterSearchList,
  FilterSection,
} from "@/components/ui/filter-panel";
import { LoadError } from "@/components/LoadError";
import { SavedViews } from "@/components/SavedViews";
import { ListSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { Combobox } from "@/components/ui/combobox";
import { CopyableText } from "@/components/ui/copyable-value";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
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
  type SavedViewFilters,
} from "@/lib/api";
import { useCan } from "@/lib/can";
import type { CodeLanguage } from "@/lib/code";
import { useCurrencyCode } from "@/lib/currency";
import { useScope } from "@/lib/scope";
import { useFormat } from "@/lib/i18n/format";
import { parseLogLookup, type LogLookup } from "@/lib/log-lookup";
import { useModalA11y } from "@/lib/modal-a11y";
import {
  DEFAULT_TIME_WINDOW,
  readTimeWindow,
  useTimeWindowOptions,
  windowBounds,
  type TimeWindow,
} from "@/lib/time-window";
import { useDrawerA11y } from "@/lib/use-drawer-a11y";
import { BELOW_MD, BELOW_XL, useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PAGE_SIZE = 50;
// how often the live feed asks for the newest page
const POLL_MS = 5000;
// the log reads one window, by name, the last 24 hours unless the address names
// another (`?window=7d`). its bounds are worked out as each page is requested (a
// poll, a retry, a filter change), not when the screen mounts, so a tab left
// open keeps reading the window as it is now rather than as it was when it was
// opened (#2315). the name is what the query key carries
type StatusFilter = "all" | "error" | "success";

const num = (v: number | string | undefined): number => {
  const n = Number(v ?? 0);
  return Number.isFinite(n) ? n : 0;
};

// the verdict of a status, in the badge's own tones, for the table and the drawer
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

// an unknown value would be a 400 from the control plane, so an address that
// carries one reads as no status filter rather than as a failed screen
function readStatus(raw: string | null): StatusFilter {
  return raw === "error" || raw === "success" ? raw : "all";
}

type FilterParam =
  | "window"
  | "status"
  | "model"
  | "key"
  | "business_unit"
  | "customer"
  | "unpriced"
  | "request_id"
  | "trace_id";

/**
 * The rail's filters, kept in the address rather than in component state
 * (#1985).
 *
 * A view of the log can then be reloaded, bookmarked or pasted to someone else,
 * and it comes back filtered the same way. The parameters carry the control
 * plane's own names, so the address reads like the query the screen sends.
 * Every write replaces the history entry, so the back button leaves the screen
 * instead of stepping back through each click in the rail.
 *
 * The id a reader pasted lives there too (#1861), as `request_id` or
 * `trace_id`, so `/logs?request_id=…` opens that request. An address that
 * names both reads as the request id, the narrower of the two.
 *
 * `window` names the reporting window and `key` one virtual key's id. Neither
 * has a control in the rail beyond the window picker: they arrive from a saved
 * view (#2452) or a pasted address, and Clear filters drops the key.
 *
 * `unpriced=true` narrows the log to requests with no price (#1986). Any other
 * value reads as the filter left off, the way an unknown status does.
 */
function useLogFilters() {
  const [params, setParams] = useSearchParams();
  const timeWindow = readTimeWindow(params.get("window"));
  const status = readStatus(params.get("status"));
  const keyId = (params.get("key") ?? "").trim();
  const model = params.get("model") ?? "";
  const unitParam = params.get("business_unit") ?? "";
  const customerParam = params.get("customer") ?? "";
  const unpriced = params.get("unpriced") === "true";
  const requestId = (params.get("request_id") ?? "").trim();
  const traceId = (params.get("trace_id") ?? "").trim();
  // memoised on the raw value: a fresh array every render would look like a
  // changed filter to anything that depends on it
  const units = React.useMemo(() => unitParam.split(",").filter(Boolean), [unitParam]);
  const customers = React.useMemo(() => customerParam.split(",").filter(Boolean), [customerParam]);
  const lookup = React.useMemo<LogLookup | null>(
    () =>
      requestId
        ? { kind: "request_id", value: requestId }
        : traceId
          ? { kind: "trace_id", value: traceId }
          : null,
    [requestId, traceId],
  );
  const update = React.useCallback(
    (patch: Partial<Record<FilterParam, string>>) =>
      setParams(
        (prev) => {
          // other parameters are left alone, since the address is not only the rail's
          const next = new URLSearchParams(prev);
          for (const [name, value] of Object.entries(patch)) {
            if (value) next.set(name, value);
            else next.delete(name);
          }
          return next;
        },
        { replace: true },
      ),
    [setParams],
  );
  return {
    window: timeWindow,
    status,
    keyId,
    model,
    units,
    customers,
    unpriced,
    /** the id being looked up, if any */
    lookup,
    /** changes whenever any filter does */
    key: [
      timeWindow,
      status,
      keyId,
      model,
      unitParam,
      customerParam,
      unpriced,
      lookup?.kind,
      lookup?.value,
    ].join("|"),
    setWindow: (next: TimeWindow) => update({ window: next === DEFAULT_TIME_WINDOW ? "" : next }),
    /**
     * Replace every filter a saved view holds with the view's own, and leave
     * the rest of the address alone. A filter the view does not name is
     * cleared: applying it must give the view, not the view plus what was
     * already picked. A lookup would mask the filters, so it goes too.
     */
    applyView: (view: SavedViewFilters) =>
      update({
        window: view.window && view.window !== DEFAULT_TIME_WINDOW ? view.window : "",
        status: view.status && view.status !== "all" ? view.status : "",
        model: view.model ?? "",
        key: view.key ?? "",
        business_unit: (view.business_unit ?? []).join(","),
        customer: (view.customer ?? []).join(","),
        request_id: "",
        trace_id: "",
      }),
    setStatus: (next: StatusFilter) => update({ status: next === "all" ? "" : next }),
    setModel: (next: string) => update({ model: next }),
    setKey: (next: string) => update({ key: next }),
    setUnits: (next: string[]) => update({ business_unit: next.join(",") }),
    setCustomers: (next: string[]) => update({ customer: next.join(",") }),
    setUnpriced: (next: boolean) => update({ unpriced: next ? "true" : "" }),
    clear: () =>
      update({ status: "", model: "", key: "", business_unit: "", customer: "", unpriced: "" }),
    setLookup: (next: LogLookup | null) =>
      update({
        request_id: next?.kind === "request_id" ? next.value : "",
        trace_id: next?.kind === "trace_id" ? next.value : "",
        // a new lookup starts from the whole log: the rail's picks would
        // narrow it without the field saying so
        ...(next
          ? { status: "", model: "", key: "", business_unit: "", customer: "", unpriced: "" }
          : {}),
      }),
  };
}

// the columns a reader cannot do without stay at every width: time, model,
// status and cost, then the way into the drawer. the others give way as the
// table narrows, the widest to go first, and each is also in the drawer. it is
// the table's own width that decides rather than the window's, because the
// sidebar, the filter rail and the detail drawer all take width the window
// still reports as free, so a table that kept its columns scrolled Cost out of
// view (#1986). `@container` is on the scroll area below
const TH =
  "sticky top-0 z-[1] whitespace-nowrap border-b border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-2 py-2.5 text-left text-xs font-medium text-muted-foreground @min-[480px]:px-3";
const TD =
  "border-b border-[color:var(--border-subtle)] px-2 py-[9px] font-mono text-xs @min-[480px]:px-3";
// below 480px the table is the wrong shape for a request, so each row stacks
// instead: model and status on the first line, time and cost on the second,
// the chevron beside both (#2446). the table elements stay, so the header and
// the cells keep their names; the header is only taken out of sight. the
// model gets what the fixed cells leave and is cut with an ellipsis rather than
// wrapped, its full name in `title` and in the chevron's accessible name.
// every class is written out whole: tailwind cannot see one built from parts
const TR_STACKED =
  "@max-[479px]:grid @max-[479px]:grid-cols-[minmax(0,1fr)_auto_1.75rem] @max-[479px]:items-center @max-[479px]:gap-x-2 @max-[479px]:border-b @max-[479px]:border-[color:var(--border-subtle)] @max-[479px]:px-3 @max-[479px]:py-2";
const TD_STACKED = "@max-[479px]:border-b-0 @max-[479px]:p-0";
const PROVIDER_COL = "hidden @min-[840px]:table-cell";
const TOKENS_COL = "hidden @min-[720px]:table-cell";
const LATENCY_COL = "hidden @min-[600px]:table-cell";

// LLM logs: collapsible filter rail, full-height
// streaming request table with sticky headers, and a right detail drawer with
// the raw request/response payloads. `pollMs` is only ever set by a story, so
// a play can watch several polling intervals pass inside the test-runner's
// per-story budget
export default function Logs({ pollMs = POLL_MS }: { pollMs?: number }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const filters = useLogFilters();
  const {
    window: logWindow,
    status,
    keyId,
    model,
    units: unitSel,
    customers: customerSel,
    unpriced,
    lookup,
  } = filters;
  const windowOptions = useTimeWindowOptions();
  const lookupKey = lookup ? `${lookup.kind}:${lookup.value}` : "";
  // the cursor each page after the first was opened with, oldest first. a
  // stack rather than a page index: the control plane pages on a keyset, so
  // "previous" has to return to a cursor it was handed rather than compute a
  // row offset, which the gateway's writes would shift (#1410, #1411)
  const [cursors, setCursors] = React.useState<string[]>([]);
  const page = cursors.length;
  const [selected, setSelected] = React.useState<InvocationRow | null>(null);
  // below `md` the 248px filter rail would leave the table 127px. the 380px
  // detail drawer beside the 232px sidebar leaves it 412px at `lg`, and 164px
  // with the rail open too, which is where Cost scrolled out of view (#1203,
  // #1986), so the drawer is a sheet below `xl`. both become overlays at those
  // widths: the same panels, out of the flow
  const railOverlays = useMediaQuery(BELOW_MD);
  const detailAsSheet = useMediaQuery(BELOW_XL);
  const drawer = useDrawerA11y(selected != null && !detailAsSheet, () => setSelected(null));
  const filterPanel = React.useRef<HTMLDivElement>(null);
  const filterA11y = useModalA11y(filterPanel, {
    open: railOverlays && filtersOpen,
    onEscape: () => setFiltersOpen(false),
  });
  const [streaming, setStreaming] = React.useState(true);
  const errorHeading = React.useId();

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

  // the keys the caller can read in the scoped project, which is the list the
  // key picker offers and the drawer already asks for (#1983). a caller the
  // gate refuses gets no picker rather than a list that always fails
  const can = useCan();
  const keys = useQuery({
    queryKey: ["virtual-keys", scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: can("virtual_key", "read") !== false && !!scope.projectId,
    retry: false,
    staleTime: 60_000,
  });

  React.useEffect(() => setCursors([]), [filters.key]);
  // the drawer belongs to the lookup it was opened by
  React.useEffect(() => setSelected(null), [lookupKey]);

  const query = useQuery({
    queryKey: [
      "invocations",
      lookup ? "lookup" : logWindow,
      status,
      keyId,
      model,
      unitSel.join(","),
      customerSel.join(","),
      unpriced,
      cursors[page - 1] ?? "",
      lookupKey,
    ],
    queryFn: () =>
      fetchInvocationsPage({
        // an id names one request wherever it sits in the retained log, so a
        // lookup carries no window: the control plane reads an id with no
        // `since` as every retained row, and a 24 hour bound would answer an
        // older request with an empty page that reads as "no such request"
        ...(lookup ? {} : windowBounds(logWindow)),
        request_id: lookup?.kind === "request_id" ? lookup.value : undefined,
        trace_id: lookup?.kind === "trace_id" ? lookup.value : undefined,
        model: model || undefined,
        key: keyId || undefined,
        // the rail allows several of each, so the whole selection travels
        business_unit: unitSel.length ? unitSel : undefined,
        customer: customerSel.length ? customerSel : undefined,
        unpriced: unpriced || undefined,
        status,
        limit: PAGE_SIZE,
        cursor: cursors[page - 1],
      }),
    retry: (n, error) => !isUnavailable(error) && n < 2,
    // the page on screen stays up while the next one loads, but only when it
    // answers the same question: rows from the feed are not a lookup's rows,
    // and a lookup holds nothing back while it is out
    placeholderData: (prev, prevQuery) =>
      !lookup && prevQuery?.queryKey[1] === logWindow ? prev : undefined,
    // a lookup reads every retained row, so it is asked again on request
    // (Find) rather than on a timer or on every return to the tab
    refetchOnWindowFocus: !lookup,
    // a query that has never held data goes back to pending on every refetch,
    // which unmounts its error: polling one that failed swapped the alert for
    // a skeleton and back every cycle, and a screen reader heard the alert
    // again each time (#1984). so the feed stops there and waits for the
    // retry button. a failure with rows already on screen keeps its error
    // through a refetch, so that one goes on polling and says it is retrying
    refetchInterval: (q) =>
      streaming && !lookup && !(q.state.status === "error" && q.state.data === undefined)
        ? pollMs
        : false,
  });

  // UX stream (#805); the screen key comes from the enclosing UxScreenProvider.
  // both follow the log read, the one this screen exists for: `models` only
  // feeds the rail's model picker, so keying off it reported the screen ready
  // over a skeleton and missed a ClickHouse outage altogether (#2017). ready
  // means answered rather than not loading, because a retry parked in a hidden
  // tab is pending and not fetching and is still no answer. a deployment with no
  // analytics store is an answer and a supported one, so it is neither pending
  // nor an error state. the region is named like the empty state's, so the two
  // pair up in the dead-states query
  useScreenReady(!query.isPending);
  useErrorState(query.isError && !isUnavailable(query.error), "request-logs");

  // the one row a lookup finds is what was asked for, so its drawer opens. it
  // opens once per lookup: a refetch, or closing the drawer, must not bring it
  // back. Find on the lookup already showing clears the mark to ask again
  const opened = React.useRef("");
  const found = query.data?.data;
  React.useEffect(() => {
    if (!lookupKey) {
      opened.current = "";
      return;
    }
    if (!query.isSuccess || opened.current === lookupKey) return;
    opened.current = lookupKey;
    if (found?.length === 1) setSelected(found[0]);
  }, [lookupKey, query.isSuccess, query.dataUpdatedAt, found]);

  // what the field shows follows the address, which a link or the command
  // palette can change from outside
  const [draft, setDraft] = React.useState(lookup?.value ?? "");
  React.useEffect(() => setDraft(lookup?.value ?? ""), [lookup?.value]);
  const lookupField = React.useRef<HTMLInputElement>(null);
  const submitLookup = (event: React.FormEvent) => {
    event.preventDefault();
    const next = parseLogLookup(draft);
    if (!next) return;
    // a traceparent pasted whole is shown as the trace id read out of it
    setDraft(next.value);
    if (next.kind === lookup?.kind && next.value === lookup.value) {
      opened.current = "";
      void query.refetch();
      return;
    }
    filters.setLookup(next);
  };
  const clearLookup = () => {
    setDraft("");
    filters.setLookup(null);
    lookupField.current?.focus();
  };

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
    (status === "all" ? 0 : 1) +
    (model ? 1 : 0) +
    (keyId ? 1 : 0) +
    unitSel.length +
    customerSel.length +
    (unpriced ? 1 : 0);
  // what a saved view keeps of the screen: the lookup, the page and the
  // unpriced flag are not filters it holds, and an `all` status or an empty
  // value is left out rather than saved as a filter that filters nothing
  const savedFilters: SavedViewFilters = {
    window: logWindow,
    ...(status !== "all" ? { status } : {}),
    ...(model ? { model } : {}),
    ...(keyId ? { key: keyId } : {}),
    ...(unitSel.length ? { business_unit: unitSel } : {}),
    ...(customerSel.length ? { customer: customerSel } : {}),
  };
  // the list the model filter picks from. a model the address names but the
  // catalogue no longer lists still filters the log, so it is offered too:
  // otherwise the control would read as unset while the rows are narrowed
  const modelOptions = React.useMemo(() => {
    const names = (models.data ?? []).map((m) => m.model);
    if (model && !names.includes(model)) names.push(model);
    return names.map((name) => ({ value: name, label: name }));
  }, [models.data, model]);

  // the list the key filter picks from, by name. a key the address names but
  // this project's list does not hold (a saved view from another project, a
  // pasted link) still filters the log, so it is offered by its id too
  const keyOptions = React.useMemo(() => {
    const options = (keys.data ?? []).map((k) => ({
      value: k.id,
      label: k.name ? `${k.name} (${k.key_prefix}…)` : `${k.key_prefix}…`,
    }));
    if (keyId && !options.some((o) => o.value === keyId)) {
      options.push({ value: keyId, label: keyId });
    }
    return options;
  }, [keys.data, keyId]);

  // a deployment with no analytics store is a shape rolter supports, not a
  // failure, so it gets a calm panel naming the setting rather than the red
  // alert a 500 gets (#1236, #1984). no retry, since none can help
  if (isUnavailable(query.error)) {
    return (
      <div className="p-[22px]">
        <AnalyticsUnavailable error={query.error} i18nKey="pages.logs.noAnalytics" />
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
      : lookup
        ? {
            // a lookup is an answer, not a feed, so nothing pulses
            dot: "bg-[color:var(--text-subtle)]",
            label: `${t(
              lookup.kind === "trace_id"
                ? "pages.logs.lookup.feedTrace"
                : "pages.logs.lookup.feedRequest",
            )} · ${t("pages.logs.requests", { count: rows.length })}`,
          }
        : {
            dot: streaming
              ? "rl-pulse bg-[color:var(--status-success)]"
              : "bg-[color:var(--text-subtle)]",
            label: `${streaming ? t("pages.logs.streaming") : t("pages.logs.paused")} · ${t(
              "pages.logs.requests",
              { count: rows.length },
            )}`,
          };

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
          {cost(selected) ?? (
            // room enough here to say it, so the explanation is text on the
            // screen and not a tooltip a keyboard or a touch never reaches
            <span className="flex flex-col items-start gap-1.5">
              <Unpriced />
              <span className="font-sans text-xs text-muted-foreground">
                {t("analytics.unpricedHint")}
              </span>
            </span>
          )}
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
      <Payloads row={selected} />
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
            {/* always drawn, so the sections below never move when the first
                filter is picked; it only enables once there is one to clear */}
            <Button
              variant="ghost"
              size="sm"
              disabled={filterCount === 0}
              onClick={filters.clear}
              className="w-full justify-start px-2 text-muted-foreground"
            >
              <FilterX aria-hidden className="h-3.5 w-3.5" />
              {t("common.clearFilters")}
            </Button>
            {/* one choice of three rather than a pair of checkboxes, which
                cleared both ticks without a word when a reader checked both
                (#1985) */}
            <FilterSection
              title={t("pages.logs.status")}
              defaultOpen
              count={status === "all" ? 0 : 1}
            >
              <Segmented
                ariaLabel={t("pages.logs.status")}
                value={status}
                onChange={filters.setStatus}
                options={[
                  { value: "all", label: t("pages.logs.statusAll") },
                  { value: "error", label: t("pages.logs.statusErrors") },
                  { value: "success", label: t("pages.logs.statusOk") },
                ]}
              />
              {/* the control plane's `success` is any status from 1 to 399,
                  so the label cannot promise a 2xx */}
              <p className="text-xs leading-relaxed text-muted-foreground">
                {t("pages.logs.statusOkHint")}
              </p>
            </FilterSection>
            {/* the window is a name the address carries, so a saved view can hold it */}
            <FilterSection
              title={t("pages.logs.window")}
              defaultOpen
              count={logWindow === DEFAULT_TIME_WINDOW ? 0 : 1}
            >
              <Combobox
                aria-label={t("pages.logs.window")}
                options={windowOptions}
                value={logWindow}
                onChange={(next) => filters.setWindow(readTimeWindow(next))}
              />
            </FilterSection>
            {/* the control plane filters on one exact model, so this picks one */}
            <FilterSection title={t("pages.logs.model")} defaultOpen count={model ? 1 : 0}>
              <Combobox
                aria-label={t("pages.logs.model")}
                options={modelOptions}
                value={model}
                onChange={filters.setModel}
                placeholder={t("pages.logs.allModels")}
                clearable
              />
            </FilterSection>
            {/* the control plane filters on one exact key id, shown by its name */}
            {keyOptions.length > 0 && (
              <FilterSection title={t("pages.logs.virtualKey")} defaultOpen count={keyId ? 1 : 0}>
                <Combobox
                  aria-label={t("pages.logs.virtualKey")}
                  options={keyOptions}
                  value={keyId}
                  onChange={filters.setKey}
                  placeholder={t("pages.logs.allKeys")}
                  clearable
                />
              </FilterSection>
            )}
            {/* the flag the gateway recorded per request, applied by the server
                before the page is cut like every other filter here (#1986) */}
            <FilterSection title={t("pages.logs.cost")} defaultOpen count={unpriced ? 1 : 0}>
              <FilterCheckList
                options={[{ value: "unpriced", label: t("pages.logs.unpricedOnly") }]}
                selected={unpriced ? ["unpriced"] : []}
                onChange={(picked) => filters.setUnpriced(picked.includes("unpriced"))}
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
                  onChange={filters.setUnits}
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
                  onChange={filters.setCustomers}
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
        {/* wraps: at 375px in russian the feed's own words, the pause button and the
            pager do not fit one row, and the label was squeezed under the button */}
        <div className="flex flex-none flex-wrap items-center gap-x-2.5 gap-y-2 px-[18px] py-3">
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
          <SavedViews surface="llm_logs" current={savedFilters} onApply={filters.applyView} />
          <span className="inline-flex min-w-0 items-center gap-[7px] text-xs text-muted-foreground">
            <span className={cn("h-[7px] w-[7px] flex-none rounded-full", feed.dot)} />
            {feed.label}
          </span>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            {/* a lookup does not stream, so there is nothing to pause */}
            {!lookup && (
              <Button size="sm" variant="outline" onClick={() => setStreaming((v) => !v)}>
                {streaming ? t("pages.logs.pause") : t("pages.logs.resume")}
              </Button>
            )}
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

        {/* the id a client was handed, pasted here: a request id or a trace
            id, told apart by its shape. it lives in the address like the
            rail's filters, and Clear (or the empty result's button) returns
            to the feed (#1861) */}
        <form
          role="search"
          aria-label={t("pages.logs.lookup.label")}
          onSubmit={submitLookup}
          className="flex flex-none items-center gap-2 border-b border-[color:var(--border-subtle)] px-[18px] pb-3"
        >
          <div className="relative min-w-0 flex-1 sm:max-w-md">
            <Search
              aria-hidden
              className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[color:var(--text-subtle)]"
            />
            <Input
              ref={lookupField}
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              aria-label={t("pages.logs.lookup.label")}
              placeholder={t("pages.logs.lookup.placeholder")}
              autoComplete="off"
              autoCapitalize="off"
              spellCheck={false}
              enterKeyHint="search"
              className="h-8 pl-8 pr-8 font-mono text-xs placeholder:font-sans"
            />
            {draft && (
              <button
                type="button"
                aria-label={t("pages.logs.lookup.clearField")}
                onClick={clearLookup}
                className="absolute right-0.5 top-1/2 flex h-7 w-7 -translate-y-1/2 items-center justify-center rounded-sm text-[color:var(--text-subtle)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X aria-hidden className="h-3.5 w-3.5" />
              </button>
            )}
          </div>
          <Button type="submit" size="sm" variant="outline" disabled={!draft.trim()}>
            {t("pages.logs.lookup.find")}
          </Button>
        </form>

        <div className="@container relative min-h-0 flex-1 overflow-auto">
          <table className="w-full min-w-[320px] table-fixed border-collapse text-sm @max-[479px]:block">
            <colgroup>
              <col className="w-[104px] @min-[480px]:w-28" />
              {/* what is left once the fixed ones have theirs */}
              <col />
              <col className="hidden w-[148px] @min-[840px]:table-column" />
              <col className="w-[58px] @min-[480px]:w-[68px]" />
              <col className="hidden w-24 @min-[600px]:table-column" />
              <col className="hidden w-[88px] @min-[720px]:table-column" />
              <col className="w-[84px] @min-[480px]:w-24" />
              <col className="w-7 @min-[480px]:w-9" />
            </colgroup>
            <thead className="@max-[479px]:sr-only">
              <tr>
                <th scope="col" className={TH}>
                  {t("pages.logs.time")}
                </th>
                <th scope="col" className={TH}>
                  {t("pages.logs.model")}
                </th>
                <th scope="col" className={cn(TH, PROVIDER_COL)}>
                  {t("common.provider")}
                </th>
                <th scope="col" className={TH}>
                  {t("pages.logs.status")}
                </th>
                <th scope="col" className={cn(TH, LATENCY_COL, "text-right")}>
                  {t("pages.logs.latency")}
                </th>
                <th scope="col" className={cn(TH, TOKENS_COL, "text-right")}>
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
            <tbody className="@max-[479px]:block">
              {rows.map((r) => {
                const st = num(r.status);
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
                      TR_STACKED,
                      isOpen
                        ? "bg-[color:var(--surface-selected)]"
                        : "hover:bg-[color:var(--surface-hover)]",
                    )}
                  >
                    {/* the clock and its milliseconds, which tell rows apart.
                        the day is in the title and in the drawer's verdict,
                        not repeated on every row of a 24 hour window */}
                    <td
                      className={cn(
                        TD,
                        TD_STACKED,
                        "truncate whitespace-nowrap @max-[479px]:col-start-1 @max-[479px]:row-start-2 @max-[479px]:text-[color:var(--text-secondary)]",
                      )}
                    >
                      <time dateTime={r.ts} title={fmt.dateTimeMs(r.ts)}>
                        {fmt.timeMs(r.ts)}
                      </time>
                    </td>
                    <td
                      title={r.model}
                      className={cn(
                        TD,
                        TD_STACKED,
                        "[overflow-wrap:anywhere] @max-[479px]:col-start-1 @max-[479px]:row-start-1 @max-[479px]:truncate @max-[479px]:whitespace-nowrap @max-[479px]:text-sm",
                      )}
                    >
                      {r.model}
                    </td>
                    <td
                      className={cn(
                        TD,
                        PROVIDER_COL,
                        "truncate whitespace-nowrap text-[color:var(--text-secondary)]",
                      )}
                    >
                      {r.provider || "—"}
                    </td>
                    <td
                      className={cn(
                        TD,
                        TD_STACKED,
                        "@max-[479px]:col-start-2 @max-[479px]:row-start-1 @max-[479px]:justify-self-end",
                      )}
                    >
                      <Badge
                        tone={verdictTone(st)}
                        className="font-mono text-[0.6875rem] font-semibold"
                      >
                        {st || "ERR"}
                      </Badge>
                    </td>
                    <td
                      className={cn(
                        TD,
                        LATENCY_COL,
                        "text-right text-[color:var(--text-secondary)]",
                      )}
                    >
                      {t("analytics.ms", { value: fmt.number(Math.round(num(r.latency_ms))) })}
                    </td>
                    <td
                      className={cn(
                        TD,
                        TOKENS_COL,
                        "text-right text-[color:var(--text-secondary)]",
                      )}
                    >
                      {fmt.number(num(r.total_tokens))}
                    </td>
                    <td
                      className={cn(
                        TD,
                        TD_STACKED,
                        "@max-[479px]:col-start-2 @max-[479px]:row-start-2",
                        "whitespace-nowrap text-right text-[color:var(--text-secondary)]",
                      )}
                    >
                      {cost(r) ?? <Unpriced titled />}
                    </td>
                    <td
                      className={cn(
                        TD,
                        TD_STACKED,
                        "@max-[479px]:col-start-3 @max-[479px]:row-span-2 @max-[479px]:row-start-1 @max-[479px]:justify-self-end",
                        "pl-0 pr-2 @min-[480px]:pl-0 @min-[480px]:pr-2.5 text-right",
                      )}
                    >
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
                        className="-my-[5px] -mr-[5px] ml-auto flex rounded-sm p-[5px] text-[color:var(--text-subtle)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
          {/* an id that matches nothing and an id the caller may not read are the
              same answer: the control plane filters by visibility in the query,
              so it cannot say which, and neither does this (#1861) */}
          {query.isSuccess && rows.length === 0 && page === 0 && lookup && (
            <EmptyState
              uxTarget="request-logs"
              icon={<SearchX />}
              title={t("pages.logs.lookup.missTitle")}
              description={
                filterCount
                  ? t("pages.logs.lookup.missFilteredBody")
                  : t("pages.logs.lookup.missBody")
              }
              actions={
                <>
                  <Button variant="outline" onClick={clearLookup}>
                    {t("pages.logs.lookup.clear")}
                  </Button>
                  {filterCount > 0 && (
                    <Button variant="outline" onClick={filters.clear}>
                      {t("common.clearFilters")}
                    </Button>
                  )}
                </>
              }
            />
          )}
          {query.isSuccess && rows.length === 0 && page === 0 && !lookup && (
            <EmptyState
              uxTarget="request-logs"
              icon={<ScrollText />}
              title={filterCount ? t("pages.logs.noMatchTitle") : t("pages.logs.emptyTitle")}
              description={filterCount ? t("pages.logs.noMatchBody") : t("pages.logs.emptyBody")}
              actions={
                filterCount ? (
                  <Button variant="outline" onClick={filters.clear}>
                    {t("common.clearFilters")}
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
 * The request and response bodies in the detail drawer, and, when one or both
 * are missing, why (#954, #2131).
 *
 * Two bodies that are missing for the same reason are explained once, under a
 * heading that names both. The row does not record the reasons apart, and the
 * gateway stores a request's two bodies together, so a request with neither
 * almost always has the same story for each, and printing it per panel made the
 * drawer about a screen longer for nothing. A request that has one body is
 * different: the panel for the body that is there says nothing, and the one for
 * the body that is not says what is left to say about it.
 */
function Payloads({ row }: { row: InvocationRow }) {
  const { t } = useTranslation();
  const request = pretty(row.request_payload);
  const response = pretty(row.response_payload);
  const withheld = Number(row.payload_withheld ?? 0) === 1;

  if (request !== null && response !== null) {
    return (
      <>
        <DrawerBlock
          label={t("pages.logs.request")}
          content={request}
          language={payloadLanguage(row.request_payload)}
        />
        <DrawerBlock
          label={t("pages.logs.response")}
          content={response}
          language={payloadLanguage(row.response_payload)}
        />
      </>
    );
  }
  if (request === null && response === null) {
    return <PayloadMissing label={t("pages.logs.requestAndResponse")} withheld={withheld} />;
  }
  return request !== null ? (
    <>
      <DrawerBlock
        label={t("pages.logs.request")}
        content={request}
        language={payloadLanguage(row.request_payload)}
      />
      <PayloadMissing label={t("pages.logs.response")} emptyKey="pages.logs.payloadResponseEmpty" />
    </>
  ) : (
    <>
      <PayloadMissing label={t("pages.logs.request")} emptyKey="pages.logs.payloadRequestEmpty" />
      <DrawerBlock
        label={t("pages.logs.response")}
        content={response ?? ""}
        language={payloadLanguage(row.response_payload)}
      />
    </>
  );
}

/**
 * Why a body is not in the drawer.
 *
 * With neither body there are four reasons. A payload is absent when capture is
 * off, when it is on but this request's model or key falls outside the
 * allow-list, or when the payload's retention window (shorter than the log
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
 *
 * With one body stored the three guesses are gone: the other body was captured,
 * so capture was on, the request passed the allow-list and the retention window
 * is still open. The gateway keeps nothing for a body that was empty when it
 * was logged, and that is what is left, so the note says it and sends the
 * reader nowhere.
 */
function PayloadMissing({
  label,
  emptyKey,
  withheld = false,
}: {
  label: string;
  /** set when only this one body is missing and the other is stored: what to say of it */
  emptyKey?: "pages.logs.payloadRequestEmpty" | "pages.logs.payloadResponseEmpty";
  withheld?: boolean;
}) {
  const { t } = useTranslation();
  const can = useCan();
  const readsSettings = can("logging_settings", "read");
  const neither = emptyKey === undefined;
  const settings = useQuery({
    queryKey: ["logging-settings", "payload-hint"],
    queryFn: fetchLoggingSettings,
    // only asked when the answer is readable, and a failure is not worth
    // surfacing: the generic explanation below is still true
    enabled: readsSettings === true && neither && !withheld,
    retry: false,
    staleTime: 60_000,
  });

  const captureOff = settings.data ? !settings.data.payload_capture_enabled : undefined;
  const reason = emptyKey
    ? t(emptyKey)
    : withheld
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
            body has nowhere there to point, and cannot fill a body that was
            empty. only an explicit "no" hides the link, the rule the rail
            follows for the same screen */}
        {neither &&
          !withheld &&
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
      <dd className="min-w-0">
        {value ? <CopyableText variant="inline" value={value} copyLabel={copyLabel} /> : absent}
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

/**
 * The label a request with no price carries where its cost would be (#1986).
 *
 * A dash reads as "nothing to show", the same as a missing provider, and a zero
 * claims the request was free, so it says what is true: no price was configured
 * when it ran. The table has no room for the explanation and carries it as a
 * title; the drawer has, and prints it beside the label instead.
 */
function Unpriced({ titled = false }: { titled?: boolean }) {
  const { t } = useTranslation();
  return (
    <Badge
      tone="neutral"
      className="font-mono"
      title={titled ? t("analytics.unpricedHint") : undefined}
    >
      {t("analytics.unpriced")}
    </Badge>
  );
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
