import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import * as React from "react";

import {
  ProviderGroupSheet,
  type ProviderGroupSheetMode,
} from "@/components/ProviderGroupSheet";
import { CopyButton } from "@/components/CopyButton";
import {
  ListHeader,
  ListRow,
  ListTable,
  PageBody,
  SearchInput,
  SortLabel,
  useSort,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { useScope } from "@/lib/scope";

const GRID = "1.2fr 1fr 1.2fr 2fr 108px";

export default function ProviderGroups() {
  const queryClient = useQueryClient();
  const scope = useScope();

  const groups = useQuery({
    queryKey: ["provider-groups", scope.orgId],
    queryFn: () => fetchProviderGroups(scope.orgId as string),
    enabled: !!scope.orgId,
  });
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

  const scopeBlocked = !scope.isLoading && !!scope.error;

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
      <div className="flex items-center gap-3">
        <SearchInput
          placeholder="Search provider groups"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <Button
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.orgId}
        >
          + Add group
        </Button>
      </div>

      {groups.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {groups.error && (
        <p className="text-sm text-destructive">Failed to load provider groups.</p>
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scope.error}. Read-only view still works.
        </p>
      )}
      {!scope.isLoading && !scope.error && !scope.orgId && (
        <p className="text-sm text-muted-foreground">
          No org configured yet — pick or create one to manage provider groups.
        </p>
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
              <Button
                size="sm"
                variant="outline"
                className="h-[30px]"
                onClick={() => setSheet({ mode: "edit", group })}
              >
                Edit
              </Button>
              <button
                type="button"
                title="Delete provider group"
                onClick={() => setDeleteTarget(group)}
                className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger)]"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </ListRow>
        ))}
        {!groups.isLoading && rows.length === 0 && (
          <p className="px-4 py-8 text-center text-sm text-muted-foreground">
            No provider groups match.
          </p>
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
          <p className="text-xs text-destructive">{(removeGroup.error as Error).message}</p>
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
              removeGroup.mutate(deleteTarget.id, {
                onSuccess: () => setDeleteTarget(null),
              });
            }}
          >
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </PageBody>
  );
}
