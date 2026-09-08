import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Building2, WalletCards } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import {
  AnalyticsUnavailableError,
  createBusinessUnit,
  createCustomer,
  deleteBusinessUnit,
  deleteCustomer,
  fetchAttributionSpend,
  fetchBusinessUnits,
  fetchCustomers,
  updateBusinessUnit,
  updateCustomer,
  type AttributionSpendRow,
  type BusinessUnitRow,
  type CustomerRow,
} from "@/lib/api";
import type { Capability } from "@/lib/can";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { errorDetail, useToast } from "@/lib/toast";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// the window the spend column reports, matching the Dashboard's: one figure on
// two screens has to mean the same thing, or the totals disagree for a reason
// nobody can see
const SPEND_WINDOW = { since: new Date(Date.now() - 86_400_000).toISOString() };

const num = (v: number | string | undefined): number => Number(v ?? 0);

/** the unattributed bucket comes back with an empty id, not a sentinel uuid */
const UNATTRIBUTED_ID = "";

// the server's slug rule, mirrored so a bad value is caught before the round
// trip. slugs are the stable attribution identity, not a display name
const SLUG = /^[a-z0-9][a-z0-9-]{0,62}$/;

const UNASSIGNED = "__none__";

function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);
}

// a retired unit or customer keeps its history and stops being offered for new
// attribution; deleting one removes it outright
function RetiredBadge({ retiredAt }: { retiredAt: string | null }) {
  if (!retiredAt) return null;
  return <Badge tone="neutral">RETIRED</Badge>;
}

interface EditorState {
  id: string | null;
  name: string;
  slug: string;
  allowSlugChange: boolean;
  businessUnitId: string;
}

const blank = (): EditorState => ({
  id: null,
  name: "",
  slug: "",
  allowSlugChange: false,
  businessUnitId: UNASSIGNED,
});

function slugErrorFor(form: EditorState, original: string | null): string | undefined {
  const slug = form.slug.trim() || slugify(form.name);
  if (slug && !SLUG.test(slug)) {
    return "Slug must be lowercase alphanumerics and dashes, starting with a letter or digit.";
  }
  // the server refuses a silent rename because attribution already recorded
  // against the old slug does not follow it
  if (original && slug !== original && !form.allowSlugChange) {
    return "Renaming the slug breaks attribution recorded against the old one. Confirm below to allow it.";
  }
  return undefined;
}

function Editor({
  kind,
  form,
  setForm,
  original,
  units,
  onClose,
  onSubmit,
  pending,
  error,
}: {
  kind: "unit" | "customer";
  form: EditorState;
  setForm: (f: EditorState) => void;
  original: string | null;
  units: BusinessUnitRow[];
  onClose: () => void;
  onSubmit: () => void;
  pending: boolean;
  error?: string;
}) {
  const slugError = slugErrorFor(form, original);
  const noun = kind === "unit" ? "business unit" : "customer";
  const effectiveSlug = form.slug.trim() || slugify(form.name);
  return (
    <>
      <SheetHeader
        title={form.id ? `Edit ${noun}` : `New ${noun}`}
        subtitle={effectiveSlug || "—"}
        onClose={onClose}
      />
      <SheetBody>
        <Field label="Name">
          <Input
            aria-label="Name"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder={kind === "unit" ? "Platform Engineering" : "Acme Corp"}
          />
        </Field>
        <Field
          label="Slug"
          hint="Derived from the name when left blank. This is the stable identity spend is attributed by."
          error={slugError}
        >
          <Input
            aria-label="Slug"
            className="font-mono text-xs"
            value={form.slug}
            onChange={(e) => setForm({ ...form, slug: e.target.value })}
            placeholder={slugify(form.name) || "acme-corp"}
          />
        </Field>
        {original && effectiveSlug !== original && (
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <input
              type="checkbox"
              aria-label="Allow slug change"
              checked={form.allowSlugChange}
              onChange={(e) =>
                setForm({ ...form, allowSlugChange: e.target.checked })
              }
            />
            Rename the slug from{" "}
            <span className="font-mono text-foreground">{original}</span> anyway
          </label>
        )}
        {kind === "customer" && (
          <Field
            label="Business unit"
            hint="Roll this customer's spend up into a business unit, or leave it unassigned."
          >
            <Select
              aria-label="Business unit"
              value={form.businessUnitId}
              onChange={(e) =>
                setForm({ ...form, businessUnitId: e.target.value })
              }
            >
              <option value={UNASSIGNED}>Unassigned</option>
              {units
                .filter((u) => !u.retired_at || u.id === form.businessUnitId)
                .map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.name}
                  </option>
                ))}
            </Select>
          </Field>
        )}
        {error && <p className="text-xs text-[color:var(--status-danger-text)]">{error}</p>}
      </SheetBody>
      <SheetFooter>
        <div className="flex justify-end gap-2 px-[22px] py-3.5">
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!form.name.trim() || !!slugError || pending}
            onClick={onSubmit}
          >
            {form.id ? "Save" : "Create"}
          </Button>
        </div>
      </SheetFooter>
    </>
  );
}

/**
 * What the window cost, and how much of it nobody claimed.
 *
 * The unattributed figure is given the same weight as the total on purpose: a
 * chargeback report that lists five units and quietly omits a sixth of the
 * spend is worse than one that admits the hole, and the hole is the number an
 * operator acts on.
 */
function SpendStrip({
  rows,
  loading,
  error,
  onRetry,
}: {
  rows: AttributionSpendRow[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();

  // the calm not-configured note the Dashboard shows, in the one strip that
  // needs ClickHouse — the governance list itself is postgres-backed and keeps
  // working without it
  if (error instanceof AnalyticsUnavailableError) {
    return (
      <div className="rounded-[10px] border border-[color:var(--border-default)]">
        <EmptyState
          uxTarget="cost-attribution-spend"
          title={t("pages.dashboard.notConfiguredTitle")}
          description={t("pages.dashboard.notConfiguredBody")}
        />
      </div>
    );
  }
  if (error) {
    return (
      <LoadError
        error={error}
        resource={t("errors.resources.attributionSpend")}
        onRetry={onRetry}
      />
    );
  }
  if (loading) return <Skeleton height={72} radius={10} />;

  const total = rows.reduce((sum, r) => sum + num(r.cost_usd), 0);
  const unattributed = rows
    .filter((r) => r.id === UNATTRIBUTED_ID)
    .reduce((sum, r) => sum + num(r.cost_usd), 0);
  const share = total > 0 ? unattributed / total : 0;

  return (
    <div className="flex flex-wrap items-end gap-x-10 gap-y-3 rounded-[10px] border border-[color:var(--border-default)] bg-card px-4 py-3">
      <SpendFigure
        label={t("pages.costAttribution.spendWindow")}
        value={fmt.currency(total, currency)}
      />
      <SpendFigure
        label={t("pages.costAttribution.spendAttributed")}
        value={fmt.currency(total - unattributed, currency)}
      />
      <SpendFigure
        label={t("pages.costAttribution.spendUnattributed")}
        value={fmt.currency(unattributed, currency)}
        note={
          unattributed > 0
            ? t("pages.costAttribution.spendUnattributedShare", {
                share: fmt.percent(share, 1),
              })
            : undefined
        }
      />
    </div>
  );
}

function SpendFigure({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note?: string;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[0.6875rem] uppercase tracking-[0.07em] text-muted-foreground">
        {label}
      </span>
      <span className="font-mono text-lg leading-none text-foreground">{value}</span>
      {note && (
        <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">{note}</span>
      )}
    </div>
  );
}

/** the spend line on one unit's or customer's card */
function CardSpend({ row }: { row?: AttributionSpendRow }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  if (!row) {
    return (
      <div className="font-mono text-xs text-[color:var(--text-subtle)]">
        {t("pages.costAttribution.noSpend")}
      </div>
    );
  }
  return (
    <div className="flex items-baseline gap-2">
      <span className="font-mono text-base leading-none text-foreground">
        {fmt.currency(num(row.cost_usd), currency)}
      </span>
      <span className="text-xs text-muted-foreground">
        {t("pages.costAttribution.cardRequests", { count: num(row.requests) })}
      </span>
    </div>
  );
}

// shared list shell: both screens are the same CRUD over an org-scoped
// collection, differing only in the business-unit assignment column
function AttributionScreen<T extends BusinessUnitRow | CustomerRow>({
  kind,
  rows,
  units,
  isLoading,
  isError,
  error,
  onRetry,
  onCreate,
  onUpdate,
  onRetire,
  onDelete,
  deleting,
  deleteError,
  resetDelete,
  mutating,
  mutationError,
  disabled,
  spend,
  spendLoading,
  spendError,
  onRetrySpend,
}: {
  kind: "unit" | "customer";
  rows: T[];
  units: BusinessUnitRow[];
  isLoading: boolean;
  isError: boolean;
  error?: Error;
  onRetry: () => void;
  onCreate: (form: EditorState) => void;
  onUpdate: (form: EditorState, original: T) => void;
  onRetire: (row: T, retired: boolean) => void;
  // the caller owns the mutation, so it is the caller that knows the delete
  // succeeded — it closes the confirmation through `onSuccess` (#1179)
  onDelete: (row: T, onSuccess: () => void) => void;
  deleting: boolean;
  deleteError?: unknown;
  resetDelete: () => void;
  mutating: boolean;
  mutationError?: Error;
  disabled: boolean;
  /** window spend keyed by the dimension's own id; the empty id is the
   *  unattributed bucket, which has no card of its own */
  spend: AttributionSpendRow[];
  spendLoading: boolean;
  spendError: unknown;
  onRetrySpend: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(false);
  const [form, setForm] = React.useState<EditorState>(blank);
  const [editing, setEditing] = React.useState<T | null>(null);
  // retiring is the reversible option and deleting is not, so the two must not
  // sit one indistinguishable click apart
  const [deleteTarget, setDeleteTarget] = React.useState<T | null>(null);
  const startDelete = (row: T) => {
    resetDelete();
    setDeleteTarget(row);
  };

  const startCreate = () => {
    setEditing(null);
    setForm(blank());
    setOpen(true);
  };
  const startEdit = (row: T) => {
    setEditing(row);
    setForm({
      id: row.id,
      name: row.name,
      slug: row.slug,
      allowSlugChange: false,
      businessUnitId:
        "business_unit_id" in row && row.business_unit_id
          ? row.business_unit_id
          : UNASSIGNED,
    });
    setOpen(true);
  };

  const noun = kind === "unit" ? "business unit" : "customer";
  const plural = kind === "unit" ? "business units" : "customers";
  // one screen serves two resources, so the capability it gates on follows the
  // kind rather than the file (#1183)
  const gate: Capability =
    kind === "unit" ? "business_unit:create" : "customer:create";
  // the row controls follow the same resource the create button does (#1258)
  const updateGate: Capability =
    kind === "unit" ? "business_unit:update" : "customer:update";
  const deleteGate: Capability =
    kind === "unit" ? "business_unit:delete" : "customer:delete";
  const unitName = (id: string | null) =>
    units.find((u) => u.id === id)?.name ?? null;
  const spendById = new Map(spend.map((row) => [row.id, row]));

  if (isLoading) {
    return (
      <PageBody>
        <TableSkeleton rows={4} />
      </PageBody>
    );
  }
  if (isError) {
    return (
      <PageBody>
        <LoadError
          error={error}
          resource={
            kind === "unit"
              ? t("errors.resources.businessUnits")
              : t("errors.resources.customers")
          }
          onRetry={onRetry}
        />
      </PageBody>
    );
  }

  const active = rows.filter((r) => !r.retired_at).length;

  return (
    <PageBody>
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-sm text-muted-foreground">
          {rows.length} {plural} · {active} active
        </span>
        {/* the confirmation carries the delete's own failure, so the banner
            stands down while it is open rather than saying it twice */}
        {mutationError && !deleteTarget && (
          <span className="text-xs text-[color:var(--status-danger-text)]">{mutationError.message}</span>
        )}
        <GatedButton gate={gate} className="ml-auto" disabled={disabled} onClick={startCreate}>
          + New {noun}
        </GatedButton>
      </div>

      <SpendStrip
        rows={spend}
        loading={spendLoading}
        error={spendError}
        onRetry={onRetrySpend}
      />

      {rows.length === 0 ? (
        <EmptyState
          uxTarget="cost-attribution"
          icon={kind === "unit" ? <Building2 /> : <WalletCards />}
          title={
            kind === "unit"
              ? t("pages.costAttribution.unitEmptyTitle")
              : t("pages.costAttribution.customerEmptyTitle")
          }
          description={
            kind === "unit"
              ? t("pages.costAttribution.unitEmptyBody")
              : t("pages.costAttribution.customerEmptyBody")
          }
          actions={
            <GatedButton gate={gate} disabled={disabled} onClick={startCreate}>
              + New {noun}
            </GatedButton>
          }
        />
      ) : (
        <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(300px,100%),1fr))]">
          {rows.map((row) => {
            const assigned =
              "business_unit_id" in row ? unitName(row.business_unit_id) : null;
            return (
              <div
                key={row.id}
                // a retired unit reads as a quieter card, not as faded text:
                // container opacity takes every glyph under 4.5:1 (#1181)
                className={cn(
                  "flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] p-4",
                  row.retired_at ? "bg-[color:var(--surface-subtle)]/60" : "bg-card",
                )}
              >
                <div className="flex items-start gap-2.5">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-semibold">{row.name}</div>
                    <div className="truncate font-mono text-xs text-muted-foreground">
                      {row.slug}
                    </div>
                  </div>
                  <RetiredBadge retiredAt={row.retired_at} />
                </div>
                <CardSpend row={spendById.get(row.id)} />
                {kind === "customer" && (
                  <div className="text-xs text-muted-foreground">
                    {assigned ? (
                      <>
                        rolls up into{" "}
                        <span className="text-foreground">{assigned}</span>
                      </>
                    ) : (
                      "unassigned"
                    )}
                  </div>
                )}
                <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-2.5">
                  <GatedButton
                    gate={updateGate}
                    variant="outline"
                    size="sm"
                    aria-label={t("pages.costAttribution.editAria", { name: row.name })}
                    disabled={disabled}
                    onClick={() => startEdit(row)}
                  >
                    Edit
                  </GatedButton>
                  {/* retiring keeps the history a delete would strand */}
                  <GatedButton
                    gate={updateGate}
                    variant="ghost"
                    size="sm"
                    aria-label={t(
                      row.retired_at
                        ? "pages.costAttribution.restoreAria"
                        : "pages.costAttribution.retireAria",
                      { name: row.name },
                    )}
                    disabled={disabled || mutating}
                    onClick={() => onRetire(row, !row.retired_at)}
                  >
                    {row.retired_at ? "Restore" : "Retire"}
                  </GatedButton>
                  <GatedButton
                    gate={deleteGate}
                    variant="ghost"
                    size="sm"
                    className="ml-auto text-[color:var(--status-danger-text)]"
                    aria-label={t("pages.costAttribution.deleteAria", { name: row.name })}
                    disabled={disabled || mutating}
                    onClick={() => startDelete(row)}
                  >
                    Delete
                  </GatedButton>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(o) => !o && setDeleteTarget(null)}
        title={t("pages.costAttribution.confirm.title", { name: deleteTarget?.name })}
        description={t(
          kind === "unit"
            ? "pages.costAttribution.confirm.unitBody"
            : "pages.costAttribution.confirm.customerBody",
        )}
        confirmLabel={t("common.delete")}
        pending={deleting}
        error={deleteError}
        onConfirm={() =>
          deleteTarget && onDelete(deleteTarget, () => setDeleteTarget(null))
        }
      />

      <Sheet open={open} onOpenChange={setOpen}>
        <Editor
          kind={kind}
          form={form}
          setForm={setForm}
          original={editing?.slug ?? null}
          units={units}
          onClose={() => setOpen(false)}
          pending={mutating}
          error={mutationError?.message}
          onSubmit={() => {
            if (editing) onUpdate(form, editing);
            else onCreate(form);
            setOpen(false);
          }}
        />
      </Sheet>
    </PageBody>
  );
}

// business units: roll teams up into cost-attributed units (#539, #563)
export function BusinessUnits() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  const orgId = scope.orgId as string | undefined;

  const units = useQuery({
    queryKey: ["business-units", orgId],
    queryFn: () => fetchBusinessUnits(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  // spend for the window, unattributed bucket included: the strip's whole job
  // is to show what the cards below it do not account for
  const spend = useQuery({
    queryKey: ["analytics", "by-attribution", "business_unit"],
    queryFn: () => fetchAttributionSpend(SPEND_WINDOW, "business_unit", true),
    retry: false,
  });


  // UX stream (#805); screen key comes from the enclosing UxScreenProvider

  useScreenReady(!units.isLoading);

  useErrorState(!!units.error, "business-units");
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["business-units", orgId] });

  const create = useMutation({
    mutationFn: (f: EditorState) =>
      createBusinessUnit(orgId as string, {
        name: f.name.trim(),
        slug: f.slug.trim() || undefined,
      }),
    onSuccess: (_result, f) => {
      invalidate();
      toast.push({ tone: "success", title: t("toast.created", { what: f.name.trim() }) });
    },
    onError: (error, f) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: f.name.trim() }),
        detail: errorDetail(error),
      });
    },
  });
  const update = useMutation({
    mutationFn: ({ form, row }: { form: EditorState; row: BusinessUnitRow }) =>
      updateBusinessUnit(row.id, {
        name: form.name.trim(),
        slug: form.slug.trim() || undefined,
        allow_slug_change: form.allowSlugChange,
      }),
    onSuccess: (_result, { form }) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: form.name.trim() }),
      });
    },
    onError: (error, { form }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: form.name.trim() }),
        detail: errorDetail(error),
      });
    },
  });
  const retire = useMutation({
    mutationFn: ({ row, retired }: { row: BusinessUnitRow; retired: boolean }) =>
      updateBusinessUnit(row.id, { retired }),
    onSuccess: (_result, { row }) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: row.name }),
      });
    },
    onError: (error, { row }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: row.name }),
        detail: errorDetail(error),
      });
    },
  });
  const remove = useMutation({
    mutationFn: (row: BusinessUnitRow) => deleteBusinessUnit(row.id),
    onSuccess: (_result, row) => {
      invalidate();
      toast.push({ tone: "success", title: t("toast.deleted", { what: row.name }) });
    },
    onError: (error, row) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: row.name }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <AttributionScreen
      kind="unit"
      rows={units.data ?? []}
      units={units.data ?? []}
      isLoading={units.isLoading}
      isError={units.isError}
      error={units.error as Error | undefined}
      onRetry={() => void units.refetch()}
      disabled={!orgId}
      mutating={
        create.isPending || update.isPending || retire.isPending || remove.isPending
      }
      mutationError={
        (create.error ?? update.error ?? retire.error ?? remove.error) as
          | Error
          | undefined
      }
      onCreate={(form) => create.mutate(form)}
      onUpdate={(form, row) => update.mutate({ form, row })}
      onRetire={(row, retired) => retire.mutate({ row, retired })}
      onDelete={(row, onSuccess) => remove.mutate(row, { onSuccess })}
      deleting={remove.isPending}
      deleteError={remove.error}
      resetDelete={() => remove.reset()}
      spend={spend.data ?? []}
      spendLoading={spend.isLoading}
      spendError={spend.error}
      onRetrySpend={() => void spend.refetch()}
    />
  );
}

// customers: attribute usage and spend to the org's own customers (#539, #563)
export function Customers() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  const orgId = scope.orgId as string | undefined;

  const customers = useQuery({
    queryKey: ["customers", orgId],
    queryFn: () => fetchCustomers(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const spend = useQuery({
    queryKey: ["analytics", "by-attribution", "customer"],
    queryFn: () => fetchAttributionSpend(SPEND_WINDOW, "customer", true),
    retry: false,
  });


  // UX stream (#805); screen key comes from the enclosing UxScreenProvider

  useScreenReady(!customers.isLoading);

  useErrorState(!!customers.error, "customers");
  // needed for the assignment dropdown and to name the unit on each card
  const units = useQuery({
    queryKey: ["business-units", orgId],
    queryFn: () => fetchBusinessUnits(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: ["customers", orgId] });

  const create = useMutation({
    mutationFn: (f: EditorState) =>
      createCustomer(orgId as string, {
        name: f.name.trim(),
        slug: f.slug.trim() || undefined,
        business_unit_id:
          f.businessUnitId === UNASSIGNED ? null : f.businessUnitId,
      }),
    onSuccess: (_result, f) => {
      invalidate();
      toast.push({ tone: "success", title: t("toast.created", { what: f.name.trim() }) });
    },
    onError: (error, f) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: f.name.trim() }),
        detail: errorDetail(error),
      });
    },
  });
  const update = useMutation({
    mutationFn: ({ form, row }: { form: EditorState; row: CustomerRow }) =>
      updateCustomer(row.id, {
        name: form.name.trim(),
        slug: form.slug.trim() || undefined,
        allow_slug_change: form.allowSlugChange,
        // null unassigns; the server treats an omitted field as unchanged, so
        // the editor always sends its current selection
        business_unit_id:
          form.businessUnitId === UNASSIGNED ? null : form.businessUnitId,
      }),
    onSuccess: (_result, { form }) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: form.name.trim() }),
      });
    },
    onError: (error, { form }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: form.name.trim() }),
        detail: errorDetail(error),
      });
    },
  });
  const retire = useMutation({
    mutationFn: ({ row, retired }: { row: CustomerRow; retired: boolean }) =>
      updateCustomer(row.id, { retired }),
    onSuccess: (_result, { row }) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: row.name }),
      });
    },
    onError: (error, { row }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: row.name }),
        detail: errorDetail(error),
      });
    },
  });
  const remove = useMutation({
    mutationFn: (row: CustomerRow) => deleteCustomer(row.id),
    onSuccess: (_result, row) => {
      invalidate();
      toast.push({ tone: "success", title: t("toast.deleted", { what: row.name }) });
    },
    onError: (error, row) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: row.name }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <AttributionScreen
      kind="customer"
      rows={customers.data ?? []}
      units={units.data ?? []}
      isLoading={customers.isLoading}
      isError={customers.isError}
      error={customers.error as Error | undefined}
      onRetry={() => void customers.refetch()}
      disabled={!orgId}
      mutating={
        create.isPending || update.isPending || retire.isPending || remove.isPending
      }
      mutationError={
        (create.error ?? update.error ?? retire.error ?? remove.error) as
          | Error
          | undefined
      }
      onCreate={(form) => create.mutate(form)}
      onUpdate={(form, row) => update.mutate({ form, row })}
      onRetire={(row, retired) => retire.mutate({ row, retired })}
      onDelete={(row, onSuccess) => remove.mutate(row, { onSuccess })}
      deleting={remove.isPending}
      deleteError={remove.error}
      resetDelete={() => remove.reset()}
      spend={spend.data ?? []}
      spendLoading={spend.isLoading}
      spendError={spend.error}
      onRetrySpend={() => void spend.refetch()}
    />
  );
}
