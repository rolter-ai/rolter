import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, Pencil, Plus, Trash2, Key, Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import {
  DEFAULT_KEY_TTL_DAYS,
  KeyCacheField,
  KeyExpiryField,
  KeyModelsField,
  KeyNameField,
  KeyReachSummary,
  cacheMode,
  keyNameProblem,
  parseCacheMode,
  parseModels,
  ttlToDays,
  type CacheMode,
} from "@/components/KeyMintFields";
import {
  AttributionBadges,
  KeyAttributionFields,
  KeyProvidersField,
  UNATTRIBUTED,
  attributionId,
  attributionValue,
} from "@/components/KeyAttributionFields";

import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { CopyButton } from "@/components/CopyButton";
import { EditorSheet } from "@/components/EditorSheet";
import { EmptyState } from "@/components/ui/empty-state";
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
import { Select } from "@/components/ui/select";
import { Tag } from "@/components/ui/tag";
import {
  createVirtualKey,
  deleteVirtualKey,
  fetchBusinessUnits,
  fetchCustomers,
  fetchProviders,
  fetchVirtualKeys,
  setVirtualKeyAttribution,
  setVirtualKeyCache,
  setVirtualKeyDisabled,
  setVirtualKeyProviders,
  type BusinessUnitRow,
  type CreatedVirtualKey,
  type CustomerRow,
  type ProviderRow,
  type VirtualKeyRow,
} from "@/lib/api";
import { useGate } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useFormTelemetry, useScreenReady } from "@/lib/ux-react";

const KEYS_QUERY_KEY = ["virtual-keys"];

export default function Keys() {
  const { t } = useTranslation();
  const toast = useToast();
  // the same short date the mint sheet previews, so a row and its preview
  // cannot disagree about when the key stops working (#1182)
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const scope = useScope();
  // the row controls are the same capabilities the mint button is: editing a
  // key, flipping it off and deleting it are all an admin's (#1258)
  const updateGate = useGate("virtual_key:update");
  const deleteGate = useGate("virtual_key:delete");
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;

  const keys = useQuery({
    queryKey: [...KEYS_QUERY_KEY, scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: !!scope.projectId,
  });

  // the three org-scoped lookups the attribution editor needs. `retry: false`
  // because a member without org read access gets a 403 that will not improve
  // by asking again — the editor drops the control it cannot populate instead
  const units = useQuery({
    queryKey: ["business-units", scope.orgId],
    queryFn: () => fetchBusinessUnits(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const customers = useQuery({
    queryKey: ["customers", scope.orgId],
    queryFn: () => fetchCustomers(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: [...KEYS_QUERY_KEY, scope.projectId] });

  // the row toggles have no inline error line, so a refused change is the
  // toast's to report; a change that went through needs no announcement
  const reportFailure = (error: unknown) =>
    toast.push({
      tone: "error",
      title: t("toast.saveFailed", { what: t("errors.resources.virtualKeys") }),
      detail: errorDetail(error),
    });

  const toggleDisabled = useMutation({
    mutationFn: ({ id, disabled }: { id: string; disabled: boolean }) =>
      setVirtualKeyDisabled(id, disabled),
    onSuccess: invalidate,
    onError: reportFailure,
  });

  const setCache = useMutation({
    mutationFn: ({ id, cache }: { id: string; cache: boolean | null }) =>
      setVirtualKeyCache(id, cache),
    onSuccess: invalidate,
    onError: reportFailure,
  });

  const removeKey = useMutation({
    mutationFn: (id: string) => deleteVirtualKey(id),
    onSuccess: invalidate,
  });

  const [addOpen, setAddOpen] = React.useState(false);
  const [editTarget, setEditTarget] = React.useState<VirtualKeyRow | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<VirtualKeyRow | null>(null);
  const [created, setCreated] = React.useState<CreatedVirtualKey | null>(null);
  const [search, setSearch] = React.useState("");

  // UX stream (#805); the screen key comes from the enclosing UxScreenProvider
  useScreenReady(!keys.isLoading);
  useErrorState(!!keys.error, "virtual-key-list");
  const deleteUx = useFormTelemetry("virtual-key-delete", !!deleteTarget);

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;

  const q = search.trim().toLowerCase();
  const rows = (keys.data ?? []).filter(
    (k) => !q || (k.name ?? "").toLowerCase().includes(q) || k.key_prefix.includes(q),
  );

  // id -> display name for the two attribution dimensions, so a row and the
  // editor name the same unit rather than showing a uuid in one of them
  const unitName = (id: string | null | undefined) =>
    units.data?.find((u) => u.id === id)?.name;
  const customerName = (id: string | null | undefined) =>
    customers.data?.find((c) => c.id === id)?.name;

  const exportCsv = () => {
    const lines = [
      "name,key_prefix,models,providers,business_unit,customer,disabled,expires_at",
      ...rows.map((k) =>
        [
          k.name ?? "",
          k.key_prefix,
          k.models.join("|"),
          (k.providers ?? []).join("|"),
          unitName(k.business_unit_id) ?? "",
          customerName(k.customer_id) ?? "",
          k.disabled,
          k.expires_at ?? "",
        ].join(","),
      ),
    ];
    const blob = new Blob([lines.join("\n")], { type: "text/csv" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = "virtual-keys.csv";
    a.click();
    URL.revokeObjectURL(a.href);
  };

  const GRID = "1.2fr 1fr 1.4fr 1.4fr 1.1fr 60px 68px";

  return (
    <PageBody>
      {/* "api key" names three different credentials in this product; the
          screen says which one it mints (#943) */}
      <p className="text-sm text-muted-foreground">
        {t("pages.virtualKeys.explainer")}
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <SearchInput
          placeholder="Search by name…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className="ml-auto flex items-center gap-2">
          <Button variant="outline" onClick={exportCsv}>
            Export CSV
          </Button>
          <GatedButton gate="virtual_key:create" onClick={() => setAddOpen(true)} disabled={scopeBlocked || !scope.projectId}>
            <Plus className="h-4 w-4" />
            Add Virtual Key
          </GatedButton>
        </div>
      </div>

      {keys.error && (
        <LoadError
          error={keys.error}
          resource={t("errors.resources.virtualKeys")}
          onRetry={() => keys.refetch()}
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scopeMessage}. Read-only view still works.
        </p>
      )}

      <ListTable>
        <ListHeader grid={GRID}>
          <span>{t("pages.virtualKeys.colName")}</span>
          <span>{t("pages.virtualKeys.colKey")}</span>
          <span>{t("pages.virtualKeys.colModels")}</span>
          {/* attribution sits next to the models allow-list because both
              answer "what does this key touch", and neither is the secret */}
          <span>{t("pages.virtualKeys.colAttribution")}</span>
          <span>{t("pages.virtualKeys.colCache")}</span>
          <span>{t("pages.virtualKeys.colStatus")}</span>
          <span />
        </ListHeader>
        {keys.isLoading && <ListSkeleton rows={4} className="p-3" />}
        {rows.map((key) => (
          <ListRow
            key={key.id}
            grid={GRID}
            // a disabled key reads as a quieter band, not as faded text:
            // container opacity fades the glyphs toward the page background
            // and takes every one of them under 4.5:1 (#1181)
            className={key.disabled ? "bg-[color:var(--surface-subtle)]/60" : undefined}
          >
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold">{key.name ?? "unnamed key"}</div>
              <div className="truncate text-[0.6875rem] text-muted-foreground">
                {key.expires_at
                  ? t("pages.virtualKeys.expiresOn", { date: fmt.date(key.expires_at) })
                  : t("pages.virtualKeys.noExpiry")}
              </div>
            </div>
            <div className="flex min-w-0 items-center gap-0.5">
              <code className="min-w-0 flex-1 truncate font-mono text-xs text-[color:var(--text-secondary)]">
                {key.key_prefix}…
              </code>
              <CopyButton value={key.key_prefix} label="Copy key prefix" className="h-6 px-1" />
            </div>
            <div className="flex min-w-0 flex-wrap gap-1 overflow-hidden">
              {key.models.length ? (
                key.models.slice(0, 3).map((model) => <Tag key={model}>{model}</Tag>)
              ) : (
                <Badge tone="neutral">all models</Badge>
              )}
              {key.models.length > 3 && (
                <span className="font-mono text-[10px] text-[color:var(--text-subtle)]">
                  +{key.models.length - 3}
                </span>
              )}
            </div>
            <AttributionBadges
              unit={unitName(key.business_unit_id)}
              customer={customerName(key.customer_id)}
            />
            <Select
              aria-label={t("pages.virtualKeys.cacheAria", {
                name: key.name ?? key.key_prefix,
              })}
              title={updateGate.reason}
              className="h-8 text-xs"
              value={cacheMode(key.cache_enabled)}
              disabled={setCache.isPending || updateGate.denied}
              onChange={(event) =>
                setCache.mutate({ id: key.id, cache: parseCacheMode(event.target.value) })
              }
            >
              <option value="inherit">inherit</option>
              <option value="off">off</option>
              <option value="on">on</option>
            </Select>
            <GatedSwitch
              gate="virtual_key:update"
              checked={!key.disabled}
              disabled={toggleDisabled.isPending}
              aria-label={t("pages.virtualKeys.toggleAria", {
                name: key.name ?? key.key_prefix,
              })}
              onCheckedChange={(enabled) =>
                toggleDisabled.mutate({ id: key.id, disabled: !enabled })
              }
            />
            <div className="flex items-center justify-self-end">
              <button
                type="button"
                title={updateGate.reason ?? t("pages.virtualKeys.edit")}
                aria-label={t("pages.virtualKeys.editKey", {
                  name: key.name ?? key.key_prefix,
                })}
                disabled={scopeBlocked || updateGate.denied}
                onClick={() => setEditTarget(key)}
                className="flex rounded-[6px] p-1 text-[color:var(--text-subtle)] transition-colors hover:bg-[color:var(--surface-hover)] hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Pencil className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                title={
                  deleteGate.reason ??
                  t("pages.virtualKeys.deleteKey", {
                    name: key.name ?? key.key_prefix,
                  })
                }
                aria-label={t("pages.virtualKeys.deleteKey", {
                  name: key.name ?? key.key_prefix,
                })}
                disabled={deleteGate.denied}
                onClick={() => setDeleteTarget(key)}
                className="flex rounded-[6px] p-1 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </ListRow>
        ))}
        {!keys.isLoading && rows.length === 0 && (
          <EmptyState
            uxTarget="virtual-keys"
            icon={<Key />}
            title={search ? t("pages.virtualKeys.noMatchTitle") : t("pages.virtualKeys.emptyTitle")}
            description={
              search ? t("pages.virtualKeys.noMatchBody") : t("pages.virtualKeys.emptyBody")
            }
            actions={
              search ? (
                <Button variant="outline" onClick={() => setSearch("")}>
                  {t("common.clearSearch")}
                </Button>
              ) : (
                <GatedButton
                  gate="virtual_key:create"
                  disabled={scopeBlocked || !scope.projectId}
                  onClick={() => setAddOpen(true)}
                >
                  {t("pages.virtualKeys.emptyAction")}
                </GatedButton>
              )
            }
          />
        )}
      </ListTable>
      <div className="flex items-center justify-between px-0.5 text-xs text-muted-foreground">
        <span>
          {rows.length} of {keys.data?.length ?? 0} keys
        </span>
      </div>

      {scope.projectId && (
        <AddKeyDialog
          open={addOpen}
          onOpenChange={setAddOpen}
          projectId={scope.projectId}
          units={units.data ?? []}
          customers={customers.data ?? []}
          providers={providers.data ?? []}
          onCreated={(key) => {
            invalidate();
            setCreated(key);
          }}
        />
      )}

      <EditKeyDialog
        target={editTarget}
        onOpenChange={(open) => !open && setEditTarget(null)}
        units={units.data ?? []}
        customers={customers.data ?? []}
        providers={providers.data ?? []}
        onSaved={() => {
          invalidate();
          setEditTarget(null);
        }}
      />

      <Dialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete virtual key</DialogTitle>
          <DialogDescription>
            <span className="font-mono">{deleteTarget?.key_prefix}…</span> will stop
            authenticating immediately. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        {removeKey.isError && (
          <p className="text-xs text-[color:var(--status-danger-text)]">
            {(removeKey.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removeKey.isPending}
            onClick={() => {

              if (!deleteTarget) return;
              deleteUx.submitted();
              const name = deleteTarget.name;
              removeKey.mutate(deleteTarget.id, {
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
            {removeKey.isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Delete
          </Button>
        </DialogFooter>
      </Dialog>

      <CreatedKeyDialog created={created} onOpenChange={(open) => !open && setCreated(null)} />
    </PageBody>
  );
}

function AddKeyDialog({
  open,
  onOpenChange,
  projectId,
  units,
  customers,
  providers,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  units: BusinessUnitRow[];
  customers: CustomerRow[];
  providers: ProviderRow[];
  onCreated: (key: CreatedVirtualKey) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = React.useState("");
  const [modelsText, setModelsText] = React.useState("");
  const [cache, setCache] = React.useState<CacheMode>("inherit");
  const [ttl, setTtl] = React.useState(String(DEFAULT_KEY_TTL_DAYS));
  const [providerSel, setProviderSel] = React.useState<string[]>([]);
  const [unitId, setUnitId] = React.useState(UNATTRIBUTED);
  const [customerId, setCustomerId] = React.useState(UNATTRIBUTED);

  // names the form, never its contents — this dialog mints a credential
  const ux = useFormTelemetry("virtual-key-create", open);

  React.useEffect(() => {
    if (open) {
      setName("");
      setModelsText("");
      setCache("inherit");
      setTtl(String(DEFAULT_KEY_TTL_DAYS));
      setProviderSel([]);
      setUnitId(UNATTRIBUTED);
      setCustomerId(UNATTRIBUTED);
    }
  }, [open]);

  const models = React.useMemo(() => parseModels(modelsText), [modelsText]);

  const create = useMutation({
    // POST /virtual-keys carries the provider allow-list but not the
    // attribution, so a key that was given one is pointed at it immediately
    // afterwards through the endpoint that owns that decision
    mutationFn: async () => {
      const created = await createVirtualKey(projectId, {
        name: name.trim(),
        models,
        providers: providerSel,
        cache: parseCacheMode(cache),
        expires_in_days: ttlToDays(ttl),
      });
      if (unitId !== UNATTRIBUTED || customerId !== UNATTRIBUTED) {
        await setVirtualKeyAttribution(created.id, {
          business_unit_id: attributionId(unitId),
          customer_id: attributionId(customerId),
        });
      }
      return created;
    },
    onSuccess: (key) => {
      ux.saved();
      onOpenChange(false);
      onCreated(key);
    },
    onError: () => ux.failed(),
  });

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Create virtual key"
      subtitle="The plaintext key is shown once, right after creation — copy it then"
      dirty={
        Boolean(name || modelsText || providerSel.length) ||
        cache !== "inherit" ||
        unitId !== UNATTRIBUTED ||
        customerId !== UNATTRIBUTED
      }
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      // the sheet footer has no room for a spinner, so pending state reads
      // from the label instead
      saveLabel={create.isPending ? "Creating…" : "Create"}
      canSave={keyNameProblem(name) === null}
      saving={create.isPending}
      onSave={() => {
        ux.submitted();
        create.mutate();
      }}
    >
      <div className="space-y-3">
        <KeyNameField value={name} onChange={setName} />
        <KeyExpiryField value={ttl} onChange={setTtl} />
        <KeyCacheField value={cache} onChange={setCache} />
        <KeyModelsField value={modelsText} onChange={setModelsText} />
        <KeyProvidersField
          providers={providers}
          selected={providerSel}
          onChange={setProviderSel}
        />
        <KeyAttributionFields
          units={units}
          customers={customers}
          businessUnitId={unitId}
          customerId={customerId}
          onChange={(unit, customer) => {
            setUnitId(unit);
            setCustomerId(customer);
          }}
        />
        <KeyReachSummary
          project={t("keyMint.reach.thisProject")}
          models={models}
          providers={providerSel}
          ttl={ttl}
        />
      </div>
    </EditorSheet>
  );
}

/**
 * Edit what an existing key reaches and who pays for it.
 *
 * Deliberately not a general key editor: the control plane has no rename and no
 * re-scope, so the sheet offers exactly the two things it can actually change —
 * the upstream allow-list and the attribution — and each goes to its own
 * endpoint. Only the dimension that moved is sent, so re-saving an untouched
 * sheet writes nothing and audits nothing.
 */
function EditKeyDialog({
  target,
  onOpenChange,
  units,
  customers,
  providers,
  onSaved,
}: {
  target: VirtualKeyRow | null;
  onOpenChange: (open: boolean) => void;
  units: BusinessUnitRow[];
  customers: CustomerRow[];
  providers: ProviderRow[];
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [providerSel, setProviderSel] = React.useState<string[]>([]);
  const [unitId, setUnitId] = React.useState(UNATTRIBUTED);
  const [customerId, setCustomerId] = React.useState(UNATTRIBUTED);
  const ux = useFormTelemetry("virtual-key-attribution", !!target);

  // seeded from the row every time the sheet opens on a different key, so a
  // draft abandoned on one key cannot leak into the next one
  React.useEffect(() => {
    if (!target) return;
    setProviderSel(target.providers ?? []);
    setUnitId(attributionValue(target.business_unit_id));
    setCustomerId(attributionValue(target.customer_id));
  }, [target]);

  const name = target?.name ?? target?.key_prefix ?? "";
  const sorted = (list: string[]) => [...list].sort().join("|");
  const providersChanged = sorted(providerSel) !== sorted(target?.providers ?? []);
  const attributionChanged =
    unitId !== attributionValue(target?.business_unit_id) ||
    customerId !== attributionValue(target?.customer_id);

  const save = useMutation({
    mutationFn: async () => {
      if (!target) return;
      if (providersChanged) await setVirtualKeyProviders(target.id, providerSel);
      if (attributionChanged) {
        await setVirtualKeyAttribution(target.id, {
          business_unit_id: attributionId(unitId),
          customer_id: attributionId(customerId),
        });
      }
    },
    onSuccess: () => {
      ux.saved();
      // the sheet closes on success, taking any inline confirmation with it,
      // so the outcome is announced where it survives that (#1197)
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: name }),
      });
      onSaved();
    },
    onError: (error) => {
      ux.failed();
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      open={!!target}
      onOpenChange={onOpenChange}
      title={t("pages.virtualKeys.editTitle")}
      subtitle={name}
      dirty={providersChanged || attributionChanged}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={save.isPending ? t("pages.virtualKeys.saving") : t("pages.virtualKeys.save")}
      canSave={providersChanged || attributionChanged}
      saving={save.isPending}
      onSave={() => {
        ux.submitted();
        save.mutate();
      }}
    >
      <div className="space-y-3">
        <p className="text-xs text-muted-foreground">
          {t("pages.virtualKeys.editSubtitleHint")}
        </p>
        <KeyProvidersField
          providers={providers}
          selected={providerSel}
          onChange={setProviderSel}
        />
        <KeyAttributionFields
          units={units}
          customers={customers}
          businessUnitId={unitId}
          customerId={customerId}
          onChange={(unit, customer) => {
            setUnitId(unit);
            setCustomerId(customer);
          }}
        />
      </div>
    </EditorSheet>
  );
}

// shows the plaintext secret exactly once, right after creation; state is
// local to this dialog and is discarded on close, never re-fetchable
function CreatedKeyDialog({
  created,
  onOpenChange,
}: {
  created: CreatedVirtualKey | null;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    if (created) setCopied(false);
  }, [created]);

  const copy = async () => {
    if (!created) return;
    try {
      await navigator.clipboard.writeText(created.key);
      setCopied(true);
    } catch {
      // clipboard unavailable — user can still select/copy the text manually
    }
  };

  return (
    <Dialog open={!!created} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Key created</DialogTitle>
        <DialogDescription>
          This is the only time the plaintext key is shown. Copy it now — it can't be
          retrieved again.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-2 rounded-md border border-dashed border-border bg-muted p-3">
        <div className="flex items-center justify-between gap-2">
          <code className="break-all text-sm">{created?.key}</code>
          <Button
            size="sm"
            variant="outline"
            onClick={copy}
            aria-label={copied ? t("common.copied") : t("common.copy")}
            title={copied ? t("common.copied") : t("common.copy")}
          >
            {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
          </Button>
        </div>
      </div>
      <DialogFooter>
        <Button onClick={() => onOpenChange(false)}>Done</Button>
      </DialogFooter>
    </Dialog>
  );
}
