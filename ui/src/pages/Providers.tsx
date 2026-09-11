import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building2, Plug, Trash2, Loader2 } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import {
  ProviderSheet,
  type ProviderSheetMode,
} from "@/components/ProviderSheet";
import { GatedButton } from "@/components/GatedButton";
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
import { EmptyState } from "@/components/ui/empty-state";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { CopyButton } from "@/components/CopyButton";
import {
  deleteProvider,
  fetchConfigProblems,
  fetchProviders,
  type ProviderRow,
} from "@/lib/api";
import { useGate } from "@/lib/can";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import {
  useErrorState,
  useFormTelemetry,
  useScreenReady,
} from "@/lib/ux-react";

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
  const [deleteTarget, setDeleteTarget] = React.useState<ProviderRow | null>(
    null,
  );
  const [search, setSearch] = React.useState("");

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider
  useScreenReady(!providers.isLoading);
  useErrorState(!!providers.error, "provider-list");
  const deleteUx = useFormTelemetry("provider-delete", !!deleteTarget);

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  // editing and deleting a provider are an admin's, the same as adding one
  // (#1258)
  const deleteGate = useGate("provider:delete");

  const q = search.trim().toLowerCase();
  const rows = (providers.data ?? []).filter(
    (p) =>
      !q ||
      p.name.toLowerCase().includes(q) ||
      p.kind.toLowerCase().includes(q) ||
      p.slug.toLowerCase().includes(q),
  );

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
        <GatedButton
          gate="provider:create"
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
            <span className="truncate font-mono text-sm">{provider.name}</span>
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
                size="sm"
                variant="outline"
                className="h-[30px]"
                aria-label={t("pages.providers.editOne", { name: provider.name })}
                onClick={() => setSheet({ mode: "edit", provider })}
              >
                {t("pages.providers.edit")}
              </GatedButton>
              <button
                type="button"
                title={deleteGate.reason ?? t("pages.providers.deleteTitle")}
                aria-label={t("pages.providers.deleteOne", {
                  name: provider.name,
                })}
                disabled={deleteGate.denied}
                onClick={() => setDeleteTarget(provider)}
                className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </ListRow>
        ))}
        {!providers.isLoading && rows.length === 0 && (
          <EmptyState
            uxTarget="providers"
            icon={<Plug />}
            title={q ? t("pages.providers.noMatch") : t("pages.providers.emptyTitle")}
            description={q ? t("pages.providers.noMatchBody") : t("pages.providers.emptyBody")}
            actions={
              q ? (
                <Button variant="outline" onClick={() => setSearch("")}>
                  {t("common.clearSearch")}
                </Button>
              ) : (
                <GatedButton
                  gate="provider:create"
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

      <ProviderSheet
        open={!!sheet}
        mode={sheet?.mode ?? "add"}
        onOpenChange={(open) => !open && setSheet(null)}
        orgId={scope.orgId ?? null}
        provider={sheet?.provider ?? null}
        onDone={invalidate}
      />

      <Dialog
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <DialogHeader>
          <DialogTitle>{t("pages.providers.deleteTitle")}</DialogTitle>
          <DialogDescription>
            <Trans
              i18nKey="pages.providers.deleteHint"
              values={{ name: deleteTarget?.name ?? "" }}
              components={[<span key="name" className="font-mono" />]}
            />
          </DialogDescription>
        </DialogHeader>
        {removeProvider.isError && (
          <p className="text-xs text-[color:var(--status-danger-text)]">
            {(removeProvider.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="destructive"
            disabled={removeProvider.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              deleteUx.submitted();
              const name = deleteTarget.name;
              removeProvider.mutate(deleteTarget.id, {
                onSuccess: () => {
                  deleteUx.saved();
                  setDeleteTarget(null);
                  toast.push({ tone: "success", title: t("toast.deleted", { what: name }) });
                },
                onError: (error) => {
                  deleteUx.failed();
                  toast.push({
                    tone: "error",
                    title: t("toast.deleteFailed", { what: name }),
                    detail: errorDetail(error),
                  });
                },
              });
            }}
          >
            {removeProvider.isPending && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            {t("common.delete")}
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}
