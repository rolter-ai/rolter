import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2, Loader2 } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ProviderSheet, type ProviderSheetMode } from "@/components/ProviderSheet";
import { ListHeader, ListRow, ListTable, PageBody, SearchInput } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { CopyButton } from "@/components/CopyButton";
import { deleteProvider, fetchProviders, type ProviderRow } from "@/lib/api";
import { useScope } from "@/lib/scope";
import { useErrorState, useFormTelemetry, useScreenReady } from "@/lib/ux-react";

const PROVIDERS_QUERY_KEY = ["providers"];

export default function Providers() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const scope = useScope();

  const providers = useQuery({
    queryKey: [...PROVIDERS_QUERY_KEY, scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: [...PROVIDERS_QUERY_KEY, scope.orgId] });

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

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider
  useScreenReady(!providers.isLoading);
  useErrorState(!!providers.error, "provider-list");
  const deleteUx = useFormTelemetry("provider-delete", !!deleteTarget);

  const scopeBlocked = !scope.isLoading && !!scope.error;

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
      <div className="flex items-center gap-3">
        <SearchInput
          placeholder={t("pages.providers.search")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <Button
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.orgId}
        >
          {t("pages.providers.add")}
        </Button>
      </div>

      {providers.isLoading && (
        <p className="text-sm text-muted-foreground">{t("pages.providers.loading")}</p>
      )}
      {providers.error && (
        <p className="text-sm text-destructive">{t("pages.providers.loadFailed")}</p>
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("pages.providers.scopeBlocked", { error: scope.error })}
        </p>
      )}
      {!scope.isLoading && !scope.error && !scope.orgId && (
        <p className="text-sm text-muted-foreground">{t("pages.providers.noOrg")}</p>
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
              <Button
                size="sm"
                variant="outline"
                className="h-[30px]"
                onClick={() => setSheet({ mode: "edit", provider })}
              >
                {t("pages.providers.edit")}
              </Button>
              <button
                type="button"
                title={t("pages.providers.deleteTitle")}
                aria-label={t("pages.providers.deleteOne", { name: provider.name })}
                onClick={() => setDeleteTarget(provider)}
                className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </ListRow>
        ))}
        {!providers.isLoading && rows.length === 0 && (
          <p className="px-4 py-8 text-center text-sm text-muted-foreground">
            {t("pages.providers.noMatch")}
          </p>
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

      <Dialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
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
          <p className="text-xs text-destructive">
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
              removeProvider.mutate(deleteTarget.id, {
                onSuccess: () => {
                  deleteUx.saved();
                  setDeleteTarget(null);
                },
                onError: () => deleteUx.failed(),
              });
            }}
          >
            {removeProvider.isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("common.delete")}
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}
