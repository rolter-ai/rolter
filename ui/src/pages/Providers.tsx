import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building2, Plug, Tag } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ProviderSheet, type ProviderSheetMode } from "@/components/ProviderSheet";
import { GatedButton } from "@/components/GatedButton";
import { LabelChips, LabelFilterSelect, LabelSheet, useSubjectLabels } from "@/components/Labels";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { UnservedConfigNotice } from "@/components/UnservedConfigNotice";
import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  SearchInput,
  Toolbar,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { CopyButton } from "@/components/CopyButton";
import { deleteProvider, fetchConfigProblems, fetchProviders, type ProviderRow } from "@/lib/api";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const PROVIDERS_QUERY_KEY = ["providers"];

export default function Providers() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
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
  const [search, setSearch] = React.useState("");
  // the label the list is narrowed to, as `key=value`; "" is no filter
  const [labelFilter, setLabelFilter] = React.useState("");
  const [labelling, setLabelling] = React.useState<ProviderRow | null>(null);

  const labels = useSubjectLabels(scope.orgId, "provider");

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider
  useScreenReady(!providers.isLoading);
  useErrorState(!!providers.error, "provider-list");

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  // editing and deleting a provider are an admin's, the same as adding one
  // (#1258)

  const q = search.trim().toLowerCase();
  const rows = (providers.data ?? []).filter(
    (p) =>
      (!q ||
        p.name.toLowerCase().includes(q) ||
        p.kind.toLowerCase().includes(q) ||
        p.slug.toLowerCase().includes(q)) &&
      labels.matches(p.id, labelFilter),
  );
  const filtering = !!q || !!labelFilter;

  const GRID = "1fr 1.1fr 2fr 1fr 1fr 108px";

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

      <ListTable>
        <ListHeader grid={GRID}>
          <span>{t("pages.providers.colName")}</span>
          <span>{t("pages.providers.colType")}</span>
          <span>{t("pages.providers.colApiBase")}</span>
          <span>{t("pages.providers.colSlug")}</span>
          <span>{t("pages.providers.colKeyEnv")}</span>
          <span />
        </ListHeader>
        {providers.isLoading && <ListSkeleton rows={4} className="p-3" />}
        {rows.map((provider) => (
          <ListRow key={provider.id} grid={GRID}>
            <span className="flex min-w-0 flex-col gap-1">
              <span className="truncate font-mono text-sm">{provider.name}</span>
              <LabelChips labels={labels.bySubject(provider.id)} />
            </span>
            <span>
              <Badge tone="outline">{provider.kind}</Badge>
            </span>
            <span className="truncate font-mono text-xs text-muted-foreground">
              {provider.api_base}
            </span>
            <span className="flex min-w-0 items-center gap-1">
              <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
                {provider.slug}
              </span>
              <CopyButton
                value={`${provider.slug}/`}
                label={t("pages.providers.copyPrefix")}
                className="h-6 px-1"
              />
            </span>
            <span className="truncate font-mono text-xs text-muted-foreground">
              {provider.api_key_env || "—"}
            </span>
            <div className="flex items-center justify-end gap-1.5">
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
            </div>
          </ListRow>
        ))}
        {!providers.isLoading && rows.length === 0 && (
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
                  {t("common.clearSearch")}
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
        )}
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
        description={t("pages.providers.confirm.deleteBody")}
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
      />
    </PageBody>
  );
}
