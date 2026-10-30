import { useQuery } from "@tanstack/react-query";
import { ChevronRight, SearchX, Wrench, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router";

import { Overline } from "@/components/ui/overline";
import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { LoadError } from "@/components/LoadError";
import { FormSkeleton, TableSkeleton } from "@/components/LoadingState";
import {
  ListActionsHeader,
  ListCell,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListTable,
  PageBody,
  Pill,
} from "@/components/screen";
import type { BadgeTone } from "@/components/ui/badge";
import { LogStepUp } from "@/components/McpStepUp";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { TimeWindowPicker } from "@/components/TimeWindowPicker";
import { Combobox } from "@/components/ui/combobox";
import { CopyableText } from "@/components/ui/copyable-value";
import { EmptyState, EmptyStateLink } from "@/components/ui/empty-state";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import { STAT_GRID, StatCard } from "@/components/ui/stat-card";
import {
  AnalyticsUnavailableError,
  ApiError,
  fetchMcpLogDetail,
  fetchMcpLogs,
  fetchMcpServers,
  fetchMcpSummary,
  MCP_STATUSES,
  MCP_TRANSPORTS,
  type McpLogRow,
} from "@/lib/api";
import { mcpStatusLabel } from "@/lib/enum-labels";
import type { CodeLanguage } from "@/lib/code";
import { useOptionalAuth } from "@/lib/auth";
import { useScope } from "@/lib/scope";
import { useFormat } from "@/lib/i18n/format";
import { useDrawerA11y } from "@/lib/use-drawer-a11y";
import { BELOW_LG, useMediaQuery } from "@/lib/use-media-query";
import { useAddressSelection, useWindowQueryKey, windowBounds } from "@/lib/time-window";
import { useScreenReady } from "@/lib/ux-react";

// a tool payload is JSON when it parses as JSON, and opaque text when it does
// not; highlighting the second as JSON would invent structure that is not there
function payloadLanguage(value: string | null): CodeLanguage {
  if (!value) return "text";
  try {
    JSON.parse(value);
    return "json";
  } catch {
    return "text";
  }
}

const STATUS_TONE: Record<string, BadgeTone> = {
  success: "success",
  error: "danger",
  timeout: "warning",
  auth_denied: "danger",
  transport_error: "warning",
};

const statusTone = (s: string): BadgeTone => STATUS_TONE[s] ?? "neutral";

// a call no reply was awaited for, whose status says nothing about the tool
// (#3526, #3529, #3528): the summary leaves it out of the rates, and a row says why
type NotAwaited = "notification" | "accepted" | "unread";

function notAwaited(row: Pick<McpLogRow, "notification" | "outcome">): NotAwaited | null {
  if (row.outcome === "accepted") return "accepted";
  if (row.outcome === "unread") return "unread";
  if (Number(row.notification ?? 0) === 1) return "notification";
  return null;
}

// the pill of such a row is neutral: it is neither a success nor a failure
// of the tool. an `accepted` call reads as accepted rather than as the
// `success` stored beside it for an older table
function rowStatus(row: McpLogRow, t: ReturnType<typeof useTranslation>["t"]) {
  const note = notAwaited(row);
  const label =
    note === "accepted" || note === "unread"
      ? t(`pages.mcpLogs.statuses.${note}`)
      : mcpStatusLabel(row.status, t);
  const tone = note && row.status === "success" ? "neutral" : statusTone(row.status);
  return { note, label, tone };
}

const GRID = "150px 1.1fr 1.3fr 130px 110px 90px 24px";

// clickhouse-backed MCP tool-call log explorer: summary KPIs, filterable
// cursor-paginated table, and a per-event detail drawer with redacted payloads
function McpLogsScreen() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const ms = (v: number) => t("analytics.ms", { value: fmt.number(Math.round(v)) });
  const [status, setStatus] = React.useState("");
  const [transport, setTransport] = React.useState("");
  // the signed-in account's id, off the session. an open-mode session has no
  // account, so it has no calls of its own to narrow to and no shortcut
  const myId = useOptionalAuth()?.user?.id ?? "";
  const [mine, setMine] = React.useState(false);
  // the server and tool live in the address, so the catalog's "View logs" link
  // lands on a filtered list and a filtered list can be shared
  const [params, setParams] = useSearchParams();
  const server = (params.get("server") ?? "").trim();
  const tool = (params.get("tool") ?? "").trim();
  const setParam = (name: "server" | "tool", value: string) => {
    setParams(
      (prev) => {
        // other parameters are left alone, since the address is not only ours
        const next = new URLSearchParams(prev);
        if (value) next.set(name, value);
        else next.delete(name);
        return next;
      },
      { replace: true },
    );
    setCursors([]);
  };
  // suggestions only: the log keeps slugs of servers since deleted, so a typed
  // value is accepted whether or not the registry lists it. a registry the
  // caller cannot read just leaves the fields free-text
  const { orgId } = useScope();
  const registry = useQuery({
    queryKey: ["mcp-servers", orgId],
    queryFn: () => fetchMcpServers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const servers = Array.isArray(registry.data) ? registry.data : [];
  const toolNames = [
    ...new Set(
      servers
        .filter((entry) => !server || entry.slug === server)
        .flatMap((entry) => (Array.isArray(entry.tools) ? entry.tools : [])),
    ),
  ].sort();
  const [cursors, setCursors] = React.useState<string[]>([]);
  const cursor = cursors[cursors.length - 1];
  // the window is a name the address carries (`?window=7d`, or a custom range of
  // days, `?window=custom&since=…&until=…`, #3407), so a failing call
  // from two days ago can be traced and the view shared (#3300). its bounds are
  // worked out as each read leaves, not when the screen opened
  const [timeWindow, pickWindow] = useAddressSelection();
  // the window's days are the chart zone's, so a zone change reads it again (#3519)
  const windowKey = useWindowQueryKey(timeWindow);
  // a cursor walks the window it was read over, which a zone change moves (#3559)
  React.useEffect(() => setCursors([]), [windowKey[1]]);
  const [selected, setSelected] = React.useState<string | null>(null);

  const summary = useQuery({
    queryKey: ["mcp-summary", ...windowKey],
    queryFn: () => fetchMcpSummary(windowBounds(timeWindow)),
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `summary` is the query the user is actually waiting on for this screen

  useScreenReady(!summary.isLoading);

  const logs = useQuery({
    queryKey: ["mcp-logs", ...windowKey, status, transport, server, tool, mine ? myId : "", cursor],
    queryFn: () =>
      fetchMcpLogs({
        ...windowBounds(timeWindow),
        status: status || undefined,
        transport: transport || undefined,
        server: server || undefined,
        tool: tool || undefined,
        user: mine && myId ? myId : undefined,
        limit: 50,
        cursor,
      }),
    retry: false,
  });

  // a deployment with no analytics store, or a control plane too old to serve
  // /api/v1/mcp/logs at all: both arrive as AnalyticsUnavailableError, and both
  // are a supported shape the screen states calmly rather than a failure (#1236,
  // #2016)
  const unavailable =
    logs.error instanceof AnalyticsUnavailableError
      ? logs.error
      : summary.error instanceof AnalyticsUnavailableError
        ? summary.error
        : null;

  if (unavailable) {
    return (
      <PageBody>
        <AnalyticsUnavailable error={unavailable} i18nKey="pages.mcpLogs.noAnalytics" />
      </PageBody>
    );
  }

  const resetPaging = () => setCursors([]);
  const rows = logs.data?.data ?? [];
  // a filtered page that came back empty is a different answer from a
  // deployment that has never seen an MCP call, and only one of them is fixed
  // by clearing something
  const filtersActive = !!status || !!transport || !!server || !!tool || mine || cursors.length > 0;
  const clearFilters = () => {
    setStatus("");
    setTransport("");
    setMine(false);
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        next.delete("server");
        next.delete("tool");
        return next;
      },
      { replace: true },
    );
    resetPaging();
  };

  return (
    <PageBody className="h-full min-h-0">
      {/* a summary that failed is announced, not dashed: the dash below means
          "not loaded yet" and nothing else (#2165). it records `error_state`
          itself, as the call list's alert does */}
      {summary.isError && (
        // load-error-allow: the four figures have no empty state of their own; the call list pairs as mcp-logs
        <LoadError
          error={summary.error}
          resource={t("errors.resources.mcpSummary")}
          onRetry={() => void summary.refetch()}
          target="mcp-log-summary"
        />
      )}
      <div className={STAT_GRID}>
        {/* a window the deployment proxied nothing in answers with no row at
            all, and that is a count of zero, not an unknown one — the dash is
            for a summary that has not arrived or did not load (#1611) */}
        {/* a notification or an accepted call says nothing about the tool, so
            the failure count and rate are over the awaited calls only and the
            rest are counted apart (#3526) */}
        <StatCard
          label={t("pages.mcpLogs.calls")}
          value={summary.isSuccess ? fmt.number(Number(summary.data?.calls ?? 0)) : "—"}
          delta={
            summary.isSuccess && Number(summary.data?.not_awaited ?? 0) > 0
              ? t("pages.mcpLogs.notAwaitedCount", {
                  count: Number(summary.data?.not_awaited),
                  formatted: fmt.number(Number(summary.data?.not_awaited)),
                })
              : undefined
          }
        />
        <StatCard
          label={t("pages.mcpLogs.failures")}
          value={summary.isSuccess ? fmt.number(Number(summary.data?.failures ?? 0)) : "—"}
          delta={
            summary.isSuccess && summary.data?.failure_rate != null
              ? t("pages.mcpLogs.failureRate", {
                  rate: fmt.percent(Number(summary.data.failure_rate)),
                })
              : undefined
          }
        />
        <StatCard
          label={t("pages.mcpLogs.avgLatency")}
          value={latencyStat(summary.data?.avg_latency_ms, ms)}
        />
        <StatCard
          label={t("pages.mcpLogs.p95Latency")}
          value={latencyStat(summary.data?.p95_latency_ms, ms)}
        />
      </div>

      <div className="flex flex-wrap items-center gap-2.5">
        <TimeWindowPicker
          className="w-[180px]"
          aria-label={t("common.timeWindow.label")}
          value={timeWindow}
          onChange={(next) => {
            pickWindow(next);
            resetPaging();
          }}
        />
        <Combobox
          className="w-[160px]"
          aria-label={t("pages.mcpLogs.statusFilterAria")}
          value={status}
          onChange={(picked) => {
            setStatus(picked);
            resetPaging();
          }}
          options={[
            { value: "", label: t("pages.mcpLogs.allStatuses") },
            ...MCP_STATUSES.map((s) => ({ value: s, label: mcpStatusLabel(s, t) })),
          ]}
        />
        <Combobox
          className="w-[180px]"
          aria-label={t("pages.mcpLogs.transportFilterAria")}
          value={transport}
          onChange={(picked) => {
            setTransport(picked);
            resetPaging();
          }}
          options={[
            { value: "", label: t("pages.mcpLogs.allTransports") },
            ...MCP_TRANSPORTS.map((kind) => ({ value: kind, label: kind })),
          ]}
        />
        <Combobox
          className="w-[180px]"
          aria-label={t("pages.mcpLogs.serverFilterAria")}
          value={server}
          onChange={(picked) => setParam("server", picked.trim())}
          allowCustom
          commitOnBlur
          clearable
          placeholder={t("pages.mcpLogs.allServers")}
          options={servers.map((entry) => ({ value: entry.slug, label: entry.slug }))}
        />
        <Combobox
          className="w-[200px]"
          aria-label={t("pages.mcpLogs.toolFilterAria")}
          value={tool}
          onChange={(picked) => setParam("tool", picked.trim())}
          allowCustom
          commitOnBlur
          clearable
          placeholder={t("pages.mcpLogs.allTools")}
          options={toolNames.map((name) => ({ value: name, label: name }))}
        />
        {/* one click to the caller's own calls, through the `user` filter the
            API already takes. every role that reaches the screen sees it */}
        {myId && (
          <Button
            size="sm"
            variant={mine ? "default" : "outline"}
            aria-pressed={mine}
            onClick={() => {
              setMine((v) => !v);
              resetPaging();
            }}
          >
            {t("pages.mcpLogs.myCalls")}
          </Button>
        )}
        <div className="ml-auto flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={cursors.length === 0}
            onClick={() => setCursors((c) => c.slice(0, -1))}
          >
            {t("pages.mcpLogs.prev")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!logs.data?.next_cursor || rows.length < 50}
            onClick={() =>
              logs.data?.next_cursor && setCursors((c) => [...c, logs.data.next_cursor as string])
            }
          >
            {t("pages.mcpLogs.next")}
          </Button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1 gap-3.5">
        <div className="min-w-0 flex-1">
          {logs.isLoading && <TableSkeleton rows={6} />}
          {logs.isError && !unavailable && (
            <LoadError
              error={logs.error}
              resource={t("errors.resources.mcpLogs")}
              onRetry={() => void logs.refetch()}
              target="mcp-logs"
            />
          )}
          {rows.length === 0 && logs.isSuccess && (
            <EmptyState
              thread
              uxTarget="mcp-logs"
              icon={<Wrench />}
              title={
                filtersActive ? t("pages.mcpLogs.noMatchTitle") : t("pages.mcpLogs.emptyTitle")
              }
              description={
                filtersActive ? t("pages.mcpLogs.noMatchBody") : t("pages.mcpLogs.emptyBody")
              }
              actions={
                filtersActive ? (
                  <Button variant="outline" onClick={clearFilters}>
                    {t("common.clearFilters")}
                  </Button>
                ) : (
                  <EmptyStateLink to="/mcp-catalog">
                    {t("pages.mcpLogs.emptyAction")}
                  </EmptyStateLink>
                )
              }
            />
          )}
          {rows.length > 0 && (
            <ListTable label={t("screens.mcp-logs.title")} className="max-h-full overflow-y-auto">
              <ListHeader grid={GRID} className="sticky top-0 z-10">
                <ListHeaderCell>{t("pages.mcpLogs.time")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpLogs.server")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpLogs.tool")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpLogs.status")}</ListHeaderCell>
                <ListHeaderCell>{t("pages.mcpLogs.transport")}</ListHeaderCell>
                <ListHeaderCell className="text-right">{t("pages.mcpLogs.latency")}</ListHeaderCell>
                <ListActionsHeader label={t("analytics.details")} />
              </ListHeader>
              {rows.map((r) => (
                <McpRow key={r.event_id} row={r} onSelect={() => setSelected(r.event_id)} />
              ))}
            </ListTable>
          )}
        </div>
        {selected && <DetailDrawer eventId={selected} onClose={() => setSelected(null)} />}
      </div>
    </PageBody>
  );
}

function McpRow({ row, onSelect }: { row: McpLogRow; onSelect: () => void }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const { note, label, tone } = rowStatus(row, t);
  return (
    <ListRow
      grid={GRID}
      className="cursor-pointer transition-colors hover:bg-[color:var(--surface-subtle)]"
      onClick={onSelect}
    >
      <ListCell className="font-mono text-xs text-[color:var(--text-secondary)]">
        {fmt.dateTime(row.ts)}
      </ListCell>
      <ListCell className="truncate font-mono text-xs">{row.server}</ListCell>
      <ListCell className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
        {row.tool || t("pages.mcpLogs.noTool")}
      </ListCell>
      <ListCell className="grid gap-0.5">
        <Pill tone={tone}>{label}</Pill>
        {note && (
          <span className="truncate text-2xs text-[color:var(--text-subtle)]">
            {t(`pages.mcpLogs.notAwaited.${note}`)}
          </span>
        )}
      </ListCell>
      <ListCell className="truncate font-mono text-2xs text-[color:var(--text-subtle)]">
        {row.transport}
      </ListCell>
      <ListCell className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
        {t("analytics.ms", { value: fmt.number(Math.round(row.latency_ms)) })}
      </ListCell>
      <ListCell className="grid">
        {/* the row's click target is a mouse convenience; this button is the
            keyboard's and the screen reader's way into the same drawer (#2022),
            named with the time as well since a server's tool repeats down the page */}
        <button
          type="button"
          aria-label={t("pages.mcpLogs.openDetails", {
            tool: row.tool || t("pages.mcpLogs.noTool"),
            server: row.server,
            time: fmt.dateTime(row.ts),
          })}
          onClick={(e) => {
            e.stopPropagation();
            onSelect();
          }}
          className="flex h-6 w-6 items-center justify-center rounded-sm text-[color:var(--text-subtle)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <ChevronRight className="h-4 w-4" />
        </button>
      </ListCell>
    </ListRow>
  );
}

// an empty window averages to null (or nan) upstream; that is "no calls", not
// a latency of "null ms". `ms` renders a finite value through the dashboard
// locale and the `analytics.ms` unit
function latencyStat(value: number | null | undefined, ms: (v: number) => string): string {
  if (value == null || !Number.isFinite(Number(value))) return "—";
  return ms(Number(value));
}

function DetailDrawer({ eventId, onClose }: { eventId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  // below `lg` a 380px panel beside the table leaves neither of them usable, so
  // the detail comes over the top as a sheet instead (#1203)
  const asSheet = useMediaQuery(BELOW_LG);
  const drawer = useDrawerA11y(!asSheet, onClose, eventId);
  const detail = useQuery({
    queryKey: ["mcp-log", eventId],
    queryFn: () => fetchMcpLogDetail(eventId),
    retry: false,
  });
  const d = detail.data;

  const pretty = (value: string | null) => {
    if (!value) return null;
    try {
      return JSON.stringify(JSON.parse(value), null, 2);
    } catch {
      return value;
    }
  };

  const body = (
    <>
      {detail.isLoading && <FormSkeleton fields={3} />}
      {detail.isError &&
        // the store going away between the list and one of its rows is the same
        // answer the list would have given, so it gets the same panel (#2016)
        (detail.error instanceof AnalyticsUnavailableError ? (
          <AnalyticsUnavailable error={detail.error} i18nKey="pages.mcpLogs.noAnalytics" />
        ) : detail.error instanceof ApiError && detail.error.status === 404 ? (
          // the read is scoped: an event that is gone and one outside the
          // caller's orgs, teams and projects both answer 404, and retrying
          // cannot change either, so it is stated rather than offered again
          <EmptyState
            uxTarget="mcp-log-detail"
            icon={<SearchX />}
            title={t("pages.mcpLogs.notFoundTitle")}
            description={t("pages.mcpLogs.notFoundBody")}
            className="px-0 py-6"
          />
        ) : (
          <LoadError
            error={detail.error}
            resource={t("errors.resources.mcpLogDetail")}
            onRetry={() => void detail.refetch()}
            target="mcp-log-detail"
          />
        ))}
      {d && (
        <>
          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
            <DrawerStat label={t("pages.mcpLogs.status")} value={rowStatus(d, t).label} />
            <DrawerStat
              label={t("pages.mcpLogs.latency")}
              value={t("analytics.ms", { value: fmt.number(Math.round(d.latency_ms)) })}
            />
            <DrawerStat label={t("pages.mcpLogs.transport")} value={d.transport} />
            <DrawerStat label={t("pages.mcpLogs.time")} value={fmt.dateTime(d.ts)} />
            <DrawerId
              label={t("pages.mcpLogs.request")}
              value={d.request_id}
              copyLabel={t("pages.mcpLogs.copyRequest")}
            >
              {/* the gateway request this call rode on is a row in the request log */}
              {d.request_id && (
                <Link
                  to={`/logs?request_id=${encodeURIComponent(d.request_id)}`}
                  className="text-xs underline underline-offset-4 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {t("pages.mcpLogs.openInLogs")}
                </Link>
              )}
            </DrawerId>
            <DrawerId
              label={t("pages.mcpLogs.trace")}
              value={d.trace_id}
              copyLabel={t("pages.mcpLogs.copyTrace")}
            />
          </div>
          {notAwaited(d) && (
            <p className="text-xs text-muted-foreground">
              {/* a notification the server refused or failed was not delivered,
                  so the summary counts it as a failure */}
              {notAwaited(d) === "notification" && d.status !== "success"
                ? t("pages.mcpLogs.notAwaitedDetail.notificationFailed")
                : t(`pages.mcpLogs.notAwaitedDetail.${notAwaited(d)}`)}
            </p>
          )}
          {d.error && <p className="text-xs text-[color:var(--status-danger-text)]">{d.error}</p>}
          <LogStepUp detail={d} />
          {Number(d.payload_withheld ?? 0) === 1 && !d.arguments && !d.result && (
            // the server blanked the bodies for this caller's role and says so;
            // that is certain, unlike an empty body, so it is stated plainly
            // rather than left looking like a call with no arguments
            <div>
              <Overline as="h3" className="mb-1">
                {t("pages.mcpLogs.argumentsAndResult")}
              </Overline>
              <div className="rounded-[8px] border border-dashed border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3">
                <p className="text-xs leading-relaxed text-muted-foreground">
                  {t("pages.mcpLogs.payloadWithheld")}
                </p>
              </div>
            </div>
          )}
          {pretty(d.arguments) && (
            <DrawerBlock
              label={t("pages.mcpLogs.arguments")}
              body={pretty(d.arguments)!}
              language={payloadLanguage(d.arguments)}
            />
          )}
          {pretty(d.result) && (
            <DrawerBlock
              label={t("pages.mcpLogs.result")}
              body={pretty(d.result)!}
              language={payloadLanguage(d.result)}
            />
          )}
        </>
      )}
    </>
  );

  if (asSheet) {
    return (
      <Sheet open onOpenChange={(next) => !next && onClose()}>
        <SheetHeader
          title={t("analytics.mcpDetails")}
          subtitle={d ? `${d.server} → ${d.tool || t("pages.mcpLogs.noTool")}` : "…"}
          onClose={onClose}
        />
        <SheetBody>{body}</SheetBody>
      </Sheet>
    );
  }

  return (
    <aside
      {...drawer}
      aria-label={t("analytics.mcpDetails")}
      className="rl-fade-in flex w-[380px] flex-none flex-col gap-3.5 overflow-y-auto rounded-lg border border-[color:var(--border-default)] bg-card p-4 focus-visible:outline-none"
    >
      <div className="flex items-center gap-2">
        <span className="min-w-0 truncate font-mono text-sm font-semibold">
          {d ? `${d.server} → ${d.tool || t("pages.mcpLogs.noTool")}` : "…"}
        </span>
        <button
          type="button"
          aria-label={t("pages.mcpLogs.closeDetails")}
          onClick={onClose}
          className="ml-auto text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
      {body}
    </aside>
  );
}

function DrawerStat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <Overline>{label}</Overline>
      <div className="truncate font-mono text-xs text-[color:var(--text-secondary)]">{value}</div>
    </div>
  );
}

// an id is checked by its end and pasted elsewhere, so it wraps and copies
// rather than truncating; it spans the drawer's row on a two-column grid
function DrawerId({
  label,
  value,
  copyLabel,
  children,
}: {
  label: string;
  value: string | null | undefined;
  copyLabel: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="min-w-0 sm:col-span-2">
      <Overline>{label}</Overline>
      {value ? (
        <CopyableText
          variant="inline"
          value={value}
          copyLabel={copyLabel}
          className="text-xs text-[color:var(--text-secondary)]"
        />
      ) : (
        <div className="font-mono text-xs text-[color:var(--text-secondary)]">—</div>
      )}
      {children}
    </div>
  );
}

function DrawerBlock({
  label,
  body,
  language,
}: {
  label: string;
  body: string;
  language: CodeLanguage;
}) {
  return (
    <div>
      <Overline className="mb-1">{label}</Overline>
      {/* tool arguments and results are JSON almost always and opaque text
          occasionally; the shared block colours the first and leaves the
          second alone (#949) */}
      <CodeBlock value={body} language={language} label={label} maxHeight={220} density="compact" />
    </div>
  );
}

// a scoped read (`mcp_log` is project-scoped, viewer floor): the rail and `Screen`
// refuse a caller the capability table refuses, and the server narrows the rows
// to the orgs, teams and projects the caller holds a role in
export default McpLogsScreen;
