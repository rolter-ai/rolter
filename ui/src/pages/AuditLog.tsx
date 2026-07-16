import { useQuery } from "@tanstack/react-query";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Table, type TableColumn } from "@/components/ui/table";
import { fetchAuditLog, type AuditLogEntry } from "@/lib/api";
import { useScope } from "@/lib/scope";

const AUDIT_LOG_QUERY_KEY = ["audit-log"];

export default function AuditLog() {
  const scope = useScope();
  const [expanded, setExpanded] = React.useState<string | null>(null);

  const entries = useQuery({
    queryKey: [...AUDIT_LOG_QUERY_KEY, scope.orgId],
    queryFn: () => fetchAuditLog(scope.orgId as string),
    enabled: !!scope.orgId,
  });

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
      render: (v, row) =>
        v ? (
          <span className="font-mono text-xs text-muted-foreground">
            {v as string}
            {row.target_id ? `/${String(row.target_id).slice(0, 8)}` : ""}
          </span>
        ) : (
          "—"
        ),
    },
    {
      key: "detail",
      header: "Detail",
      render: (v, row) =>
        v ? (
          <button
            type="button"
            className="text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
            onClick={() =>
              setExpanded(expanded === row.id ? null : row.id)
            }
          >
            {expanded === row.id ? "hide" : "show"}
          </button>
        ) : (
          "—"
        ),
    },
  ];

  return (
    <div className="space-y-4">
      <div>
        <h1 className="text-2xl font-semibold">Audit Log</h1>
        <p className="text-sm text-muted-foreground">
          Admin, CRUD, and auth actions recorded for this org.
        </p>
      </div>

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
          <Table
            columns={columns as unknown as TableColumn<Record<string, unknown>>[]}
            data={(entries.data ?? []) as unknown as Record<string, unknown>[]}
            rowKey="id"
          />
          {expanded && (
            <pre className="max-h-64 overflow-auto rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3 text-xs">
              {JSON.stringify(
                entries.data?.find((e) => e.id === expanded)?.detail,
                null,
                2,
              )}
            </pre>
          )}
        </>
      )}
    </div>
  );
}
