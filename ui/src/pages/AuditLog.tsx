import { useQuery } from "@tanstack/react-query";
import { Building2, ScrollText } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Table, type TableColumn } from "@/components/ui/table";
import { fetchAuditLogPage, fetchUsers, type AuditLogEntry } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PAGE_SIZE = 25;

// "All", the widest window — anything narrower is a filter the operator chose,
// so an empty page under it means "nothing matched" rather than "nothing yet"
const DEFAULT_RANGE = 3;

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
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  const [expanded, setExpanded] = React.useState<string | null>(null);

  const [actor, setActor] = React.useState("");
  const [action, setAction] = React.useState("");
  const [target, setTarget] = React.useState("");
  const [rangeIdx, setRangeIdx] = React.useState(DEFAULT_RANGE);
  const [cursors, setCursors] = React.useState<string[]>([]);
  const cursor = cursors[cursors.length - 1];

  const from = React.useMemo(() => {
    const hours = RANGES[rangeIdx].hours;
    return hours != null
      ? new Date(Date.now() - hours * 3_600_000).toISOString()
      : undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeIdx, cursor]);

  // the org's accounts, so the actor column can say who rather than the first
  // eight hex digits of a uuid, and the actor filter can be picked by e-mail
  const users = useQuery({
    queryKey: ["users", scope.orgId],
    queryFn: () => fetchUsers(scope.orgId as string),
    enabled: !!scope.orgId,
  });
  const emailOf = (id: string) => users.data?.find((u) => u.id === id)?.email;
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


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `page` is the query the user is actually waiting on for this screen

  useScreenReady(!page.isLoading);

  useErrorState(!!page.error, "audit-log");

  // reset to the first page whenever the filter set changes
  React.useEffect(() => {
    setCursors([]);
  }, [action, target, actorParam, rangeIdx]);

  const rows = page.data?.items ?? [];
  const [total, setTotal] = React.useState<number | null>(null);
  const filtersActive = !!actor || !!action || !!target || rangeIdx !== DEFAULT_RANGE;
  const clearFilters = () => {
    setActor("");
    setAction("");
    setTarget("");
    setRangeIdx(DEFAULT_RANGE);
  };
  React.useEffect(() => {
    if (page.data?.total !== undefined) setTotal(page.data.total);
  }, [page.data]);

  const columns: TableColumn<AuditLogEntry>[] = [
    {
      key: "at",
      header: "Time",
      mono: true,
      render: (v) => fmt.dateTime(v as string),
    },
    {
      key: "actor_user_id",
      header: "Actor",
      mono: true,
      render: (v) =>
        v ? (
          <span title={String(v)}>{emailOf(String(v)) ?? String(v).slice(0, 8)}</span>
        ) : (
          "system"
        ),
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
            className="rounded-sm text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
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
      {page.error && (
        <LoadError
          error={page.error}
          resource={t("errors.resources.auditLog")}
          onRetry={() => page.refetch()}
        />
      )}
      {!scope.isLoading && !scope.errorKey && !scope.orgId && (
        <EmptyState
          uxTarget="audit-log-no-org"
          icon={<Building2 />}
          title={t("pages.auditLog.noOrgTitle")}
          description={t("pages.auditLog.noOrgBody")}
        />
      )}

      {scope.orgId && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            {users.data && users.data.length > 0 ? (
              <Select
                className="w-[280px]"
                aria-label={t("pages.auditLog.actorFilterAria")}
                value={actor}
                onChange={(e) => setActor(e.target.value)}
              >
                <option value="">{t("pages.auditLog.anyActor")}</option>
                {users.data.map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.email}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                className="w-[280px] font-mono text-xs"
                aria-label={t("pages.auditLog.actorFilterAria")}
                placeholder={t("pages.auditLog.actorPlaceholder")}
                value={actor}
                onChange={(e) => setActor(e.target.value)}
              />
            )}
            <Select
              className="w-52"
              aria-label={t("pages.auditLog.actionFilterAria")}
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
              aria-label={t("pages.auditLog.targetFilterAria")}
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
                  className={`rounded-md border px-2.5 py-1 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring ${
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

          {/* the placeholder lives in the table so the columns stay on screen
              and say what a matching entry would have looked like (#1180) */}
          {page.isLoading ? (
            <TableSkeleton rows={6} />
          ) : (
            <Table
              columns={columns as unknown as TableColumn<Record<string, unknown>>[]}
              data={rows as unknown as Record<string, unknown>[]}
              rowKey="id"
              empty={
                <EmptyState
                  uxTarget="audit-log"
                  icon={<ScrollText />}
                  title={
                    filtersActive
                      ? t("pages.auditLog.noMatchTitle")
                      : t("pages.auditLog.emptyTitle")
                  }
                  description={
                    filtersActive
                      ? t("pages.auditLog.noMatchBody")
                      : t("pages.auditLog.emptyBody")
                  }
                  actions={
                    filtersActive ? (
                      <Button variant="outline" onClick={clearFilters}>
                        {t("common.clearSearch")}
                      </Button>
                    ) : undefined
                  }
                />
              }
            />
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
                  className="rounded-md border border-border px-2.5 py-1 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-40"
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
                  className="rounded-md border border-border px-2.5 py-1 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-40"
                >
                  Next
                </button>
              </div>
            </div>
          )}

          {expanded && (
            /* the entry's recorded detail, through the shared code block so a
               changed field is picked out rather than buried in monospace
               (#949) */
            <CodeBlock
              value={JSON.stringify(rows.find((e) => e.id === expanded)?.detail, null, 2)}
              language="json"
              label={t("pages.auditLog.detail")}
              maxHeight={256}
            />
          )}
        </>
      )}
    </PageBody>
  );
}
