import { useQuery } from "@tanstack/react-query";
import { X } from "lucide-react";
import * as React from "react";

import { ListHeader, ListRow, ListTable, PageBody, Pill } from "@/components/screen";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/select";
import {
  AnalyticsUnavailableError,
  fetchMcpLogDetail,
  fetchMcpLogs,
  fetchMcpSummary,
  MCP_STATUSES,
  MCP_TRANSPORTS,
  type McpLogRow,
} from "@/lib/api";

const STATUS_TONE: Record<string, [string, string]> = {
  success: ["var(--status-success)", "rgba(22,163,74,.14)"],
  error: ["var(--status-danger)", "var(--red-tint)"],
  timeout: ["var(--status-warning)", "rgba(245,158,11,.14)"],
  auth_denied: ["var(--status-danger)", "var(--red-tint)"],
  transport_error: ["var(--status-warning)", "rgba(245,158,11,.14)"],
};

const statusTone = (s: string) => STATUS_TONE[s] ?? ["var(--text-secondary)", "var(--surface-subtle)"];

const GRID = "150px 1.1fr 1.3fr 130px 110px 90px";

// clickhouse-backed MCP tool-call log explorer: summary KPIs, filterable
// cursor-paginated table, and a per-event detail drawer with redacted payloads
export default function McpLogs() {
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

  const unavailable =
    logs.error instanceof AnalyticsUnavailableError ||
    summary.error instanceof AnalyticsUnavailableError;

  if (unavailable) {
    return (
      <PageBody>
        <div className="rounded-[10px] border border-[color:var(--border-subtle)] p-5 text-sm text-muted-foreground">
          Analytics isn't configured for this deployment — set{" "}
          <code className="font-mono">clickhouse_url</code> on the control plane to record MCP
          tool-call logs.
        </div>
      </PageBody>
    );
  }

  const resetPaging = () => setCursors([]);
  const rows = logs.data?.data ?? [];

  return (
    <PageBody className="h-full min-h-0">
      <div className="grid grid-cols-4 gap-3.5">
        <McpStat label="Calls (24h)" value={summary.data ? String(summary.data.calls) : "—"} />
        <McpStat
          label="Failures"
          value={summary.data ? String(summary.data.failures) : "—"}
        />
        <McpStat
          label="Avg latency"
          value={summary.data ? `${summary.data.avg_latency_ms} ms` : "—"}
        />
        <McpStat
          label="p95 latency"
          value={summary.data ? `${Math.round(summary.data.p95_latency_ms)} ms` : "—"}
        />
      </div>

      <div className="flex items-center gap-2.5">
        <Select
          className="w-[160px]"
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
          {logs.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
          {logs.isError && !unavailable && (
            <p className="text-sm text-muted-foreground">{(logs.error as Error).message}</p>
          )}
          {rows.length === 0 && logs.isSuccess && (
            <p className="text-sm text-muted-foreground">No MCP tool calls in the last 24h.</p>
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
  const tone = statusTone(row.status);
  return (
    <ListRow
      grid={GRID}
      className="cursor-pointer transition-colors hover:bg-[color:var(--surface-subtle)]"
      onClick={onSelect}
    >
      <span className="font-mono text-xs text-[color:var(--text-secondary)]">
        {row.ts.slice(5, 19).replace("T", " ")}
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

  return (
    <aside className="rl-fade-in flex w-[380px] flex-none flex-col gap-3.5 overflow-y-auto rounded-[10px] border border-[color:var(--border-default)] bg-card p-4">
      <div className="flex items-center gap-2">
        <span className="min-w-0 truncate font-mono text-sm font-semibold">
          {d ? `${d.server} → ${d.tool}` : "…"}
        </span>
        <button
          type="button"
          onClick={onClose}
          className="ml-auto text-muted-foreground transition-colors hover:text-foreground"
        >
          <X className="h-4 w-4" />
        </button>
      </div>
      {detail.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {detail.isError && (
        <p className="text-sm text-muted-foreground">{(detail.error as Error).message}</p>
      )}
      {d && (
        <>
          <div className="grid grid-cols-2 gap-2.5">
            <DrawerStat label="Status" value={d.status} />
            <DrawerStat label="Latency" value={`${d.latency_ms} ms`} />
            <DrawerStat label="Transport" value={d.transport} />
            <DrawerStat label="Time" value={d.ts.slice(0, 19).replace("T", " ")} />
            <DrawerStat label="Request" value={d.request_id || "—"} />
            <DrawerStat label="Trace" value={d.trace_id || "—"} />
          </div>
          {d.error && <p className="text-xs text-destructive">{d.error}</p>}
          {pretty(d.arguments) && <DrawerBlock label="Arguments" body={pretty(d.arguments)!} />}
          {pretty(d.result) && <DrawerBlock label="Result" body={pretty(d.result)!} />}
        </>
      )}
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

function DrawerBlock({ label, body }: { label: string; body: string }) {
  return (
    <div>
      <div className="mb-1 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <pre className="max-h-[220px] overflow-auto rounded-[8px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-2.5 font-mono text-[0.6875rem] leading-relaxed text-[color:var(--text-secondary)]">
        {body}
      </pre>
    </div>
  );
}
