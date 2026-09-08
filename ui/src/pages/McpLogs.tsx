import { useQuery } from "@tanstack/react-query";
import { Wrench, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { FormSkeleton, TableSkeleton } from "@/components/LoadingState";
import { ListHeader, ListRow, ListTable, PageBody, Pill } from "@/components/screen";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import { Select } from "@/components/ui/select";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import {
  AnalyticsUnavailableError,
  fetchMcpLogDetail,
  fetchMcpLogs,
  fetchMcpSummary,
  MCP_STATUSES,
  MCP_TRANSPORTS,
  type McpLogRow,
} from "@/lib/api";
import type { CodeLanguage } from "@/lib/code";
import { useFormat } from "@/lib/i18n/format";
import { useDrawerA11y } from "@/lib/use-drawer-a11y";
import { BELOW_LG, useMediaQuery } from "@/lib/use-media-query";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

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

const STATUS_TONE: Record<string, [string, string]> = {
  success: ["var(--status-success-text)", "rgba(22,163,74,.14)"],
  error: ["var(--status-danger-text)", "var(--red-tint)"],
  timeout: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
  auth_denied: ["var(--status-danger-text)", "var(--red-tint)"],
  transport_error: ["var(--status-warning-text)", "rgba(245,158,11,.14)"],
};

const statusTone = (s: string) => STATUS_TONE[s] ?? ["var(--text-secondary)", "var(--surface-subtle)"];

const GRID = "150px 1.1fr 1.3fr 130px 110px 90px";

// clickhouse-backed MCP tool-call log explorer: summary KPIs, filterable
// cursor-paginated table, and a per-event detail drawer with redacted payloads
function McpLogsScreen() {
  const { t } = useTranslation();
  const [status, setStatus] = React.useState("");
  const [transport, setTransport] = React.useState("");
  const [cursors, setCursors] = React.useState<string[]>([]);
  const cursor = cursors[cursors.length - 1];
  const [selected, setSelected] = React.useState<string | null>(null);

  const summary = useQuery({
    queryKey: ["mcp-summary"],
    queryFn: () => fetchMcpSummary({ since: new Date(Date.now() - 86_400_000).toISOString() }),
    retry: false,
  });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `summary` is the query the user is actually waiting on for this screen

  useScreenReady(!summary.isLoading);

  useErrorState(!!summary.error, "mcp-logs");
  const logs = useQuery({
    queryKey: ["mcp-logs", status, transport, cursor],
    queryFn: () =>
      fetchMcpLogs({
        since: new Date(Date.now() - 86_400_000).toISOString(),
        status: status || undefined,
        transport: transport || undefined,
        limit: 50,
        cursor,
      }),
    retry: false,
  });

  // a deployment with no analytics store, or a control plane too old to serve
  // /api/v1/mcp/logs at all: both arrive as AnalyticsUnavailableError, and both
  // are a load state LoadError already knows how to explain (#1236)
  const unavailable =
    logs.error instanceof AnalyticsUnavailableError
      ? logs.error
      : summary.error instanceof AnalyticsUnavailableError
        ? summary.error
        : null;

  if (unavailable) {
    return (
      <PageBody>
        <LoadError error={unavailable} resource={t("errors.resources.mcpLogs")} />
      </PageBody>
    );
  }

  const resetPaging = () => setCursors([]);
  const rows = logs.data?.data ?? [];
  // a filtered page that came back empty is a different answer from a
  // deployment that has never seen an MCP call, and only one of them is fixed
  // by clearing something
  const filtersActive = !!status || !!transport || cursors.length > 0;
  const clearFilters = () => {
    setStatus("");
    setTransport("");
    resetPaging();
  };

  return (
    <PageBody className="h-full min-h-0">
      <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2 lg:grid-cols-4">
        <McpStat label="Calls (24h)" value={summary.data ? String(summary.data.calls) : "—"} />
        <McpStat
          label="Failures"
          value={summary.data ? String(summary.data.failures) : "—"}
        />
        <McpStat label="Avg latency" value={latencyStat(summary.data?.avg_latency_ms)} />
        <McpStat label="p95 latency" value={latencyStat(summary.data?.p95_latency_ms)} />
      </div>

      <div className="flex flex-wrap items-center gap-2.5">
        <Select
          className="w-[160px]"
          aria-label={t("pages.mcpLogs.statusFilterAria")}
          value={status}
          onChange={(e) => {
            setStatus(e.target.value);
            resetPaging();
          }}
        >
          <option value="">all statuses</option>
          {MCP_STATUSES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </Select>
        <Select
          className="w-[180px]"
          aria-label={t("pages.mcpLogs.transportFilterAria")}
          value={transport}
          onChange={(e) => {
            setTransport(e.target.value);
            resetPaging();
          }}
        >
          <option value="">all transports</option>
          {MCP_TRANSPORTS.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </Select>
        <div className="ml-auto flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={cursors.length === 0}
            onClick={() => setCursors((c) => c.slice(0, -1))}
          >
            ← Prev
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!logs.data?.next_cursor || rows.length < 50}
            onClick={() =>
              logs.data?.next_cursor && setCursors((c) => [...c, logs.data.next_cursor as string])
            }
          >
            Next →
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
            />
          )}
          {rows.length === 0 && logs.isSuccess && (
            <EmptyState
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
                    {t("common.clearSearch")}
                  </Button>
                ) : undefined
              }
            />
          )}
          {rows.length > 0 && (
            <ListTable className="max-h-full overflow-y-auto">
              <ListHeader grid={GRID} className="sticky top-0 z-10">
                <span>Time</span>
                <span>Server</span>
                <span>Tool</span>
                <span>Status</span>
                <span>Transport</span>
                <span className="text-right">Latency</span>
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
  const fmt = useFormat();
  const tone = statusTone(row.status);
  return (
    <ListRow
      grid={GRID}
      className="cursor-pointer transition-colors hover:bg-[color:var(--surface-subtle)]"
      onClick={onSelect}
    >
      <span className="font-mono text-xs text-[color:var(--text-secondary)]">
        {fmt.dateTime(row.ts)}
      </span>
      <span className="truncate font-mono text-xs">{row.server}</span>
      <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
        {row.tool}
      </span>
      <Pill color={tone[0]} tint={tone[1]}>
        {row.status}
      </Pill>
      <span className="truncate font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
        {row.transport}
      </span>
      <span className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
        {row.latency_ms} ms
      </span>
    </ListRow>
  );
}

// an empty window averages to null (or nan) upstream; that is "no calls", not
// a latency of "null ms"
function latencyStat(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(Number(value))) return "—";
  return `${Math.round(Number(value))} ms`;
}

function McpStat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-[10px] border border-[color:var(--border-subtle)] bg-card p-4">
      <div className="mb-1 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="font-mono text-xl font-semibold">{value}</div>
    </div>
  );
}

function DetailDrawer({ eventId, onClose }: { eventId: string; onClose: () => void }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  // below `lg` a 380px panel beside the table leaves neither of them usable, so
  // the detail comes over the top as a sheet instead (#1203)
  const asSheet = useMediaQuery(BELOW_LG);
  const drawer = useDrawerA11y(!asSheet, onClose);
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
      {detail.isError && (
        <LoadError
          error={detail.error}
          resource={t("errors.resources.mcpLogDetail")}
          onRetry={() => void detail.refetch()}
        />
      )}
      {d && (
        <>
          <div className="grid grid-cols-1 gap-2.5 sm:grid-cols-2">
            <DrawerStat label="Status" value={d.status} />
            <DrawerStat label="Latency" value={`${d.latency_ms} ms`} />
            <DrawerStat label="Transport" value={d.transport} />
            <DrawerStat label="Time" value={fmt.dateTime(d.ts)} />
            <DrawerStat label="Request" value={d.request_id || "—"} />
            <DrawerStat label="Trace" value={d.trace_id || "—"} />
          </div>
          {d.error && <p className="text-xs text-[color:var(--status-danger-text)]">{d.error}</p>}
          {pretty(d.arguments) && (
            <DrawerBlock
              label="Arguments"
              body={pretty(d.arguments)!}
              language={payloadLanguage(d.arguments)}
            />
          )}
          {pretty(d.result) && (
            <DrawerBlock
              label="Result"
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
          subtitle={d ? `${d.server} → ${d.tool}` : "…"}
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
      className="rl-fade-in flex w-[380px] flex-none flex-col gap-3.5 overflow-y-auto rounded-[10px] border border-[color:var(--border-default)] bg-card p-4 focus-visible:outline-none"
    >
      <div className="flex items-center gap-2">
        <span className="min-w-0 truncate font-mono text-sm font-semibold">
          {d ? `${d.server} → ${d.tool}` : "…"}
        </span>
        <button
          type="button"
          aria-label="Close MCP log details"
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
      <div className="mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="truncate font-mono text-xs text-[color:var(--text-secondary)]">{value}</div>
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
      <div className="mb-1 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      {/* tool arguments and results are JSON almost always and opaque text
          occasionally; the shared block colours the first and leaves the
          second alone (#949) */}
      <CodeBlock
        value={body}
        language={language}
        label={label}
        maxHeight={220}
        density="compact"
      />
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(McpLogsScreen, "errors.resources.mcpLogs");
