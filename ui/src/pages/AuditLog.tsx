import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { Link } from "react-router-dom";

import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Table, type TableColumn } from "@/components/ui/table";
import { fetchAuditLog, type AuditLogEntry } from "@/lib/api";
import { useScope } from "@/lib/scope";

const AUDIT_LOG_QUERY_KEY = ["audit-log"];

// pull a wide window so client-side filtering has something to work with;
// the backend clamps this to 500 and does no filtering of its own
const FETCH_LIMIT = 500;

const PAGE_SIZE = 25;

const RANGES = [
  { label: "Last 24h", hours: 24 },
  { label: "Last 7d", hours: 24 * 7 },
  { label: "Last 30d", hours: 24 * 30 },
  { label: "All", hours: null },
] as const;

// dashboard route that owns each audited resource type, for the link-out
// column; scope-level types (org/team/project) have no dedicated page
const TARGET_PATH: Record<string, string> = {
  provider: "/providers",
  route: "/config",
  route_target: "/config",
  virtual_key: "/keys",
  user: "/users",
  membership: "/users",
  rate_limit: "/limits",
  budget: "/limits",
  model_price: "/pricing",
};

export default function AuditLog() {
  const scope = useScope();
  const [expanded, setExpanded] = React.useState<string | null>(null);

  const [actor, setActor] = React.useState("");
  const [action, setAction] = React.useState("");
  const [target, setTarget] = React.useState("");
  const [rangeIdx, setRangeIdx] = React.useState(3);
  const [page, setPage] = React.useState(0);

  const entries = useQuery({
    queryKey: [...AUDIT_LOG_QUERY_KEY, scope.orgId],
    queryFn: () => fetchAuditLog(scope.orgId as string, FETCH_LIMIT),
    enabled: !!scope.orgId,
  });

  const rows = React.useMemo(() => entries.data ?? [], [entries.data]);

  // distinct actions/targets present in the data, for the filter dropdowns
  const actions = React.useMemo(
    () => Array.from(new Set(rows.map((r) => r.action))).sort(),
    [rows],
  );
  const targets = React.useMemo(
    () =>
      Array.from(
        new Set(rows.map((r) => r.target_type).filter((t): t is string => !!t)),
      ).sort(),
    [rows],
  );

  const filtered = React.useMemo(() => {
    const actorQ = actor.trim().toLowerCase();
    const since =
      RANGES[rangeIdx].hours != null
        ? Date.now() - RANGES[rangeIdx].hours * 60 * 60 * 1000
        : null;
    return rows.filter((r) => {
      if (action && r.action !== action) return false;
      if (target && r.target_type !== target) return false;
      if (
        actorQ &&
        !(r.actor_user_id ?? "system").toLowerCase().includes(actorQ)
      )
        return false;
      if (since != null && new Date(r.at).getTime() < since) return false;
      return true;
    });
  }, [rows, actor, action, target, rangeIdx]);

  // reset to the first page whenever the filter set changes
  React.useEffect(() => {
    setPage(0);
  }, [actor, action, target, rangeIdx]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const clampedPage = Math.min(page, pageCount - 1);
  const paged = filtered.slice(
    clampedPage * PAGE_SIZE,
    clampedPage * PAGE_SIZE + PAGE_SIZE,
  );

  const columns: TableColumn<AuditLogEntry>[] = [
    {
      key: "at",
      header: "Time",
      mono: true,
      render: (v) => new Date(v as string).toLocaleString(),
    },
    {
      key: "actor_user_id",
      header: "Actor",
      mono: true,
      render: (v) => (v ? String(v).slice(0, 8) : "system"),
    },
    {
      key: "action",
      header: "Action",
      render: (v) => <Badge tone="outline">{v as string}</Badge>,
    },
    {
      key: "target_type",
      header: "Target",
      render: (v, row) => {
        if (!v) return "—";
        const path = TARGET_PATH[v as string];
        const label = (
          <span className="font-mono text-xs">
            {v as string}
            {row.target_id ? `/${String(row.target_id).slice(0, 8)}` : ""}
          </span>
        );
        return path ? (
          <Link
            to={path}
            className="text-muted-foreground underline underline-offset-2 hover:text-foreground"
          >
            {label}
          </Link>
        ) : (
          <span className="text-muted-foreground">{label}</span>
        );
      },
    },
    {
      key: "detail",
      header: "Detail",
      render: (v, row) =>
        v ? (
          <button
            type="button"
            className="text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
            onClick={() => setExpanded(expanded === row.id ? null : row.id)}
          >
            {expanded === row.id ? "hide" : "show"}
          </button>
        ) : (
          "—"
        ),
    },
  ];

  return (
    <PageBody>

      {entries.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {entries.error && (
        <p className="text-sm text-destructive">Failed to load audit log.</p>
      )}
      {!scope.isLoading && !scope.error && !scope.orgId && (
        <p className="text-sm text-muted-foreground">
          No org configured yet — pick or create one to view its audit log.
        </p>
      )}

      {scope.orgId && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              className="w-40"
              placeholder="Filter actor…"
              value={actor}
              onChange={(e) => setActor(e.target.value)}
            />
            <Select
              className="w-44"
              value={action}
              onChange={(e) => setAction(e.target.value)}
            >
              <option value="">All actions</option>
              {actions.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            </Select>
            <Select
              className="w-40"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
            >
              <option value="">All targets</option>
              {targets.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
            <div className="flex gap-1">
              {RANGES.map((r, i) => (
                <button
                  key={r.label}
                  type="button"
                  onClick={() => setRangeIdx(i)}
                  className={`rounded-md border px-2.5 py-1 text-xs ${
                    i === rangeIdx
                      ? "border-brand-folk bg-accent text-foreground"
                      : "border-border text-muted-foreground hover:text-foreground"
                  }`}
                >
                  {r.label}
                </button>
              ))}
            </div>
          </div>

          <Table
            columns={columns as unknown as TableColumn<Record<string, unknown>>[]}
            data={paged as unknown as Record<string, unknown>[]}
            rowKey="id"
          />

          {filtered.length === 0 && !entries.isLoading && (
            <p className="text-sm text-muted-foreground">
              No entries match these filters.
            </p>
          )}

          {filtered.length > 0 && (
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span>
                {clampedPage * PAGE_SIZE + 1}–
                {Math.min((clampedPage + 1) * PAGE_SIZE, filtered.length)} of{" "}
                {filtered.length}
                {rows.length >= FETCH_LIMIT && " (latest 500)"}
              </span>
              <div className="flex gap-1">
                <button
                  type="button"
                  disabled={clampedPage === 0}
                  onClick={() => setPage(clampedPage - 1)}
                  className="rounded-md border border-border px-2.5 py-1 hover:text-foreground disabled:opacity-40"
                >
                  Prev
                </button>
                <button
                  type="button"
                  disabled={clampedPage >= pageCount - 1}
                  onClick={() => setPage(clampedPage + 1)}
                  className="rounded-md border border-border px-2.5 py-1 hover:text-foreground disabled:opacity-40"
                >
                  Next
                </button>
              </div>
            </div>
          )}

          {expanded && (
            <pre className="max-h-64 overflow-auto rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3 text-xs">
              {JSON.stringify(
                filtered.find((e) => e.id === expanded)?.detail,
                null,
                2,
              )}
            </pre>
          )}
        </>
      )}
    </PageBody>
  );
}
