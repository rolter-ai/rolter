import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trash2 } from "lucide-react";
import * as React from "react";

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

const PROVIDERS_QUERY_KEY = ["providers"];

export default function Providers() {
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
          placeholder="Search providers"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <Button
          className="ml-auto"
          onClick={() => setSheet({ mode: "add" })}
          disabled={scopeBlocked || !scope.orgId}
        >
          + Add provider
        </Button>
      </div>

      {providers.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {providers.error && (
        <p className="text-sm text-destructive">Failed to load providers.</p>
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scope.error}. Read-only view still works.
        </p>
      )}
      {!scope.isLoading && !scope.error && !scope.orgId && (
        <p className="text-sm text-muted-foreground">
          No org configured yet — pick or create one to manage providers.
        </p>
      )}

      <ListTable>
        <ListHeader grid={GRID}>
          <span>Name</span>
          <span>Type</span>
          <span>API base</span>
          <span>Slug</span>
          <span>Key env</span>
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
                label="Copy address prefix"
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
                Edit
              </Button>
              <button
                type="button"
                title="Delete provider"
                onClick={() => setDeleteTarget(provider)}
                className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger)]"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </ListRow>
        ))}
        {!providers.isLoading && rows.length === 0 && (
          <p className="px-4 py-8 text-center text-sm text-muted-foreground">
            No providers match.
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
          <DialogTitle>Delete provider</DialogTitle>
          <DialogDescription>
            <span className="font-mono">{deleteTarget?.name}</span> will stop being
            usable as a route target. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        {removeProvider.isError && (
          <p className="text-xs text-destructive">
            {(removeProvider.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removeProvider.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              removeProvider.mutate(deleteTarget.id, {
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
