import { useQuery } from "@tanstack/react-query";
import { Building2, ScrollText } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { ExportCsvButton } from "@/components/ExportCsvButton";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { Combobox } from "@/components/ui/combobox";
import { CopyableText } from "@/components/ui/copyable-value";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Table, type TableColumn } from "@/components/ui/table";
import {
  auditLogExport,
  fetchAuditLogPage,
  fetchDeploymentAuditLogPage,
  fetchDeploymentUsers,
  fetchUsers,
  type AuditLogEntry,
} from "@/lib/api";
import {
  auditCsv,
  auditExportName,
  auditExportRow,
  auditJson,
  collectAuditLog,
  downloadText,
  type AuditExportFormat,
} from "@/lib/audit-export";
import { AUDIT_TARGET_TYPES, auditGroup, groupedActions } from "@/lib/audit-vocabulary";
import { useOptionalAuth } from "@/lib/auth";
import { useCan } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { useScreenReady } from "@/lib/ux-react";

const PAGE_SIZE = 25;

// "All", the widest window — anything narrower is a filter the operator chose,
// so an empty page under it means "nothing matched" rather than "nothing yet"
const DEFAULT_RANGE = 3;

const RANGES = [
  { id: "last24h", hours: 24 },
  { id: "last7d", hours: 24 * 7 },
  { id: "last30d", hours: 24 * 30 },
  { id: "all", hours: null },
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
  // identity: the screens that own each target, so an SSO or SCIM row links out
  sso_provider: "/sso",
  sso_group_mapping: "/sso",
  ldap_directory: "/ldap",
  ldap_group_mapping: "/ldap",
  api_token: "/api-tokens",
  scim_token: "/user-provisioning",
  scim_group: "/user-provisioning",
  scim_group_mapping: "/user-provisioning",
  custom_role: "/rbac",
  invitation: "/gov-users",
  team: "/gov-teams",
  access_profile: "/access-profiles",
  access_profile_assignment: "/access-profiles",
  provider_group: "/provider-groups",
  guardrail_rule: "/guardrail-rules",
  guardrail_provider: "/guardrail-providers",
  guardrail_policy: "/guardrail-policies",
  mcp_server: "/mcp-catalog",
  plugin: "/plugins",
  prompt_template: "/prompt-repo",
  skill: "/skills-repo",
  feature_flags: "/feature-flags",
  client_settings: "/client-settings",
  logging_settings: "/logs-settings",
  log_erasure: "/log-erasure",
  compatibility_policy: "/compatibility",
  cluster_node: "/cluster",
};

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// server-side paginated, filtered audit log: action/target/actor/time-range
// filters map to query params, pagination walks the keyset cursor
export default function AuditLog() {
  const { t } = useTranslation();
  const rangeLabels: Record<(typeof RANGES)[number]["id"], string> = {
    last24h: t("pages.auditLog.ranges.last24h"),
    last7d: t("pages.auditLog.ranges.last7d"),
    last30d: t("pages.auditLog.ranges.last30d"),
    all: t("pages.auditLog.ranges.all"),
  };
  const fmt = useFormat();
  const scope = useScope();
  const [expanded, setExpanded] = React.useState<string | null>(null);

  // the deployment-wide read is offered on an explicit yes only: an unanswered
  // gate must not show a switch the control plane may then refuse
  const canReadDeployment = useCan()("deployment_audit_log", "read") === true;
  const [wide, setWide] = React.useState(false);
  const deployment = canReadDeployment && wide;

  const [actor, setActor] = React.useState("");
  const [action, setAction] = React.useState("");
  const [target, setTarget] = React.useState("");
  const [rangeIdx, setRangeIdx] = React.useState(DEFAULT_RANGE);
  const [cursors, setCursors] = React.useState<string[]>([]);
  const cursor = cursors[cursors.length - 1];

  const from = React.useMemo(() => {
    const hours = RANGES[rangeIdx].hours;
    return hours != null ? new Date(Date.now() - hours * 3_600_000).toISOString() : undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeIdx, cursor]);

  // who the actor ids are, so the actor column can say who rather than the
  // first eight hex digits of a uuid, and the actor filter can be picked by
  // e-mail. the org scope reads the org's accounts. a superadmin also lists the
  // accounts that hold no membership anywhere, such as the operator
  // `rolter-seed --admin-email` creates: they are on no org's list yet are the
  // actor of whatever they write (#2858). the control plane ignores the flag
  // for anyone else. it is in the key, as on the Users screen, because
  // ModelSheet caches the plain list under `["users", orgId]` and the two
  // answers must not stand in for each other
  const callerIsSuperadmin = !!useOptionalAuth()?.user?.is_superadmin;
  // the whole-deployment scope reads every account in the deployment as well,
  // since an actor there can belong to any org and the org's list does not
  // reach the ones that belong to another (#2871). like the scope switch it is
  // offered on an explicit yes only, so an unanswered gate never fires a call
  // the control plane may then refuse. the org's list stays the answer while
  // the deployment's is on its way or failed, so the names the screen already
  // had do not vanish into short ids
  const canListDeployment = useCan()("deployment_user", "read") === true;
  const wholeDeployment = deployment && canListDeployment;
  const orgUsers = useQuery({
    queryKey: ["users", scope.orgId, { includeUnassigned: callerIsSuperadmin }],
    queryFn: () => fetchUsers(scope.orgId as string, { includeUnassigned: callerIsSuperadmin }),
    enabled: !!scope.orgId,
  });
  const deploymentUsers = useQuery({
    queryKey: ["deployment-users"],
    queryFn: fetchDeploymentUsers,
    enabled: wholeDeployment,
  });
  const people = wholeDeployment && deploymentUsers.data ? deploymentUsers.data : orgUsers.data;
  const emailOf = (id: string) =>
    Array.isArray(people) ? people.find((u) => u.id === id)?.email : undefined;
  const actorParam = UUID_RE.test(actor.trim()) ? actor.trim() : undefined;

  const page = useQuery({
    queryKey: [
      "audit-log",
      deployment ? "deployment" : scope.orgId,
      action,
      target,
      actorParam,
      rangeIdx,
      cursor,
    ],
    queryFn: () => {
      const query = {
        limit: PAGE_SIZE,
        cursor,
        action: action || undefined,
        target_type: target || undefined,
        actor: actorParam,
        from,
        include_total: !cursor,
      };
      return deployment
        ? fetchDeploymentAuditLogPage(query)
        : fetchAuditLogPage(scope.orgId as string, query);
    },
    enabled: deployment || !!scope.orgId,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `page` is the query the user is actually waiting on for this screen

  useScreenReady(!page.isLoading);

  const [total, setTotal] = React.useState<number | null>(null);

  // reset to the first page whenever the filter set changes
  React.useEffect(() => {
    setCursors([]);
    setTotal(null);
  }, [action, target, actorParam, rangeIdx, deployment]);

  const rows = page.data?.items ?? [];
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

  // the whole filtered log, streamed by the control plane with no row cap
  // (#3051) and never held in the tab (#3545): `ExportCsvButton` streams it to
  // a picked file where the browser can, and otherwise hands it to the
  // browser's download manager through a single-use ticket. the window is
  // anchored at the moment of the press, like the screen's own
  const orgLabel = deployment
    ? "deployment"
    : (scope.orgs.find((o) => o.id === scope.orgId)?.slug ?? scope.orgId ?? "org");
  const exportFrom = () => {
    const hours = RANGES[rangeIdx].hours;
    return hours != null ? new Date(Date.now() - hours * 3_600_000).toISOString() : undefined;
  };
  const exportFilters = (from: string | undefined) => ({
    action: action || undefined,
    target_type: target || undefined,
    actor: actorParam,
    from,
  });
  // a control plane that predates the export: the filters read again a page
  // at a time, so the file still holds the filtered log and not the 25 rows on
  // screen (#2130), capped as it always was there
  const readPagesInstead = async (format: AuditExportFormat) => {
    const from = exportFrom();
    const { entries, truncated } = await collectAuditLog(
      (query) =>
        deployment
          ? fetchDeploymentAuditLogPage(query)
          : fetchAuditLogPage(scope.orgId as string, query),
      exportFilters(from),
    );
    const out = entries.map((e) => auditExportRow(e, emailOf));
    downloadText(
      auditExportName({ org: orgLabel, from, format }),
      format === "csv" ? auditCsv(out) : auditJson(out),
      format === "csv" ? "text/csv" : "application/json",
    );
    return {
      tone: truncated ? ("info" as const) : ("success" as const),
      title: t(truncated ? "pages.auditLog.export.truncated" : "pages.auditLog.export.done", {
        count: out.length,
      }),
    };
  };

  const columns: TableColumn<AuditLogEntry>[] = [
    {
      key: "at",
      header: t("pages.auditLog.columns.time"),
      mono: true,
      render: (v) => fmt.dateTime(v as string),
    },
    {
      key: "actor_user_id",
      header: t("pages.auditLog.columns.actor"),
      mono: true,
      render: (v, row) =>
        v ? (
          emailOf(String(v)) ? (
            <span title={String(v)}>{emailOf(String(v))}</span>
          ) : Array.isArray(people) ? (
            // the list answered and the account is not on it, so a prefix of its
            // id would only look like a name. the whole id is in the open entry
            <Badge tone="outline" title={String(v)}>
              {t(deployment ? "pages.auditLog.actorUnknown" : "pages.auditLog.actorNotInOrg")}
            </Badge>
          ) : (
            <span title={String(v)}>{String(v).slice(0, 8)}</span>
          )
        ) : deployment && row.action.startsWith("auth.") ? (
          // a sign-in attempt against an address nobody registered has no actor
          <Badge tone="outline">{t("pages.auditLog.unknownAddress")}</Badge>
        ) : (
          t("pages.auditLog.systemActor")
        ),
    },
    ...(deployment
      ? [
          {
            key: "org_id",
            header: t("pages.auditLog.columns.org"),
            mono: true,
            render: (v: unknown) =>
              v ? (
                <span title={String(v)}>{String(v).slice(0, 8)}</span>
              ) : (
                <Badge tone="outline">{t("pages.auditLog.noOrg")}</Badge>
              ),
          },
        ]
      : []),
    {
      key: "action",
      header: t("pages.auditLog.columns.action"),
      render: (v) => <Badge tone="outline">{v as string}</Badge>,
    },
    {
      key: "target_type",
      header: t("pages.auditLog.columns.target"),
      render: (v, row) => {
        if (!v) return "—";
        const path = TARGET_PATH[v as string];
        const label = (
          <span
            className="font-mono text-xs"
            title={row.target_id ? String(row.target_id) : undefined}
          >
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
      header: t("pages.auditLog.columns.detail"),
      render: (_v, row) => (
        <button
          type="button"
          className="rounded-sm text-xs text-muted-foreground underline underline-offset-2 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          aria-expanded={expanded === row.id}
          aria-label={t(
            expanded === row.id
              ? "pages.auditLog.hideDetailsAria"
              : "pages.auditLog.showDetailsAria",
            { action: row.action, target: row.target_type || t("pages.auditLog.unknownTarget") },
          )}
          onClick={() => setExpanded(expanded === row.id ? null : row.id)}
        >
          {expanded === row.id ? t("pages.auditLog.hideDetails") : t("pages.auditLog.showDetails")}
        </button>
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
          target="audit-log"
        />
      )}
      {!deployment && !scope.isLoading && !scope.errorKey && !scope.orgId && (
        <EmptyState
          uxTarget="audit-log-no-org"
          icon={<Building2 />}
          title={t("pages.auditLog.noOrgTitle")}
          description={t("pages.auditLog.noOrgBody")}
        />
      )}

      {(deployment || scope.orgId) && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            {canReadDeployment && (
              <Segmented
                value={wide ? "deployment" : "org"}
                options={[
                  { value: "org", label: t("pages.auditLog.scopes.org") },
                  { value: "deployment", label: t("pages.auditLog.scopes.deployment") },
                ]}
                onChange={(val) => setWide(val === "deployment")}
                ariaLabel={t("pages.auditLog.scopeAria")}
              />
            )}
            {(!deployment || callerIsSuperadmin || wholeDeployment) &&
            people &&
            people.length > 0 ? (
              <Combobox
                className="w-[280px]"
                aria-label={t("pages.auditLog.actorFilterAria")}
                value={actor}
                onChange={setActor}
                // the deployment-wide read lists every account, but an actor whose
                // account was deleted is on no list and is still filtered by
                // pasting its id
                allowCustom={deployment}
                options={[
                  { value: "", label: t("pages.auditLog.anyActor") },
                  ...people.map((u) => ({ value: u.id, label: u.email })),
                ]}
              />
            ) : (
              <Input
                className="w-[280px] font-mono text-xs"
                aria-label={t("pages.auditLog.actorFilterAria")}
                placeholder={t("pages.auditLog.actorPlaceholder")}
                value={actor}
                onChange={(e) => setActor(e.target.value)}
              />
            )}
            <Combobox
              className="w-60"
              aria-label={t("pages.auditLog.actionFilterAria")}
              value={action}
              onChange={setAction}
              options={[
                { value: "", label: t("pages.auditLog.allActions") },
                ...groupedActions().map((a) => ({
                  value: a,
                  label: a,
                  group: t(`pages.auditLog.actionGroups.${auditGroup(a)}`),
                })),
              ]}
            />
            <Combobox
              className="w-44"
              aria-label={t("pages.auditLog.targetFilterAria")}
              value={target}
              onChange={setTarget}
              options={[
                { value: "", label: t("pages.auditLog.allTargets") },
                ...AUDIT_TARGET_TYPES.map((kind) => ({ value: kind, label: kind })),
              ]}
            />
            <Segmented
              value={RANGES[rangeIdx].id}
              options={RANGES.map((r) => ({ value: r.id, label: rangeLabels[r.id] }))}
              onChange={(val) => {
                const idx = RANGES.findIndex((r) => r.id === val);
                if (idx !== -1) setRangeIdx(idx);
              }}
              ariaLabel={t("pages.auditLog.timeRangeAria")}
            />
            <div className="ml-auto flex gap-1">
              {(["csv", "json"] as const).map((format) => (
                <ExportCsvButton
                  key={format}
                  toDisk
                  fileType={format}
                  label={t(`pages.auditLog.export.${format}`)}
                  what={t("errors.resources.auditLogExport")}
                  disabled={page.isLoading || !!page.error}
                  request={() =>
                    auditLogExport(
                      deployment ? "deployment" : (scope.orgId as string),
                      format,
                      exportFilters(exportFrom()),
                    )
                  }
                  fallbackName={() =>
                    auditExportName({ org: orgLabel, from: exportFrom(), format })
                  }
                  done={{
                    saved: t("pages.auditLog.export.saved"),
                    downloaded: t("pages.auditLog.export.downloaded"),
                  }}
                  fallback={() => readPagesInstead(format)}
                  // the audit log export has no row or time limit
                  bounded={false}
                />
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
              detailRow={(row) =>
                expanded === (row as unknown as AuditLogEntry).id ? (
                  <EntryDetail entry={row as unknown as AuditLogEntry} actorEmail={emailOf} />
                ) : null
              }
              read={page}
              empty={
                <EmptyState
                  thread
                  uxTarget="audit-log"
                  icon={<ScrollText />}
                  title={
                    filtersActive
                      ? t("pages.auditLog.noMatchTitle")
                      : t("pages.auditLog.emptyTitle")
                  }
                  description={
                    filtersActive ? t("pages.auditLog.noMatchBody") : t("pages.auditLog.emptyBody")
                  }
                  actions={
                    filtersActive ? (
                      <Button variant="outline" onClick={clearFilters}>
                        {t("common.clearFilters")}
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
                {t("pages.auditLog.page", { page: cursors.length + 1 })}
                {total != null && t("pages.auditLog.totalSuffix", { total })}
              </span>
              <div className="flex gap-1">
                <Button
                  size="sm"
                  variant="outline"
                  aria-label={t("pages.auditLog.prevPageAria")}
                  disabled={cursors.length === 0}
                  onClick={() => setCursors((c) => c.slice(0, -1))}
                >
                  {t("pages.auditLog.prev")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  aria-label={t("pages.auditLog.nextPageAria")}
                  disabled={!page.data?.has_next || !page.data.next_cursor}
                  onClick={() =>
                    page.data?.next_cursor &&
                    setCursors((c) => [...c, page.data.next_cursor as string])
                  }
                >
                  {t("pages.auditLog.next")}
                </Button>
              </div>
            </div>
          )}
        </>
      )}
    </PageBody>
  );
}

// the open entry: labelled with what it is, with every id whole and copyable,
// so it stands as evidence on its own once it is copied or photographed (#2129)
function EntryDetail({
  entry,
  actorEmail,
}: {
  entry: AuditLogEntry;
  actorEmail: (id: string) => string | undefined;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const target = entry.target_type || t("pages.auditLog.unknownTarget");
  const heading = `${entry.action} · ${target} · ${fmt.dateTime(entry.at)}`;
  const email = entry.actor_user_id ? actorEmail(entry.actor_user_id) : undefined;
  const ids: [string, string, string | null | undefined][] = [
    ["entryId", t("pages.auditLog.entry.entryId"), entry.id],
    ["actorId", t("pages.auditLog.entry.actorId"), entry.actor_user_id],
    ["targetId", t("pages.auditLog.entry.targetId"), entry.target_id],
    ["orgId", t("pages.auditLog.entry.orgId"), entry.org_id],
  ];
  return (
    <section aria-label={heading} className="flex min-w-0 flex-col gap-3 text-xs">
      <h3 className="text-sm font-medium text-foreground">{heading}</h3>
      <dl className="grid grid-cols-[max-content_1fr] items-start gap-x-4 gap-y-1.5">
        <dt className="text-muted-foreground">{t("pages.auditLog.entry.at")}</dt>
        <dd className="font-mono text-foreground">{entry.at}</dd>
        {email && (
          <>
            <dt className="text-muted-foreground">{t("pages.auditLog.entry.actor")}</dt>
            <dd className="text-foreground">{email}</dd>
          </>
        )}
        {ids.map(([key, label, value]) => (
          <React.Fragment key={key}>
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="min-w-0">
              {value ? (
                <CopyableText
                  variant="inline"
                  value={value}
                  copyLabel={t("pages.auditLog.entry.copy", { what: label })}
                  testId={`audit-${key}`}
                />
              ) : (
                <span className="text-muted-foreground">
                  {key === "actorId" ? t("pages.auditLog.systemActor") : "—"}
                </span>
              )}
            </dd>
          </React.Fragment>
        ))}
      </dl>
      {entry.detail != null && (
        /* the entry's recorded detail, through the shared code block so a
           changed field is picked out rather than buried in monospace (#949) */
        <CodeBlock
          value={JSON.stringify(entry.detail, null, 2)}
          language="json"
          label={t("pages.auditLog.detail")}
          maxHeight={256}
        />
      )}
    </section>
  );
}
