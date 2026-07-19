import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import * as React from "react";

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
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  createProvider,
  deleteProvider,
  fetchProviders,
  PROVIDER_KINDS,
  updateProvider,
  type ProviderRow,
} from "@/lib/api";
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

  const [addOpen, setAddOpen] = React.useState(false);
  const [editTarget, setEditTarget] = React.useState<ProviderRow | null>(null);
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
          onClick={() => setAddOpen(true)}
          disabled={scopeBlocked || !scope.orgId}
        >
          <Plus className="h-4 w-4" />
          Add provider
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
                onClick={() => setEditTarget(provider)}
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

      {scope.orgId && (
        <AddProviderDialog
          open={addOpen}
          onOpenChange={setAddOpen}
          orgId={scope.orgId}
          onDone={invalidate}
        />
      )}

      <EditProviderDialog
        provider={editTarget}
        onOpenChange={(open) => !open && setEditTarget(null)}
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

export function AddProviderDialog({
  open,
  onOpenChange,
  orgId,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  onDone: (created: ProviderRow) => void;
}) {
  const [name, setName] = React.useState("");
  const [slug, setSlug] = React.useState("");
  const [kind, setKind] = React.useState<string>(PROVIDER_KINDS[0]);
  const [apiBase, setApiBase] = React.useState("");
  const [apiKey, setApiKey] = React.useState("");
  const [apiKeyEnv, setApiKeyEnv] = React.useState("");
  const [egressProxy, setEgressProxy] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setName("");
      setSlug("");
      setKind(PROVIDER_KINDS[0]);
      setApiBase("");
      setApiKey("");
      setApiKeyEnv("");
      setEgressProxy("");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createProvider(orgId, {
        name,
        slug: slug.trim() || undefined,
        kind,
        api_base: apiBase,
        api_key: apiKey || undefined,
        api_key_env: apiKeyEnv || undefined,
        egress_proxy: egressProxy || undefined,
      }),
    onSuccess: (created) => {
      onDone(created);
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add provider</DialogTitle>
        <DialogDescription>
          Providers are scoped to the current org and used as route targets.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Name">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="openai-primary"
          />
        </Field>
        <Field
          label="Slug (optional)"
          hint="URL-safe id for provider-slug/model addressing; derived from the name if blank, and immutable after create"
        >
          <Input
            value={slug}
            onChange={(e) => setSlug(e.target.value)}
            placeholder="openai-primary"
            className="font-mono"
          />
        </Field>
        <Field label="Kind">
          <Select value={kind} onChange={(e) => setKind(e.target.value)}>
            {PROVIDER_KINDS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="API base">
          <Input
            value={apiBase}
            onChange={(e) => setApiBase(e.target.value)}
            placeholder="https://api.openai.com/v1"
          />
        </Field>
        <Field label="API key (optional)" hint="sealed at rest; never displayed again">
          <Input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            autoComplete="off"
          />
        </Field>
        <Field label="API key env var (optional)" hint="read from this env var instead">
          <Input
            value={apiKeyEnv}
            onChange={(e) => setApiKeyEnv(e.target.value)}
            placeholder="OPENAI_API_KEY"
          />
        </Field>
        <Field label="Egress proxy (optional)">
          <Input
            value={egressProxy}
            onChange={(e) => setEgressProxy(e.target.value)}
            placeholder="http://proxy.internal:8080"
          />
        </Field>
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!name.trim() || !apiBase.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function EditProviderDialog({
  provider,
  onOpenChange,
  onDone,
}: {
  provider: ProviderRow | null;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const open = !!provider;

  const [kind, setKind] = React.useState<string>(PROVIDER_KINDS[0]);
  const [apiBase, setApiBase] = React.useState("");
  const [apiKey, setApiKey] = React.useState("");
  const [apiKeyEnv, setApiKeyEnv] = React.useState("");
  const [egressProxy, setEgressProxy] = React.useState("");

  React.useEffect(() => {
    if (open && provider) {
      setKind(provider.kind);
      setApiBase(provider.api_base);
      setApiKey("");
      setApiKeyEnv(provider.api_key_env ?? "");
      setEgressProxy(provider.egress_proxy ?? "");
    }
  }, [open, provider]);

  // matches the backend's tri-state semantics: omit a field to leave it
  // unchanged, send "" to clear it, send a value to set/rotate it. api_key
  // is left out entirely unless the operator typed a new one — we never
  // pre-fill it, and an empty submit here must not accidentally clear a
  // credential that's just not being rotated.
  const save = useMutation({
    mutationFn: () => {
      if (!provider) throw new Error("no provider selected");
      return updateProvider(provider.id, {
        kind: kind !== provider.kind ? kind : undefined,
        api_base: apiBase !== provider.api_base ? apiBase : undefined,
        api_key: apiKey ? apiKey : undefined,
        api_key_env: apiKeyEnv !== (provider.api_key_env ?? "") ? apiKeyEnv : undefined,
        egress_proxy:
          egressProxy !== (provider.egress_proxy ?? "") ? egressProxy : undefined,
      });
    },
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Edit {provider?.name}</DialogTitle>
        <DialogDescription>
          Leave the API key blank to keep the stored credential unchanged. Clear the
          env var or egress proxy field to unset it.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field
          label="Slug"
          hint="immutable identity for provider-slug/model addressing"
        >
          <div className="flex items-center gap-1">
            <Input value={provider?.slug ?? ""} readOnly disabled className="font-mono" />
            {provider && (
              <CopyButton value={`${provider.slug}/`} label="Copy address prefix" />
            )}
          </div>
        </Field>
        <Field label="Kind">
          <Select value={kind} onChange={(e) => setKind(e.target.value)}>
            {PROVIDER_KINDS.map((k) => (
              <option key={k} value={k}>
                {k}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="API base">
          <Input value={apiBase} onChange={(e) => setApiBase(e.target.value)} />
        </Field>
        <Field
          label="API key (optional)"
          hint="blank leaves the stored key unchanged; sealed at rest, never displayed"
        >
          <Input
            type="password"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            autoComplete="off"
            placeholder="unchanged"
          />
        </Field>
        <Field label="API key env var (optional)">
          <Input value={apiKeyEnv} onChange={(e) => setApiKeyEnv(e.target.value)} />
        </Field>
        <Field label="Egress proxy (optional)">
          <Input value={egressProxy} onChange={(e) => setEgressProxy(e.target.value)} />
        </Field>
        {save.isError && (
          <p className="text-xs text-destructive">{(save.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button disabled={!apiBase.trim() || save.isPending} onClick={() => save.mutate()}>
          Save
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
