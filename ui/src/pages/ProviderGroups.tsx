import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building2, Layers, Loader2, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import {
  ProviderGroupSheet,
  type ProviderGroupSheetMode,
} from "@/components/ProviderGroupSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { CopyButton } from "@/components/CopyButton";
import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  SearchInput,
  SortLabel,
  useSort,
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
import {
  deleteProviderGroup,
  fetchProviderGroups,
  fetchProviders,
  type ProviderGroupRow,
} from "@/lib/api";
import { useGate } from "@/lib/can";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const GRID = "1.2fr 1fr 1.2fr 2fr 108px";

export default function ProviderGroups() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;

  const groups = useQuery({
    queryKey: ["provider-groups", scope.orgId],
    queryFn: () => fetchProviderGroups(scope.orgId as string),
    enabled: !!scope.orgId,
  });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `groups` is the query the user is actually waiting on for this screen

  useScreenReady(!groups.isLoading);

  useErrorState(!!groups.error, "provider-groups");
  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["provider-groups", scope.orgId] });

  const removeGroup = useMutation({
    mutationFn: (id: string) => deleteProviderGroup(id),
    onSuccess: invalidate,
  });

  const [search, setSearch] = React.useState("");
  const { sort, cycle, apply } = useSort<"name" | "strategy" | "slug" | "members">();
  const [sheet, setSheet] = React.useState<{
    mode: ProviderGroupSheetMode;
    group?: ProviderGroupRow | null;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<ProviderGroupRow | null>(null);

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;
  // a group is edited and deleted by the same admin that may add one (#1258)
  const deleteGate = useGate("provider_group:delete");

  const q = search.trim().toLowerCase();
  const filtered = (groups.data ?? []).filter(
    (g) =>
      !q ||
      g.name.toLowerCase().includes(q) ||
      g.slug.toLowerCase().includes(q) ||
      g.strategy.toLowerCase().includes(q),
  );
  const rows = apply(filtered, {
    name: (g) => g.name,
    strategy: (g) => g.strategy,
    slug: (g) => g.slug,
    members: (g) => g.members.length,
  });

  return (
    <PageBody>
      <Toolbar>
        <SearchInput
          placeholder="Search provider groups"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <GatedButton
          gate="provider_group:create"
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.orgId}
        >
          + Add group
        </GatedButton>
      </Toolbar>

      {groups.error && (
        <LoadError
          error={groups.error}
          resource={t("errors.resources.providerGroups")}
          onRetry={() => groups.refetch()}
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scopeMessage}. Read-only view still works.
        </p>
      )}
      {!scope.isLoading && !scope.errorKey && !scope.orgId && (
        <EmptyState
          uxTarget="provider-groups-no-org"
          icon={<Building2 />}
          title={t("pages.providerGroups.noOrgTitle")}
          description={t("pages.providerGroups.noOrgBody")}
        />
      )}

      <ListTable>
        <ListHeader grid={GRID}>
          <SortLabel label="Name" col="name" sort={sort} onCycle={(c) => cycle(c as never)} />
          <SortLabel
            label="Strategy"
            col="strategy"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <SortLabel label="Address" col="slug" sort={sort} onCycle={(c) => cycle(c as never)} />
          <SortLabel
            label="Members"
            col="members"
            sort={sort}
            onCycle={(c) => cycle(c as never)}
          />
          <span />
        </ListHeader>
        {groups.isLoading && <ListSkeleton rows={4} className="p-3" />}
        {rows.map((group) => (
          <ListRow key={group.id} grid={GRID}>
            <span className="truncate font-mono text-sm">{group.name}</span>
            <span>
              <Badge tone="outline">{group.strategy}</Badge>
            </span>
            <span className="flex min-w-0 items-center gap-1">
              <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
                {group.slug}/
              </span>
              <CopyButton
                value={`${group.slug}/`}
                label="Copy address prefix"
                className="h-6 px-1"
              />
            </span>
            <span className="flex min-w-0 flex-wrap items-center gap-1">
              {group.members.length === 0 ? (
                <span className="text-xs text-muted-foreground">no members</span>
              ) : (
                group.members.map((m) => (
                  <Badge key={m.provider_id} tone="outline" className="font-mono text-[11px]">
                    {m.provider_name}
                    {m.weight !== 1 ? ` ·${m.weight}` : ""}
                  </Badge>
                ))
              )}
            </span>
            <div className="flex items-center justify-end gap-1.5">
              <GatedButton
                gate="provider_group:update"
                size="sm"
                variant="outline"
                className="h-[30px]"
                aria-label={t("pages.providerGroups.editOne", { name: group.name })}
                onClick={() => setSheet({ mode: "edit", group })}
              >
                Edit
              </GatedButton>
              <button
                type="button"
                title={
                  deleteGate.reason ??
                  t("pages.providerGroups.deleteOne", { name: group.name })
                }
                aria-label={t("pages.providerGroups.deleteOne", { name: group.name })}
                disabled={deleteGate.denied}
                onClick={() => setDeleteTarget(group)}
                className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </ListRow>
        ))}
        {!groups.isLoading && rows.length === 0 && (
          // the old copy said "No provider groups match." with no search
          // running, which blames a filter the operator never set (#1180)
          <EmptyState
            uxTarget="provider-groups"
            icon={<Layers />}
            title={q ? t("pages.providerGroups.noMatchTitle") : t("pages.providerGroups.emptyTitle")}
            description={
              q ? t("pages.providerGroups.noMatchBody") : t("pages.providerGroups.emptyBody")
            }
            actions={
              q ? (
                <Button variant="outline" onClick={() => setSearch("")}>
                  {t("common.clearSearch")}
                </Button>
              ) : (
                <GatedButton
                  gate="provider_group:create"
                  disabled={scopeBlocked || !scope.orgId}
                  onClick={() => setSheet({ mode: "add" })}
                >
                  {t("pages.providerGroups.emptyAction")}
                </GatedButton>
              )
            }
          />
        )}
      </ListTable>

      <ProviderGroupSheet
        open={!!sheet}
        mode={sheet?.mode ?? "add"}
        onOpenChange={(open) => !open && setSheet(null)}
        orgId={scope.orgId ?? null}
        providers={providers.data ?? []}
        group={sheet?.group ?? null}
        onDone={invalidate}
      />

      <Dialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete provider group</DialogTitle>
          <DialogDescription>
            <span className="font-mono">{deleteTarget?.name}</span> will stop resolving as a{" "}
            <span className="font-mono">{deleteTarget?.slug}/model</span> address. Member
            providers are unaffected. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        {removeGroup.isError && (
          <p className="text-xs text-[color:var(--status-danger-text)]">{(removeGroup.error as Error).message}</p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removeGroup.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              const what = deleteTarget.name;
              removeGroup.mutate(deleteTarget.id, {
                onSuccess: () => {
                  setDeleteTarget(null);
                  toast.push({ tone: "success", title: t("toast.deleted", { what }) });
                },
                onError: (error) => {
                  toast.push({
                    tone: "error",
                    title: t("toast.deleteFailed", { what }),
                    detail: errorDetail(error),
                  });
                },
              });
            }}
          >
            {removeGroup.isPending && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}
