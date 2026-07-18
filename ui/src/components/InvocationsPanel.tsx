import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  FilterCheckList,
  FilterPanel,
  FilterSection,
} from "@/components/ui/filter-panel";
import { Input } from "@/components/ui/input";
import {
  AnalyticsUnavailableError,
  fetchInvocations,
  type AnalyticsWindow,
  type InvocationRow,
} from "@/lib/api";

const PAGE_SIZE = 50;

type StatusFilter = "all" | "error" | "success";

function num(value: number | string | undefined): number {
  if (value === undefined) return 0;
  const n = Number(value);
  return Number.isFinite(n) ? n : 0;
}

function fmtTs(ts: string): string {
  const d = new Date(ts.includes("Z") || ts.includes("+") ? ts : `${ts}Z`);
  return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
}

/// status pill tone: 2xx/3xx success, 4xx warning, 5xx / 0 (no response) danger
function statusTone(status: number): "success" | "warning" | "danger" {
  if (status === 0 || status >= 500) return "danger";
  if (status >= 400) return "warning";
  return "success";
}

function isUnavailable(error: unknown): boolean {
  return error instanceof AnalyticsUnavailableError;
}

export function InvocationsPanel({ window }: { window: AnalyticsWindow }) {
  const [model, setModel] = React.useState("");
  const [key, setKey] = React.useState("");
  const [status, setStatus] = React.useState<StatusFilter>("all");
  const [page, setPage] = React.useState(0);
  const [selected, setSelected] = React.useState<InvocationRow | null>(null);

  // debounce the free-text filters so we don't refetch on every keystroke
  const [debounced, setDebounced] = React.useState({ model: "", key: "" });
  React.useEffect(() => {
    const t = setTimeout(() => setDebounced({ model, key }), 300);
    return () => clearTimeout(t);
  }, [model, key]);

  // reset to the first page whenever a filter or the time window changes
  React.useEffect(() => {
    setPage(0);
  }, [debounced.model, debounced.key, status, window.since, window.until]);

  const query = useQuery({
    queryKey: [
      "invocations",
      window.since,
      window.until,
      debounced.model,
      debounced.key,
      status,
      page,
    ],
    queryFn: () =>
      fetchInvocations({
        since: window.since,
        until: window.until,
        model: debounced.model || undefined,
        key: debounced.key || undefined,
        status,
        limit: PAGE_SIZE,
        offset: page * PAGE_SIZE,
      }),
    retry: (failureCount, error) => !isUnavailable(error) && failureCount < 2,
    placeholderData: (prev) => prev,
  });

  if (isUnavailable(query.error)) {
    return (
      <Card>
        <CardContent className="py-6 text-sm text-muted-foreground">
          Analytics isn&apos;t configured for this deployment — set{" "}
          <code className="font-mono text-xs">clickhouse_url</code> on the control
          plane to enable per-invocation logs.
        </CardContent>
      </Card>
    );
  }

  const rows = query.data ?? [];
  const hasMore = rows.length === PAGE_SIZE;

  // checklist ↔ single status param: none or both checked means "all"
  const statusSelected =
    status === "all" ? [] : status === "success" ? ["success"] : ["error"];
  const onStatusChange = (sel: string[]) =>
    setStatus(sel.length === 1 ? (sel[0] as StatusFilter) : "all");

  return (
    <div className="flex items-start gap-4">
      <FilterPanel className="h-auto w-[220px] flex-none rounded-lg border border-[color:var(--border-subtle)]">
        <FilterSection title="Status" defaultOpen count={statusSelected.length}>
          <FilterCheckList
            options={[
              { value: "success", label: "Success (2xx/3xx)" },
              { value: "error", label: "Errors (4xx/5xx)" },
            ]}
            selected={statusSelected}
            onChange={onStatusChange}
          />
        </FilterSection>
        <FilterSection title="Model" defaultOpen count={debounced.model ? 1 : 0}>
          <Input
            className="h-8 text-xs"
            placeholder="Filter by model"
            value={model}
            onChange={(e) => setModel(e.target.value)}
          />
        </FilterSection>
        <FilterSection title="Virtual key" count={debounced.key ? 1 : 0}>
          <Input
            className="h-8 text-xs"
            placeholder="Filter by virtual key id"
            value={key}
            onChange={(e) => setKey(e.target.value)}
          />
        </FilterSection>
      </FilterPanel>

      <div className="min-w-0 flex-1 space-y-3">
      {query.isError && !isUnavailable(query.error) && (
        <p className="text-sm text-destructive">
          {(query.error as Error).message}
        </p>
      )}

      <Card>
        <CardContent className="p-0">
          <div className="overflow-x-auto">
            <table className="w-full text-left text-sm">
              <thead>
                <tr className="border-b border-border text-xs text-muted-foreground">
                  <th className="px-3 py-2 font-medium">Time</th>
                  <th className="px-3 py-2 font-medium">Model</th>
                  <th className="px-3 py-2 font-medium">Provider</th>
                  <th className="px-3 py-2 font-medium">Status</th>
                  <th className="px-3 py-2 font-medium">Tokens</th>
                  <th className="px-3 py-2 font-medium">Latency</th>
                  <th className="px-3 py-2 font-medium">Cost</th>
                </tr>
              </thead>
              <tbody>
                {query.isLoading && (
                  <tr>
                    <td colSpan={7} className="px-3 py-6 text-muted-foreground">
                      Loading…
                    </td>
                  </tr>
                )}
                {!query.isLoading && rows.length === 0 && (
                  <tr>
                    <td colSpan={7} className="px-3 py-6 text-muted-foreground">
                      No invocations match these filters in this window.
                    </td>
                  </tr>
                )}
                {rows.map((row) => {
                  const st = num(row.status);
                  return (
                    <tr
                      key={`${row.request_id}-${row.ts}`}
                      className="cursor-pointer border-b border-border/50 hover:bg-accent/50"
                      onClick={() => setSelected(row)}
                    >
                      <td className="whitespace-nowrap px-3 py-2 font-mono text-xs">
                        {fmtTs(row.ts)}
                      </td>
                      <td className="px-3 py-2 font-mono text-xs">{row.model}</td>
                      <td className="px-3 py-2 text-xs text-muted-foreground">
                        {row.provider || "—"}
                      </td>
                      <td className="px-3 py-2">
                        <Badge tone={statusTone(st)} dot>
                          {st === 0 ? "no response" : st}
                        </Badge>
                      </td>
                      <td className="px-3 py-2 font-mono text-xs">
                        {num(row.total_tokens).toLocaleString()}
                      </td>
                      <td className="px-3 py-2 font-mono text-xs">
                        {num(row.latency_ms).toFixed(0)} ms
                      </td>
                      <td className="px-3 py-2 font-mono text-xs">
                        ${num(row.cost_usd).toFixed(4)}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </CardContent>
      </Card>

      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span>
          Page {page + 1}
          {query.isFetching && !query.isLoading ? " · refreshing…" : ""}
        </span>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={page === 0}
            onClick={() => setPage((p) => Math.max(0, p - 1))}
          >
            Previous
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={!hasMore}
            onClick={() => setPage((p) => p + 1)}
          >
            Next
          </Button>
        </div>
      </div>

      <InvocationDetail row={selected} onClose={() => setSelected(null)} />
      </div>
    </div>
  );
}

function DetailRow({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 border-b border-border/50 py-1.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="break-all text-right font-mono text-xs">{value}</span>
    </div>
  );
}

function InvocationDetail({
  row,
  onClose,
}: {
  row: InvocationRow | null;
  onClose: () => void;
}) {
  if (!row) return null;
  const st = num(row.status);
  return (
    <Dialog open={row !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogHeader>
        <DialogTitle>Invocation</DialogTitle>
        <DialogDescription className="font-mono text-xs">
          {row.request_id}
        </DialogDescription>
      </DialogHeader>
      <div className="max-h-[60vh] space-y-0.5 overflow-y-auto">
        <DetailRow label="Timestamp" value={fmtTs(row.ts)} />
        <DetailRow
          label="Status"
          value={
            <Badge tone={statusTone(st)} dot>
              {st === 0 ? "no response" : st}
            </Badge>
          }
        />
        <DetailRow label="Model" value={row.model} />
        <DetailRow label="Provider" value={row.provider || "—"} />
        <DetailRow label="Target" value={row.target || "—"} />
        {row.variant && <DetailRow label="Variant" value={row.variant} />}
        <DetailRow label="Stream" value={num(row.stream) ? "yes" : "no"} />
        <DetailRow label="Cache hit" value={num(row.cache_hit) ? "yes" : "no"} />
        <DetailRow
          label="Provider prompt cache (read / write)"
          value={`${num(row.cache_read_tokens)} / ${num(row.cache_write_tokens)}`}
        />
        <DetailRow
          label="Tokens (prompt / completion / total)"
          value={`${num(row.prompt_tokens)} / ${num(row.completion_tokens)} / ${num(
            row.total_tokens,
          )}`}
        />
        <DetailRow label="Cost" value={`$${num(row.cost_usd).toFixed(6)}`} />
        <DetailRow label="Latency" value={`${num(row.latency_ms).toFixed(0)} ms`} />
        <DetailRow label="TTFT" value={`${num(row.ttft_ms).toFixed(0)} ms`} />
        <DetailRow label="Virtual key" value={row.virtual_key_id || "—"} />
        <DetailRow label="Project" value={row.project_id || "—"} />
        <DetailRow label="Team" value={row.team_id || "—"} />
        <DetailRow label="Org" value={row.org_id || "—"} />
        <DetailRow label="Trace id" value={row.trace_id || "—"} />
      </div>
      {row.error && (
        <div className="mt-3">
          <p className="mb-1 text-xs font-medium text-destructive">Error</p>
          <pre className="max-h-40 overflow-auto rounded border border-destructive/30 bg-destructive/5 p-2 font-mono text-xs text-destructive">
            {row.error}
          </pre>
        </div>
      )}
      {row.request_payload || row.response_payload ? (
        <div className="mt-4 space-y-3">
          {row.request_payload && <Payload label="Request payload" value={row.request_payload} />}
          {row.response_payload && <Payload label="Response payload" value={row.response_payload} />}
        </div>
      ) : (
        <p className="mt-4 text-[0.625rem] text-muted-foreground">
          Raw request/response bodies weren&apos;t captured for this invocation.
        </p>
      )}
    </Dialog>
  );
}

function Payload({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <p className="mb-1 text-xs font-medium">{label}</p>
      <pre className="max-h-60 overflow-auto rounded border bg-muted/30 p-2 font-mono text-xs whitespace-pre-wrap">
        {value}
      </pre>
    </div>
  );
}
