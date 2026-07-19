import { useQuery } from "@tanstack/react-query";
import * as React from "react";
import { Link } from "react-router-dom";

import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Table, type TableColumn } from "@/components/ui/table";
import { fetchAuditLogPage, type AuditLogEntry } from "@/lib/api";
import { useScope } from "@/lib/scope";

const PAGE_SIZE = 25;

const RANGES = [
  { label: "Last 24h", hours: 24 },
  { label: "Last 7d", hours: 24 * 7 },
  { label: "Last 30d", hours: 24 * 30 },
  { label: "All", hours: null },
] as const;

// well-known audited actions for the filter dropdown; the API filters
// server-side so the list doesn't depend on the current page
const ACTIONS = [
  "provider.create",
  "provider.update",
  "provider.delete",
  "route.create",
  "route.delete",
  "route.set_params",
  "route.set_complexity",
  "route.set_advanced",
  "virtual_key.create",
  "virtual_key.delete",
  "user.invite",
  "user.update",
  "user.delete",
  "membership.create",
  "membership.delete",
  "budget.create",
  "budget.delete",
  "security.settings.update",
] as const;

const TARGET_TYPES = [
  "provider",
  "route",
  "route_target",
  "virtual_key",
  "user",
  "membership",
  "rate_limit",
  "budget",
  "model_price",
  "security_settings",
] as const;

// dashboard route that owns each audited resource type, for the link-out
// column; scope-level types (org/team/project) have no dedicated page
const TARGET_PATH: Record<string, string> = {
  provider: "/providers",
  route: "/routing-rules",
  route_target: "/routing-rules",
  virtual_key: "/virtual-keys",
  user: "/gov-users",
  membership: "/gov-users",
  rate_limit: "/budgets",
  budget: "/budgets",
  model_price: "/pricing-overrides",
  security_settings: "/security",
};

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// server-side paginated, filtered audit log: action/target/actor/time-range
// filters map to query params, pagination walks the keyset cursor
export default function AuditLog() {
  const scope = useScope();
  const [expanded, setExpanded] = React.useState<string | null>(null);

  const [actor, setActor] = React.useState("");
  const [action, setAction] = React.useState("");
  const [target, setTarget] = React.useState("");
  const [rangeIdx, setRangeIdx] = React.useState(3);
  const [cursors, setCursors] = React.useState<string[]>([]);
  const cursor = cursors[cursors.length - 1];

  const from = React.useMemo(() => {
    const hours = RANGES[rangeIdx].hours;
    return hours != null
      ? new Date(Date.now() - hours * 3_600_000).toISOString()
      : undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeIdx, cursor]);

  const actorParam = UUID_RE.test(actor.trim()) ? actor.trim() : undefined;

  const page = useQuery({
    queryKey: ["audit-log", scope.orgId, action, target, actorParam, rangeIdx, cursor],
    queryFn: () =>
      fetchAuditLogPage(scope.orgId as string, {
        limit: PAGE_SIZE,
        cursor,
        action: action || undefined,
        target_type: target || undefined,
        actor: actorParam,
        from,
        include_total: !cursor,
      }),
    enabled: !!scope.orgId,
  });

  // reset to the first page whenever the filter set changes
  React.useEffect(() => {
    setCursors([]);
  }, [action, target, actorParam, rangeIdx]);

  const rows = page.data?.items ?? [];
  const [total, setTotal] = React.useState<number | null>(null);
  React.useEffect(() => {
    if (page.data?.total !== undefined) setTotal(page.data.total);
  }, [page.data]);

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
      {page.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {page.error && (
        <p className="text-sm text-destructive">
          Failed to load audit log: {(page.error as Error).message}
        </p>
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
              className="w-[280px] font-mono text-xs"
              placeholder="Actor user id (full UUID)…"
              value={actor}
              onChange={(e) => setActor(e.target.value)}
            />
            <Select
              className="w-52"
              value={action}
              onChange={(e) => setAction(e.target.value)}
            >
              <option value="">All actions</option>
              {ACTIONS.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            </Select>
            <Select
              className="w-44"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
            >
              <option value="">All targets</option>
              {TARGET_TYPES.map((t) => (
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
            data={rows as unknown as Record<string, unknown>[]}
            rowKey="id"
          />

          {rows.length === 0 && page.isSuccess && (
            <p className="text-sm text-muted-foreground">
              No entries match these filters.
            </p>
          )}

          {(rows.length > 0 || cursors.length > 0) && (
            <div className="flex items-center justify-between text-xs text-muted-foreground">
              <span>
                page {cursors.length + 1}
                {total != null && ` · ${total} total`}
              </span>
              <div className="flex gap-1">
                <button
                  type="button"
                  disabled={cursors.length === 0}
                  onClick={() => setCursors((c) => c.slice(0, -1))}
                  className="rounded-md border border-border px-2.5 py-1 hover:text-foreground disabled:opacity-40"
                >
                  Prev
                </button>
                <button
                  type="button"
                  disabled={!page.data?.has_next || !page.data.next_cursor}
                  onClick={() =>
                    page.data?.next_cursor &&
                    setCursors((c) => [...c, page.data.next_cursor as string])
                  }
                  className="rounded-md border border-border px-2.5 py-1 hover:text-foreground disabled:opacity-40"
                >
                  Next
                </button>
              </div>
            </div>
          )}

          {expanded && (
            <pre className="max-h-64 overflow-auto rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3 text-xs">
              {JSON.stringify(rows.find((e) => e.id === expanded)?.detail, null, 2)}
            </pre>
          )}
        </>
      )}
    </PageBody>
  );
}
