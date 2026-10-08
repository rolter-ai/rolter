import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus, Key } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

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
  ttlToDays,
  useRouteModels,
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
import { GatedCombobox } from "@/components/GatedCombobox";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { GatedSwitch } from "@/components/GatedSwitch";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { CopyButton } from "@/components/CopyButton";
import { DocsLink } from "@/components/DocsLink";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { KeyNextStep } from "@/components/KeyNextStep";
import { EmptyState } from "@/components/ui/empty-state";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListSummary,
  ListTable,
  PageBody,
  RowIconButton,
  SearchInput,
  primaryColumn,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { SecretRevealDialog } from "@/components/ui/secret-reveal";
import { Tag } from "@/components/ui/tag";
import {
  PLAYGROUND_PURPOSE,
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
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { describeError } from "@/lib/error-copy";
import { errorDetail, useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

const KEYS_QUERY_KEY = ["virtual-keys"];

export default function Keys() {
  const { t } = useTranslation();
  const toast = useToast();
  // the same short date the mint sheet previews, so a row and its preview
  // cannot disagree about when the key stops working (#1182)
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const scope = useScope();
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

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;

  const q = search.trim().toLowerCase();
  const rows = (keys.data ?? []).filter(
    (k) => !q || (k.name ?? "").toLowerCase().includes(q) || k.key_prefix.includes(q),
  );

  // id -> display name for the two attribution dimensions, so a row and the
  // editor name the same unit rather than showing a uuid in one of them
  const unitName = (id: string | null | undefined) => units.data?.find((u) => u.id === id)?.name;
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

  const GRID = `${primaryColumn(1.2)} 1fr 1.4fr 1.4fr 1.1fr 60px 68px`;

  return (
    <PageBody>
      {/* "api key" names three different credentials in this product; the
          screen says which one it mints (#943) */}
      <p className="text-sm text-muted-foreground">
        {t("pages.virtualKeys.explainer")}{" "}
        {/* the explainer stands alone; the link only adds depth, and is absent
            on a deployment that configured no documentation host (#1164) */}
        <DocsLink page="whichKey" label={t("docs.link.whichKey")} />
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <SearchInput
          placeholder={t("pages.virtualKeys.searchPlaceholder")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <div className="ml-auto flex flex-wrap items-center gap-2">
          {/* a list that has not loaded exports a header line and no rows,
              which reads as a project with no keys (#2056) */}
          <Button variant="outline" disabled={!keys.isSuccess} onClick={exportCsv}>
            {t("pages.virtualKeys.exportCsv")}
          </Button>
          <GatedButton
            gate="virtual_key:create"
            control="key-new"
            onClick={() => setAddOpen(true)}
            disabled={scopeBlocked || !scope.projectId}
          >
            <Plus className="h-4 w-4" />
            {t("pages.virtualKeys.add")}
          </GatedButton>
        </div>
      </div>

      {keys.error && (
        <LoadError
          error={keys.error}
          resource={t("errors.resources.virtualKeys")}
          onRetry={() => keys.refetch()}
          target="virtual-keys"
        />
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("common.scopeReadOnly", { reason: scopeMessage })}
        </p>
      )}

      {/* as on Providers, the table's floor carries the name column's (#2812) */}
      <ListTable label={t("screens.virtual-keys.title")} minWidth={840}>
        <ListHeader grid={GRID}>
          <ListHeaderCell>{t("pages.virtualKeys.colName")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.virtualKeys.colKey")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.virtualKeys.colModels")}</ListHeaderCell>
          {/* attribution sits next to the models allow-list because both
              answer "what does this key touch", and neither is the secret */}
          <ListHeaderCell>{t("pages.virtualKeys.colAttribution")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.virtualKeys.colCache")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.virtualKeys.colStatus")}</ListHeaderCell>
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={keys}>
          <ListSkeleton rows={4} className="p-3" />
        </ListLoadingRow>
        {rows.map((key) => (
          <ListRow
            key={key.id}
            grid={GRID}
            // a disabled key reads as a quieter band, not as faded text:
            // container opacity fades the glyphs toward the page background
            // and takes every one of them under 4.5:1 (#1181)
            className={key.disabled ? "bg-[color:var(--surface-subtle)]/60" : undefined}
          >
            <ListCell className="min-w-0">
              {/* the badge wraps under the name rather than taking the name's
                  room: the name is what the row is found by (#2812) */}
              <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1">
                <span className="truncate text-sm font-semibold">
                  {key.name ?? t("pages.virtualKeys.unnamed")}
                </span>
                {/* a key the Playground minted for itself, not one an operator
                    created: it is scoped by the server and expires on its own,
                    which the row says rather than leaving a reader to infer it
                    from a short expiry (#944) */}
                {key.purpose === PLAYGROUND_PURPOSE && (
                  <Badge tone="info" title={t("pages.virtualKeys.playgroundHint")}>
                    {t("pages.virtualKeys.playground")}
                  </Badge>
                )}
              </div>
              <div className="truncate text-[0.6875rem] text-muted-foreground">
                {key.expires_at
                  ? t("pages.virtualKeys.expiresOn", { date: fmt.date(key.expires_at) })
                  : t("pages.virtualKeys.noExpiry")}
              </div>
            </ListCell>
            <ListCell className="flex min-w-0 items-center gap-0.5">
              <code className="min-w-0 flex-1 truncate font-mono text-xs text-[color:var(--text-secondary)]">
                {key.key_prefix}…
              </code>
              <CopyButton
                value={key.key_prefix}
                label={t("pages.virtualKeys.copyPrefix")}
                className="h-6 px-1"
              />
            </ListCell>
            <ListCell className="flex min-w-0 flex-wrap gap-1 overflow-hidden">
              {key.models.length ? (
                key.models.slice(0, 3).map((model) => <Tag key={model}>{model}</Tag>)
              ) : (
                <Badge tone="neutral">{t("pages.virtualKeys.allModels")}</Badge>
              )}
              {key.models.length > 3 && (
                <span className="font-mono text-[10px] text-[color:var(--text-subtle)]">
                  +{key.models.length - 3}
                </span>
              )}
            </ListCell>
            <ListCell className="grid min-w-0">
              <AttributionBadges
                unit={unitName(key.business_unit_id)}
                customer={customerName(key.customer_id)}
              />
            </ListCell>
            <ListCell className="grid">
              <GatedCombobox
                gate="virtual_key:update"
                control="key-cache"
                aria-label={t("pages.virtualKeys.cacheAria", {
                  name: key.name ?? key.key_prefix,
                })}
                className="h-8 text-xs"
                value={cacheMode(key.cache_enabled)}
                disabled={setCache.isPending}
                onChange={(picked) =>
                  setCache.mutate({ id: key.id, cache: parseCacheMode(picked) })
                }
                options={[
                  { value: "inherit", label: t("pages.virtualKeys.cacheModes.inherit") },
                  { value: "off", label: t("pages.virtualKeys.cacheModes.off") },
                  { value: "on", label: t("pages.virtualKeys.cacheModes.on") },
                ]}
              />
            </ListCell>
            <ListCell className="grid">
              <GatedSwitch
                gate="virtual_key:update"
                control="key-toggle"
                checked={!key.disabled}
                disabled={toggleDisabled.isPending}
                aria-label={t("pages.virtualKeys.toggleAria", {
                  name: key.name ?? key.key_prefix,
                })}
                onCheckedChange={(enabled) =>
                  toggleDisabled.mutate({ id: key.id, disabled: !enabled })
                }
              />
            </ListCell>
            <ListCell className="flex items-center justify-self-end">
              <RowIconButton
                gate="virtual_key:update"
                control="key-edit"
                title={t("pages.virtualKeys.edit")}
                aria-label={t("pages.virtualKeys.editKey", {
                  name: key.name ?? key.key_prefix,
                })}
                disabled={scopeBlocked}
                onClick={() => setEditTarget(key)}
              >
                <Pencil className="h-3.5 w-3.5" />
              </RowIconButton>
              <DeleteIconButton
                gate="virtual_key:delete"
                control="key-delete"
                label={t("pages.virtualKeys.deleteKey", {
                  name: key.name ?? key.key_prefix,
                })}
                onClick={() => setDeleteTarget(key)}
              />
            </ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={keys} rows={rows.length}>
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
                  control="key-new-empty"
                  disabled={scopeBlocked || !scope.projectId}
                  onClick={() => setAddOpen(true)}
                >
                  {t("pages.virtualKeys.emptyAction")}
                </GatedButton>
              )
            }
          />
        </ListEmptyRow>
      </ListTable>
      <div className="flex items-center justify-between px-0.5 text-xs text-muted-foreground">
        <ListSummary data={keys.data} className="text-xs">
          {(all) => t("pages.virtualKeys.shownOf", { shown: rows.length, count: all.length })}
        </ListSummary>
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

      <ConfirmDialog
        // stable key for the UX stream, the same one the hand-rolled dialog
        // emitted under so the series stays continuous (#1738)
        name="virtual-key-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDeleteTarget(null);
          // a failure from this row must not greet the next one opened
          removeKey.reset();
        }}
        title={t("pages.virtualKeys.confirm.deleteTitle", {
          name: deleteTarget?.name ?? deleteTarget?.key_prefix ?? "",
        })}
        description={
          <Trans
            i18nKey="pages.virtualKeys.confirm.deleteBody"
            values={{ prefix: `${deleteTarget?.key_prefix}…` }}
            components={[<span key="prefix" className="font-mono" />]}
          />
        }
        confirmLabel={t("pages.virtualKeys.confirm.deleteConfirm")}
        pending={removeKey.isPending}
        error={removeKey.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const name = deleteTarget.name;
          removeKey.mutate(deleteTarget.id, {
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

      {/* the plaintext is shown once; `created` is dropped on close, so it is
          never re-fetchable */}
      <SecretRevealDialog
        name="virtual-key-created"
        open={!!created}
        onOpenChange={(open) => !open && setCreated(null)}
        title={t("pages.virtualKeys.createdTitle")}
        description={t("pages.virtualKeys.createdBody")}
        secret={created?.key ?? ""}
        copyLabel={t("common.copy")}
        size="lg"
      >
        <KeyNextStep models={created?.models ?? []} />
      </SecretRevealDialog>
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
  const [models, setModels] = React.useState<string[]>([]);
  const [cache, setCache] = React.useState<CacheMode>("inherit");
  const [ttl, setTtl] = React.useState(String(DEFAULT_KEY_TTL_DAYS));
  const [providerSel, setProviderSel] = React.useState<string[]>([]);
  const [unitId, setUnitId] = React.useState(UNATTRIBUTED);
  const [customerId, setCustomerId] = React.useState(UNATTRIBUTED);
  // the models this project routes, offered as ticks; only asked for while the
  // sheet is open, since a closed sheet has nothing to populate
  const routes = useRouteModels(projectId, open);

  React.useEffect(() => {
    if (open) {
      setName("");
      setModels([]);
      setCache("inherit");
      setTtl(String(DEFAULT_KEY_TTL_DAYS));
      setProviderSel([]);
      setUnitId(UNATTRIBUTED);
      setCustomerId(UNATTRIBUTED);
    }
  }, [open]);

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
      onOpenChange(false);
      onCreated(key);
    },
  });

  return (
    <EditorSheet
      name="virtual-key-create"
      open={open}
      onOpenChange={onOpenChange}
      title={t("pages.virtualKeys.createTitle")}
      subtitle={t("pages.virtualKeys.createSubtitle")}
      dirty={
        Boolean(name || models.length || providerSel.length) ||
        cache !== "inherit" ||
        unitId !== UNATTRIBUTED ||
        customerId !== UNATTRIBUTED
      }
      errorMessage={create.isError ? describeError(create.error, t).message : undefined}
      errorDetail={create.isError ? describeError(create.error, t).detail : undefined}
      // the sheet footer has no room for a spinner, so pending state reads
      // from the label instead
      saveLabel={create.isPending ? t("pages.virtualKeys.creating") : t("common.create")}
      canSave={keyNameProblem(name) === null}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <KeyNameField value={name} onChange={setName} />
        <KeyExpiryField value={ttl} onChange={setTtl} />
        <KeyCacheField value={cache} onChange={setCache} />
        <KeyModelsField
          value={models}
          onChange={setModels}
          options={routes.models}
          loading={routes.loading}
          error={routes.error}
          onRetry={routes.retry}
        />
        <KeyProvidersField providers={providers} selected={providerSel} onChange={setProviderSel} />
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
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: name }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      name="virtual-key-attribution"
      open={!!target}
      onOpenChange={onOpenChange}
      title={t("pages.virtualKeys.editTitle")}
      subtitle={name}
      dirty={providersChanged || attributionChanged}
      errorMessage={save.isError ? describeError(save.error, t).message : undefined}
      errorDetail={save.isError ? describeError(save.error, t).detail : undefined}
      saveLabel={save.isPending ? t("pages.virtualKeys.saving") : t("pages.virtualKeys.save")}
      canSave={providersChanged || attributionChanged}
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
      <div className="space-y-3">
        <p className="text-xs text-muted-foreground">{t("pages.virtualKeys.editSubtitleHint")}</p>
        <KeyProvidersField providers={providers} selected={providerSel} onChange={setProviderSel} />
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
