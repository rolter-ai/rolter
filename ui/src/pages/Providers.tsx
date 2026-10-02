import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { Building2, Plug, Tag } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ProviderSheet, type ProviderSheetMode } from "@/components/ProviderSheet";
import { useOrgScope, orgScopeText } from "@/components/OrgScopePicker";
import { ProjectScopeBadge } from "@/components/ProjectScopeField";
import { GatedButton } from "@/components/GatedButton";
import { LabelChips, LabelFilterSelect, LabelSheet, useSubjectLabels } from "@/components/Labels";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton, LoadingRegion } from "@/components/LoadingState";
import { UnservedConfigNotice } from "@/components/UnservedConfigNotice";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListLoadingRow,
  ListRow,
  ListTable,
  PageBody,
  SearchInput,
  SortLabel,
  Toolbar,
  useSort,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CopyButton } from "@/components/CopyButton";
import {
  deleteProvider,
  fetchConfig,
  fetchConfigProblems,
  fetchProviderGroups,
  fetchProviders,
  type GatewayConfigDto,
  type ProviderGroupRow,
  type ProviderRow,
} from "@/lib/api";
import { providerUsage, type UsageEntry } from "@/lib/provider-usage";
import { RowCapabilityScope } from "@/lib/can";
import { rowGateScope } from "@/lib/provider-scope";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PROVIDERS_QUERY_KEY = ["providers"];

// how many names the delete confirm spells out before it says "and N more"
const USAGE_NAMES_SHOWN = 8;

function UsageNames({
  label,
  entries,
  only,
}: {
  label: string;
  entries: UsageEntry[];
  only: string;
}) {
  const { t } = useTranslation();
  const shown = entries.slice(0, USAGE_NAMES_SHOWN);
  const rest = entries.length - shown.length;
  return (
    <div className="flex flex-col gap-1">
      <p className="text-xs font-medium text-foreground">{label}</p>
      <ul className="flex flex-wrap gap-x-3 gap-y-1">
        {shown.map((entry) => (
          <li key={entry.name} className="text-xs">
            <span className="break-all font-mono text-[color:var(--text-secondary)]">
              {entry.name}
            </span>
            {entry.only && <span className="ml-1.5 text-muted-foreground">{only}</span>}
          </li>
        ))}
        {rest > 0 && (
          <li className="text-xs text-muted-foreground">
            {t("pages.providers.usage.more", { count: rest })}
          </li>
        )}
      </ul>
    </div>
  );
}

/**
 * What still points at the provider the delete confirm is about (#2143).
 *
 * A provider can be a route target, a group member and an address of its own
 * (`slug/model`), and the confirm used to name none of them. Both reads happen
 * when the confirm opens rather than on every visit, and neither answer is
 * guessed: while they are out the space is held, and a failed read says so with
 * a retry instead of claiming nothing uses the provider.
 */
function ProviderUsageNotice({
  provider,
  config,
  groups,
}: {
  provider: ProviderRow;
  config: UseQueryResult<GatewayConfigDto>;
  groups: UseQueryResult<ProviderGroupRow[]>;
}) {
  const { t } = useTranslation();
  const failed = config.error ?? groups.error;
  const unread = config.isFetching || groups.isFetching || (!config.isSuccess && !config.isError);
  const usage = providerUsage(provider, config.data, groups.data);
  const unused = usage.routes.length === 0 && usage.groups.length === 0;
  return (
    <div aria-live="polite">
      {unread ? (
        <LoadingRegion className="w-full">
          <Skeleton height={44} radius={8} />
        </LoadingRegion>
      ) : failed ? (
        <LoadError
          error={failed}
          resource={t("pages.providers.usage.resource")}
          onRetry={() => {
            void config.refetch();
            void groups.refetch();
          }}
        />
      ) : unused ? (
        <p className="text-xs text-muted-foreground">{t("pages.providers.usage.none")}</p>
      ) : (
        <div className="flex flex-col gap-2.5 rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-3 py-2.5">
          <p className="text-xs text-[color:var(--text-secondary)]">
            {t("pages.providers.usage.inUse")}
          </p>
          {usage.routes.length > 0 && (
            <UsageNames
              label={t("pages.providers.usage.routes", { count: usage.routes.length })}
              entries={usage.routes}
              only={t("pages.providers.usage.onlyTarget")}
            />
          )}
          {usage.groups.length > 0 && (
            <UsageNames
              label={t("pages.providers.usage.groups", { count: usage.groups.length })}
              entries={usage.groups}
              only={t("pages.providers.usage.onlyMember")}
            />
          )}
        </div>
      )}
    </div>
  );
}

export default function Providers() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  // the names of the projects a row can be scoped to, for the Scope column
  const orgScope = useOrgScope(scope.orgId);
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;

  const providers = useQuery({
    queryKey: [...PROVIDERS_QUERY_KEY, scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  // what the control plane is dropping from the snapshot, so a provider that
  // silently stopped being served says so here (#926)
  const problems = useQuery({
    queryKey: ["config-problems"],
    queryFn: fetchConfigProblems,
  });

  const invalidate = () => {
    queryClient.invalidateQueries({
      queryKey: [...PROVIDERS_QUERY_KEY, scope.orgId],
    });
    // an edit that fixes (or breaks) a provider changes this answer too
    queryClient.invalidateQueries({ queryKey: ["config-problems"] });
  };

  const removeProvider = useMutation({
    mutationFn: (id: string) => deleteProvider(id),
    onSuccess: invalidate,
  });

  const [sheet, setSheet] = React.useState<{
    mode: ProviderSheetMode;
    provider?: ProviderRow | null;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<ProviderRow | null>(null);
  const { sort, cycle, apply } = useSort<
    "name" | "kind" | "apiBase" | "slug" | "keyEnv" | "scope"
  >();
  const [search, setSearch] = React.useState("");
  // the label the list is narrowed to, as `key=value`; "" is no filter
  const [labelFilter, setLabelFilter] = React.useState("");
  const [labelling, setLabelling] = React.useState<ProviderRow | null>(null);

  const labels = useSubjectLabels(scope.orgId, "provider");

  // what points at the provider a delete is about to remove, read when the
  // confirm opens. the keys are the ones the Models and Provider Groups screens
  // read, so a visit to either already holds an answer to refresh
  const usageConfig = useQuery({
    queryKey: ["config"],
    queryFn: fetchConfig,
    enabled: !!deleteTarget,
    retry: false,
  });
  const usageGroups = useQuery({
    queryKey: ["provider-groups", scope.orgId],
    queryFn: () => fetchProviderGroups(scope.orgId as string),
    enabled: !!deleteTarget && !!scope.orgId,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider
  useScreenReady(!providers.isLoading);
  useErrorState(!!providers.error, "provider-list");

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  // editing and deleting a provider are an admin's, the same as adding one
  // (#1258)

  const q = search.trim().toLowerCase();
  const filtered = (providers.data ?? []).filter(
    (p) =>
      (!q ||
        p.name.toLowerCase().includes(q) ||
        p.kind.toLowerCase().includes(q) ||
        p.slug.toLowerCase().includes(q)) &&
      labels.matches(p.id, labelFilter),
  );
  const rows = apply(filtered, {
    name: (p) => p.name,
    kind: (p) => p.kind,
    apiBase: (p) => p.api_base,
    slug: (p) => p.slug,
    keyEnv: (p) => p.api_key_env ?? "",
    // org-wide rows first, then by the project's name
    scope: (p) => (p.project_id ? orgScopeText(t, orgScope, { project_id: p.project_id }) : ""),
  });
  const filtering = !!q || !!labelFilter;

  const GRID = "1fr 1.1fr 1.7fr 1fr 1fr 1fr 108px";

  return (
    <PageBody>
      <UnservedConfigNotice problems={problems.data ?? []} />

      <Toolbar>
        <SearchInput
          placeholder={t("pages.providers.search")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <LabelFilterSelect value={labelFilter} onChange={setLabelFilter} options={labels.options} />
        <GatedButton
          gate="provider:create"
          control="provider-new"
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.orgId}
        >
          {t("pages.providers.add")}
        </GatedButton>
      </Toolbar>

      {providers.error && (
        <LoadError
          error={providers.error}
          resource={t("errors.resources.providers")}
          onRetry={() => providers.refetch()}
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("pages.providers.scopeBlocked", { error: scopeMessage })}
        </p>
      )}
      {!scope.isLoading && !scope.errorKey && !scope.orgId && (
        <EmptyState
          uxTarget="providers-no-org"
          icon={<Building2 />}
          title={t("pages.providers.noOrgTitle")}
          description={t("pages.providers.noOrg")}
        />
      )}

      <ListTable label={t("screens.providers.title")}>
        <ListHeader grid={GRID}>
          <SortLabel
            label={t("pages.providers.colName")}
            col="name"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.providers.colType")}
            col="kind"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.providers.colApiBase")}
            col="apiBase"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.providers.colSlug")}
            col="slug"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.providers.colKeyEnv")}
            col="keyEnv"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel
            label={t("pages.providers.colScope")}
            col="scope"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={providers}>
          <ListSkeleton rows={4} className="p-3" />
        </ListLoadingRow>
        {rows.map((provider) => (
          <ListRow key={provider.id} grid={GRID}>
            <ListCell className="flex min-w-0 flex-col gap-1">
              <span className="truncate font-mono text-sm">{provider.name}</span>
              <LabelChips labels={labels.bySubject(provider.id)} />
            </ListCell>
            <ListCell>
              <Badge tone="outline">{provider.kind}</Badge>
            </ListCell>
            <ListCell className="truncate font-mono text-xs text-muted-foreground">
              {provider.api_base}
            </ListCell>
            <ListCell className="flex min-w-0 items-center gap-1">
              <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
                {provider.slug}
              </span>
              <CopyButton
                value={`${provider.slug}/`}
                label={t("pages.providers.copyPrefix")}
                className="h-6 px-1"
              />
            </ListCell>
            <ListCell className="truncate font-mono text-xs text-muted-foreground">
              {provider.api_key_env || "—"}
            </ListCell>
            <ListCell className="grid">
              <ProjectScopeBadge projectId={provider.project_id} scope={orgScope} />
            </ListCell>
            <ListCell className="flex items-center justify-end gap-1.5">
              <RowCapabilityScope at={rowGateScope(provider, orgScope.byTeam)}>
                <GatedButton
                  gate="provider:update"
                  control="provider-edit"
                  size="sm"
                  variant="outline"
                  className="h-[30px]"
                  aria-label={t("pages.providers.editOne", { name: provider.name })}
                  onClick={() => setSheet({ mode: "edit", provider })}
                >
                  {t("pages.providers.edit")}
                </GatedButton>
                <Button
                  size="sm"
                  variant="outline"
                  className="h-[30px]"
                  aria-label={t("labels.labelsOf", { name: provider.name })}
                  onClick={() => setLabelling(provider)}
                >
                  <Tag className="h-3.5 w-3.5" />
                </Button>
                <DeleteIconButton
                  gate="provider:delete"
                  control="provider-delete"
                  label={t("pages.providers.deleteOne", { name: provider.name })}
                  title={t("pages.providers.deleteTitle")}
                  onClick={() => setDeleteTarget(provider)}
                />
              </RowCapabilityScope>
            </ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={providers} rows={rows.length}>
          <EmptyState
            uxTarget="providers"
            icon={<Plug />}
            title={filtering ? t("pages.providers.noMatch") : t("pages.providers.emptyTitle")}
            description={
              filtering ? t("pages.providers.noMatchBody") : t("pages.providers.emptyBody")
            }
            actions={
              filtering ? (
                <Button
                  variant="outline"
                  onClick={() => {
                    setSearch("");
                    setLabelFilter("");
                  }}
                >
                  {t("common.clearFilters")}
                </Button>
              ) : (
                <GatedButton
                  gate="provider:create"
                  control="provider-new-empty"
                  disabled={scopeBlocked || !scope.orgId}
                  onClick={() => setSheet({ mode: "add" })}
                >
                  {t("pages.providers.add")}
                </GatedButton>
              )
            }
          />
        </ListEmptyRow>
      </ListTable>

      {scope.orgId && labelling && (
        <LabelSheet
          open
          onOpenChange={(open) => !open && setLabelling(null)}
          orgId={scope.orgId}
          subjectType="provider"
          subjectId={labelling.id}
          subjectName={labelling.name}
        />
      )}

      <ProviderSheet
        open={!!sheet}
        mode={sheet?.mode ?? "add"}
        onOpenChange={(open) => !open && setSheet(null)}
        orgId={scope.orgId ?? null}
        provider={sheet?.provider ?? null}
        defaultProjectId={scope.projectId}
        onDone={invalidate}
      />

      <ConfirmDialog
        // stable key for the UX stream, the same one the hand-rolled dialog
        // emitted under so the series stays continuous (#1738)
        name="provider-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDeleteTarget(null);
          // a failure from this row must not greet the next one opened
          removeProvider.reset();
        }}
        title={t("pages.providers.confirm.deleteTitle", { name: deleteTarget?.name ?? "" })}
        description={
          <Trans
            i18nKey="pages.providers.confirm.deleteBody"
            values={{ slug: deleteTarget?.slug ?? "" }}
            components={{ code: <code className="break-all font-mono" /> }}
          />
        }
        confirmLabel={t("pages.providers.confirm.deleteConfirm")}
        pending={removeProvider.isPending}
        error={removeProvider.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const name = deleteTarget.name;
          removeProvider.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what: name }) });
            },
            onError: (error) =>
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what: name }),
                detail: errorDetail(error),
              }),
          });
        }}
      >
        {deleteTarget && (
          <ProviderUsageNotice provider={deleteTarget} config={usageConfig} groups={usageGroups} />
        )}
      </ConfirmDialog>
    </PageBody>
  );
}
