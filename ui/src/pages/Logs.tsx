import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight, Filter, X } from "lucide-react";
import * as React from "react";

import {
  FilterCheckList,
  FilterPanel,
  FilterSearchList,
  FilterSection,
} from "@/components/ui/filter-panel";
import { Button } from "@/components/ui/button";
import {
  AnalyticsUnavailableError,
  fetchInvocations,
  fetchModels,
  type InvocationRow,
} from "@/lib/api";
import { cn } from "@/lib/utils";

const PAGE_SIZE = 50;
type StatusFilter = "all" | "error" | "success";

const num = (v: number | string | undefined): number => {
  const n = Number(v ?? 0);
  return Number.isFinite(n) ? n : 0;
};

function statusTone(status: number): [string, string] {
  if (status === 0 || status >= 500) return ["var(--status-danger)", "rgba(229,57,53,.14)"];
  if (status === 429) return ["var(--status-warning)", "rgba(245,158,11,.14)"];
  if (status >= 400) return ["var(--status-warning)", "rgba(245,158,11,.14)"];
  return ["var(--status-success)", "rgba(22,163,74,.14)"];
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
  const [filtersOpen, setFiltersOpen] = React.useState(false);
  const [status, setStatus] = React.useState<StatusFilter>("all");
  const [modelSel, setModelSel] = React.useState<string[]>([]);
  const [page, setPage] = React.useState(0);
  const [selected, setSelected] = React.useState<InvocationRow | null>(null);
  const [streaming, setStreaming] = React.useState(true);

  const window = React.useMemo(
    () => ({ since: new Date(Date.now() - 24 * 3600_000).toISOString() }),
    [],
  );

  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels });

  React.useEffect(() => setPage(0), [status, modelSel]);

  const query = useQuery({
    queryKey: ["invocations", window.since, status, modelSel[0] ?? "", page],
    queryFn: () =>
      fetchInvocations({
        since: window.since,
        model: modelSel[0] || undefined,
        status,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      }),
    retry: (n, error) => !isUnavailable(error) && n < 2,
    placeholderData: (prev) => prev,
    refetchInterval: streaming ? 5000 : false,
  });

  const rows = query.data ?? [];
  const hasMore = rows.length === PAGE_SIZE;
  const filterCount = (status === "all" ? 0 : 1) + modelSel.length;

  if (isUnavailable(query.error)) {
    return (
      <div className="p-[22px]">
        <p className="rounded-lg border border-[color:var(--border-subtle)] p-6 text-sm text-muted-foreground">
          Analytics isn&apos;t configured for this deployment — set{" "}
          <code className="font-mono text-xs">clickhouse_url</code> on the control plane to
          enable per-invocation logs.
        </p>
      </div>
    );
  }

  const statusSelected = status === "all" ? [] : [status];

  return (
    <div className="flex h-full min-h-0">
      {filtersOpen && (
        <div className="w-[248px] flex-none overflow-y-auto border-r border-[color:var(--border-subtle)]">
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
          </FilterPanel>
        </div>
      )}

      <div className="flex min-w-0 flex-1 flex-col">
        <div className="flex flex-none items-center gap-2.5 border-b border-[color:var(--border-subtle)] px-[18px] py-3">
          <button
            type="button"
            onClick={() => setFiltersOpen((v) => !v)}
            className={cn(
              "inline-flex h-8 items-center gap-[7px] rounded-md border border-[color:var(--border-subtle)] px-3 text-sm text-muted-foreground transition-colors hover:text-foreground",
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
                disabled={page === 0}
                onClick={() => setPage((p) => Math.max(0, p - 1))}
                className="flex rounded-md border border-[color:var(--border-subtle)] p-[5px] text-[color:var(--text-subtle)] transition-colors enabled:hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
              >
                <ChevronLeft className="h-3.5 w-3.5" />
              </button>
              <span className="font-mono text-xs text-muted-foreground">p{page + 1}</span>
              <button
                type="button"
                title="Next page"
                disabled={!hasMore}
                onClick={() => setPage((p) => p + 1)}
                className="flex rounded-md border border-[color:var(--border-subtle)] p-[5px] text-[color:var(--text-subtle)] transition-colors enabled:hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
              >
                <ChevronRight className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <table className="w-full table-fixed border-collapse text-sm">
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
                <th className={TH}>Time</th>
                <th className={TH}>Model</th>
                <th className={TH}>Provider</th>
                <th className={TH}>Status</th>
                <th className={cn(TH, "text-right")}>Latency</th>
                <th className={cn(TH, "text-right")}>Tokens</th>
                <th className={cn(TH, "text-right")}>Cost</th>
                <th className={TH} />
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
                      {r.ts?.replace("T", " ").slice(0, 23)}
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
                      {Math.round(num(r.latency_ms))} ms
                    </td>
                    <td className={cn(TD, "text-right text-[color:var(--text-secondary)]")}>
                      {num(r.total_tokens).toLocaleString()}
                    </td>
                    <td className={cn(TD, "text-right text-[color:var(--text-secondary)]")}>
                      ${num(r.cost_usd).toFixed(4)}
                    </td>
                    <td className={cn(TD, "pr-2.5 text-right")}>
                      <ChevronRight className="ml-auto h-[15px] w-[15px] text-[color:var(--text-subtle)]" />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {!query.isLoading && rows.length === 0 && (
            <p className="px-4 py-10 text-center text-sm text-muted-foreground">
              No requests in this window.
            </p>
          )}
        </div>
      </div>

      {selected && (
        <div className="w-[380px] flex-none overflow-y-auto border-l border-[color:var(--border-subtle)] bg-background">
          <div className="flex items-center gap-2.5 border-b border-[color:var(--border-subtle)] px-[18px] py-3.5">
            <span className="truncate font-mono text-sm">{selected.request_id || "request"}</span>
            <button
              type="button"
              aria-label="Close details"
              onClick={() => setSelected(null)}
              className="ml-auto flex text-[color:var(--text-subtle)] transition-colors hover:text-foreground"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <div className="flex flex-col gap-3.5 p-[18px]">
            <div className="grid grid-cols-2 gap-3">
              <DrawerStat label="Model" value={selected.model} />
              <DrawerStat label="Provider" value={selected.provider || "—"} />
              <DrawerStat label="Latency" value={`${Math.round(num(selected.latency_ms))} ms`} />
              <DrawerStat label="Cost" value={`$${num(selected.cost_usd).toFixed(4)}`} />
              <DrawerStat
                label="Tokens"
                value={`${num(selected.prompt_tokens)} in · ${num(selected.completion_tokens)} out`}
              />
              <DrawerStat label="Virtual key" value={selected.virtual_key_id || "—"} />
            </div>
            {selected.error && (
              <DrawerBlock label="Error" content={selected.error} />
            )}
            <DrawerBlock
              label="Request"
              content={pretty(selected.request_payload) ?? "payload logging is off"}
            />
            <DrawerBlock
              label="Response"
              content={pretty(selected.response_payload) ?? "payload logging is off"}
            />
          </div>
        </div>
      )}
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

function DrawerStat({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <div className="mb-[3px] text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <div className="truncate font-mono text-sm">{value}</div>
    </div>
  );
}

function DrawerBlock({ label, content }: { label: string; content: string }) {
  return (
    <div>
      <div className="mb-1.5 text-[0.6875rem] uppercase tracking-[0.06em] text-[color:var(--text-subtle)]">
        {label}
      </div>
      <pre className="overflow-x-auto whitespace-pre-wrap rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-3 font-mono text-xs text-[color:var(--text-secondary)]">
        {content}
      </pre>
    </div>
  );
}
