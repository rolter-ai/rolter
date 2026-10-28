import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Plus, ShieldCheck, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListTable,
  PageBody,
  primaryColumn,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ChipGroup } from "@/components/ui/chip-group";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import {
  Sheet,
  SheetActions,
  SheetBody,
  SheetError,
  SheetFooter,
  SheetHeader,
} from "@/components/ui/sheet";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  POLICY_SCOPES,
  createGuardrailPolicy,
  deleteGuardrailPolicy,
  fetchGuardrailPolicies,
  fetchGuardrailProviders,
  updateGuardrailPolicy,
  type GuardrailPolicyInput,
  type GuardrailPolicyRow,
  type GuardrailProviderRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useFormat } from "@/lib/i18n/format";
import { useToast } from "@/lib/toast";
import { useFormTelemetry, useScreenReady } from "@/lib/ux-react";

const GRID = `${primaryColumn(1.1)} 1.2fr 1.4fr 1.3fr 0.8fr 0.8fr 0.8fr 150px`;

/** a threshold row as typed: the number stays text until it is saved */
interface DraftThreshold {
  category: string;
  value: string;
}

interface Draft {
  name: string;
  enabled: boolean;
  scope_kind: GuardrailPolicyRow["scope_kind"];
  scope_id: string;
  provider_ids: string[];
  thresholds: DraftThreshold[];
  on_violation: GuardrailPolicyRow["on_violation"];
  streaming: GuardrailPolicyRow["streaming"];
  description: string;
}

const blankDraft = (): Draft => ({
  name: "",
  enabled: false,
  scope_kind: "deployment",
  scope_id: "",
  provider_ids: [],
  thresholds: [],
  on_violation: "block",
  streaming: "reject",
  description: "",
});

const fromRow = (row: GuardrailPolicyRow): Draft => ({
  name: row.name,
  enabled: row.enabled,
  scope_kind: row.scope_kind,
  scope_id: row.scope_id ?? "",
  provider_ids: row.provider_ids,
  thresholds: Object.entries(row.thresholds).map(([category, value]) => ({
    category,
    value: String(value),
  })),
  on_violation: row.on_violation,
  streaming: row.streaming,
  description: row.description ?? "",
});

/** a threshold is a score the provider's category must reach: a number in [0, 1] */
function thresholdValue(text: string): number | null {
  if (text.trim() === "") return null;
  const value = Number(text);
  return Number.isFinite(value) && value >= 0 && value <= 1 ? value : null;
}

type ThresholdProblem = "category" | "duplicate" | "range";

/** what is wrong with each threshold row, `undefined` where it is fine */
function thresholdProblems(rows: DraftThreshold[]): (ThresholdProblem | undefined)[] {
  return rows.map((row, index) => {
    const category = row.category.trim();
    if (category === "") return "category";
    if (rows.findIndex((other) => other.category.trim() === category) !== index) {
      return "duplicate";
    }
    return thresholdValue(row.value) === null ? "range" : undefined;
  });
}

function toInput(draft: Draft): GuardrailPolicyInput {
  return {
    name: draft.name.trim(),
    enabled: draft.enabled,
    scope_kind: draft.scope_kind,
    scope_id: draft.scope_kind === "deployment" ? null : draft.scope_id.trim(),
    provider_ids: draft.provider_ids,
    thresholds: Object.fromEntries(
      draft.thresholds.map((row) => [row.category.trim(), thresholdValue(row.value) ?? 0]),
    ),
    on_violation: draft.on_violation,
    streaming: draft.streaming,
    description: draft.description.trim() === "" ? null : draft.description.trim(),
  };
}

function GuardrailPoliciesScreen() {
  const { t } = useTranslation();
  const toast = useToast();
  const client = useQueryClient();
  const { number } = useFormat();

  const policies = useQuery({
    queryKey: ["guardrail-policies"],
    queryFn: fetchGuardrailPolicies,
    retry: false,
  });
  // provider names for the ids a policy stores; a failed read leaves ids bare
  const providers = useQuery({
    queryKey: ["guardrail-providers", "names"],
    queryFn: fetchGuardrailProviders,
    retry: false,
  });
  // UX stream (#805); `policies` is the read the user is waiting on
  useScreenReady(!policies.isLoading);

  const providerRows = providers.data ?? [];
  const providerName = (id: string) => providerRows.find((row) => row.id === id)?.name ?? id;

  const [sheet, setSheet] = React.useState<{ policy: GuardrailPolicyRow | null } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<GuardrailPolicyRow | null>(null);
  const remove = useMutation({
    mutationFn: deleteGuardrailPolicy,
    onSuccess: () => void client.invalidateQueries({ queryKey: ["guardrail-policies"] }),
  });

  const rows = policies.data ?? [];
  const scopeLabel = (kind: GuardrailPolicyRow["scope_kind"]) =>
    t(`pages.guardrailPolicies.scopes.${kind}`);

  return (
    <PageBody>
      <div className="flex flex-col gap-3 border-b border-[color:var(--border-subtle)] pb-5 sm:flex-row sm:items-end sm:justify-between">
        <div className="max-w-2xl space-y-2 text-sm text-muted-foreground">
          <p>{t("pages.guardrailPolicies.intro")}</p>
          <p>{t("pages.guardrailPolicies.precedence")}</p>
        </div>
        <GatedButton
          gate="guardrail_policy:create"
          control="guardrail-policy-new"
          onClick={() => setSheet({ policy: null })}
        >
          <Plus className="h-4 w-4" aria-hidden /> {t("pages.guardrailPolicies.addPolicy")}
        </GatedButton>
      </div>

      {policies.error && (
        <LoadError
          error={policies.error}
          resource={t("errors.resources.guardrailPolicies")}
          onRetry={() => void policies.refetch()}
          target="guardrail-policies"
        />
      )}

      <ListTable label={t("screens.guardrail-policies.title")} minWidth={1000}>
        <ListHeader grid={GRID}>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.name")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.scope")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.providers")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.thresholds")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.onViolation")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.streaming")}</ListHeaderCell>
          <ListHeaderCell>{t("pages.guardrailPolicies.columns.state")}</ListHeaderCell>
          <ListActionsHeader />
        </ListHeader>
        <ListLoadingRow read={policies}>
          <ListSkeleton rows={4} className="p-3" />
        </ListLoadingRow>
        {rows.map((policy) => (
          <ListRow key={policy.id} grid={GRID}>
            <ListCell className="flex min-w-0 flex-col gap-0.5">
              <span className="truncate font-mono text-sm">{policy.name}</span>
              {policy.description && (
                <span className="truncate text-xs text-muted-foreground">{policy.description}</span>
              )}
            </ListCell>
            <ListCell className="flex min-w-0 flex-col items-start gap-1">
              <Badge tone="outline">{scopeLabel(policy.scope_kind)}</Badge>
              {policy.scope_id && (
                <span className="max-w-full truncate font-mono text-xs text-[color:var(--text-secondary)]">
                  {policy.scope_id}
                </span>
              )}
            </ListCell>
            <ListCell className="flex min-w-0 flex-wrap items-center gap-1">
              {policy.provider_ids.length === 0 ? (
                <span className="text-xs text-muted-foreground">
                  {t("pages.guardrailPolicies.noProviders")}
                </span>
              ) : (
                policy.provider_ids.map((id, index) => (
                  <Badge key={id} tone="outline" className="font-mono text-2xs">
                    {index + 1}. {providerName(id)}
                  </Badge>
                ))
              )}
            </ListCell>
            <ListCell className="flex min-w-0 flex-wrap items-center gap-1">
              {Object.keys(policy.thresholds).length === 0 ? (
                <span className="text-xs text-muted-foreground">
                  {t("pages.guardrailPolicies.noThresholds")}
                </span>
              ) : (
                Object.entries(policy.thresholds).map(([category, value]) => (
                  <Badge key={category} tone="outline" className="font-mono text-2xs">
                    {category} ≥ {number(value)}
                  </Badge>
                ))
              )}
            </ListCell>
            <ListCell className="grid">
              <Badge tone={policy.on_violation === "block" ? "neutral" : "warning"}>
                {t(`pages.guardrailPolicies.onViolationValues.${policy.on_violation}`)}
              </Badge>
            </ListCell>
            <ListCell className="grid">
              <Badge tone="outline">
                {t(`pages.guardrailPolicies.streamingValues.${policy.streaming}`)}
              </Badge>
            </ListCell>
            <ListCell className="grid">
              <Badge tone={policy.enabled ? "success" : "neutral"}>
                {policy.enabled
                  ? t("pages.guardrailPolicies.enabled")
                  : t("pages.guardrailPolicies.paused")}
              </Badge>
            </ListCell>
            <ListCell className="flex items-center justify-end gap-1.5">
              <GatedButton
                gate="guardrail_policy:update"
                control="guardrail-policy-edit"
                size="sm"
                variant="outline"
                className="h-[30px]"
                aria-label={t("pages.guardrailPolicies.editOne", { name: policy.name })}
                onClick={() => setSheet({ policy })}
              >
                {t("pages.guardrailPolicies.edit")}
              </GatedButton>
              <DeleteIconButton
                gate="guardrail_policy:delete"
                control="guardrail-policy-delete"
                label={t("pages.guardrailPolicies.deleteOne", { name: policy.name })}
                onClick={() => {
                  remove.reset();
                  setDeleteTarget(policy);
                }}
              />
            </ListCell>
          </ListRow>
        ))}
        <ListEmptyRow read={policies} rows={rows.length}>
          <EmptyState
            thread
            uxTarget="guardrail-policies"
            icon={<ShieldCheck />}
            title={t("pages.guardrailPolicies.emptyTitle")}
            description={t("pages.guardrailPolicies.emptyBody")}
            actions={
              <GatedButton
                gate="guardrail_policy:create"
                control="guardrail-policy-new-empty"
                onClick={() => setSheet({ policy: null })}
              >
                {t("pages.guardrailPolicies.emptyAction")}
              </GatedButton>
            }
          />
        </ListEmptyRow>
      </ListTable>

      <PolicySheet
        key={sheet?.policy?.id ?? (sheet ? "new" : "closed")}
        open={!!sheet}
        policy={sheet?.policy ?? null}
        providers={providerRows}
        onOpenChange={(open) => !open && setSheet(null)}
        onDone={() => void client.invalidateQueries({ queryKey: ["guardrail-policies"] })}
      />

      <ConfirmDialog
        name="guardrail-policy-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDeleteTarget(null);
          remove.reset();
        }}
        title={t("pages.guardrailPolicies.confirm.deleteTitle", { name: deleteTarget?.name })}
        description={t("pages.guardrailPolicies.confirm.deleteBody", {
          scope: deleteTarget ? scopeLabel(deleteTarget.scope_kind) : "",
        })}
        confirmLabel={t("common.delete")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.name;
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) =>
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorText(error, t),
              }),
          });
        }}
      />
    </PageBody>
  );
}

/** the providers a policy lists, picked as chips and ordered underneath */
function ProviderOrder({
  providers,
  selected,
  onChange,
}: {
  providers: GuardrailProviderRow[];
  selected: string[];
  onChange: (next: string[]) => void;
}) {
  const { t } = useTranslation();
  const nameOf = (id: string) => providers.find((row) => row.id === id)?.name ?? id;
  const move = (index: number, by: -1 | 1) => {
    const next = [...selected];
    [next[index], next[index + by]] = [next[index + by], next[index]];
    onChange(next);
  };
  const button =
    "flex h-7 w-7 items-center justify-center rounded-sm border border-[color:var(--border-subtle)] text-[color:var(--text-secondary)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40";
  return (
    <div className="space-y-2">
      <ChipGroup
        label={t("pages.guardrailPolicies.sheet.providers")}
        options={providers.map((row) => ({ id: row.id, name: row.name }))}
        selected={selected}
        onToggle={(id) =>
          onChange(selected.includes(id) ? selected.filter((v) => v !== id) : [...selected, id])
        }
      />
      <p className="text-xs text-muted-foreground">
        {t("pages.guardrailPolicies.sheet.providersHint")}
      </p>
      {selected.length > 0 && (
        <ol
          aria-label={t("pages.guardrailPolicies.sheet.order")}
          className="space-y-1.5 rounded-md border border-[color:var(--border-subtle)] p-2"
        >
          {selected.map((id, index) => (
            <li key={id} className="flex items-center gap-2 text-sm">
              <span className="w-5 text-xs text-muted-foreground">{index + 1}.</span>
              <span className="min-w-0 flex-1 truncate font-mono">{nameOf(id)}</span>
              <button
                type="button"
                className={button}
                disabled={index === 0}
                aria-label={t("pages.guardrailPolicies.sheet.moveUp", { name: nameOf(id) })}
                onClick={() => move(index, -1)}
              >
                <ArrowUp className="h-3.5 w-3.5" aria-hidden />
              </button>
              <button
                type="button"
                className={button}
                disabled={index === selected.length - 1}
                aria-label={t("pages.guardrailPolicies.sheet.moveDown", { name: nameOf(id) })}
                onClick={() => move(index, 1)}
              >
                <ArrowDown className="h-3.5 w-3.5" aria-hidden />
              </button>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

function ThresholdEditor({
  rows,
  onChange,
}: {
  rows: DraftThreshold[];
  onChange: (next: DraftThreshold[]) => void;
}) {
  const { t } = useTranslation();
  const problems = thresholdProblems(rows);
  const update = (index: number, patch: Partial<DraftThreshold>) =>
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <FieldLabel
          label={t("pages.guardrailPolicies.sheet.thresholds")}
          info={t("pages.guardrailPolicies.sheet.thresholdsInfo")}
        />
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={() => onChange([...rows, { category: "", value: "0.8" }])}
        >
          <Plus className="h-3.5 w-3.5" aria-hidden />
          {t("pages.guardrailPolicies.sheet.addThreshold")}
        </Button>
      </div>
      {rows.length === 0 && (
        <p className="text-xs text-muted-foreground">
          {t("pages.guardrailPolicies.sheet.noThresholds")}
        </p>
      )}
      {rows.map((row, index) => {
        const problem = problems[index];
        const errorId = `policy-threshold-${index}-error`;
        return (
          <React.Fragment key={index}>
            <div className="grid grid-cols-[minmax(0,1fr)_90px_28px] items-center gap-2">
              <Input
                aria-label={t("pages.guardrailPolicies.sheet.category")}
                aria-invalid={problem === "category" || problem === "duplicate" || undefined}
                aria-describedby={describedBy(problem && problem !== "range" && errorId)}
                className="font-mono"
                placeholder="violence"
                value={row.category}
                onChange={(event) => update(index, { category: event.target.value })}
              />
              <Input
                aria-label={t("pages.guardrailPolicies.sheet.threshold")}
                aria-invalid={problem === "range" || undefined}
                aria-describedby={describedBy(problem === "range" && errorId)}
                inputMode="decimal"
                value={row.value}
                onChange={(event) => update(index, { value: event.target.value })}
              />
              <button
                type="button"
                aria-label={t("pages.guardrailPolicies.sheet.removeThreshold")}
                title={t("pages.guardrailPolicies.sheet.removeThreshold")}
                onClick={() => onChange(rows.filter((_, i) => i !== index))}
                className="flex flex-none items-center justify-center rounded-sm border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X className="h-3.5 w-3.5" aria-hidden />
              </button>
            </div>
            <FieldError
              id={errorId}
              error={problem ? t(`pages.guardrailPolicies.sheet.problem.${problem}`) : undefined}
            />
          </React.Fragment>
        );
      })}
    </div>
  );
}

function PolicySheet({
  open,
  policy,
  providers,
  onOpenChange,
  onDone,
}: {
  open: boolean;
  policy: GuardrailPolicyRow | null;
  providers: GuardrailProviderRow[];
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [draft, setDraft] = React.useState<Draft>(() => (policy ? fromRow(policy) : blankDraft()));
  const initial = React.useRef(JSON.stringify(draft));
  const set = (patch: Partial<Draft>) => setDraft((value) => ({ ...value, ...patch }));
  const dirty = JSON.stringify(draft) !== initial.current;
  const ux = useFormTelemetry(policy ? "guardrail-policy-edit" : "guardrail-policy-create", open, {
    dirty,
  });

  const save = useMutation({
    mutationFn: (body: GuardrailPolicyInput) =>
      policy ? updateGuardrailPolicy(policy.id, body) : createGuardrailPolicy(body),
    onSuccess: (_result, body) => {
      ux.saved();
      toast.push(
        policy
          ? {
              tone: "success",
              title: t("toast.saved"),
              detail: t("toast.savedDetail", { what: body.name }),
            }
          : { tone: "success", title: t("toast.created", { what: body.name }) },
      );
      onDone();
      onOpenChange(false);
    },
    onError: () => ux.failed(),
  });
  const { guard, close, locked, prompt } = useDiscardGuard({
    dirty,
    saving: save.isPending,
    onOpenChange,
  });

  const deployment = draft.scope_kind === "deployment";
  const scopeMissing = !deployment && draft.scope_id.trim() === "";
  const thresholdsValid = thresholdProblems(draft.thresholds).every((problem) => !problem);
  const canSave = draft.name.trim() !== "" && !scopeMissing && thresholdsValid && !save.isPending;
  const scopeErrorId = "policy-scope-id-error";

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader
        title={
          policy
            ? t("pages.guardrailPolicies.sheet.titleEdit", { name: policy.name })
            : t("pages.guardrailPolicies.sheet.titleAdd")
        }
        subtitle={t("pages.guardrailPolicies.sheet.subtitle")}
        onClose={close}
        closeDisabled={locked}
      />
      <SheetBody>
        <Field label={t("pages.guardrailPolicies.sheet.name")} htmlFor="policy-name" required>
          <Input
            id="policy-name"
            value={draft.name}
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>
        <Field label={t("pages.guardrailPolicies.sheet.description")} htmlFor="policy-description">
          <Input
            id="policy-description"
            value={draft.description}
            onChange={(event) => set({ description: event.target.value })}
          />
        </Field>

        <div className="space-y-1.5">
          <FieldLabel
            id="policy-scope-label"
            label={t("pages.guardrailPolicies.sheet.scope")}
            info={t("pages.guardrailPolicies.sheet.scopeInfo")}
          />
          <Segmented
            labelledBy="policy-scope-label"
            value={draft.scope_kind}
            onChange={(scope_kind) => set({ scope_kind })}
            options={POLICY_SCOPES.map((kind) => ({
              value: kind,
              label: t(`pages.guardrailPolicies.scopes.${kind}`),
            }))}
          />
        </div>
        {!deployment && (
          <div className="space-y-1.5">
            <FieldLabel
              label={t(`pages.guardrailPolicies.sheet.scopeId.${draft.scope_kind}`)}
              htmlFor="policy-scope-id"
              required
            />
            <Input
              id="policy-scope-id"
              className="font-mono"
              aria-invalid={scopeMissing || undefined}
              aria-describedby={describedBy(scopeMissing && scopeErrorId)}
              value={draft.scope_id}
              onChange={(event) => set({ scope_id: event.target.value })}
            />
            <FieldError
              id={scopeErrorId}
              error={
                scopeMissing && draft.scope_id !== ""
                  ? t("pages.guardrailPolicies.sheet.scopeIdRequired")
                  : undefined
              }
            />
          </div>
        )}

        <ProviderOrder
          providers={providers}
          selected={draft.provider_ids}
          onChange={(provider_ids) => set({ provider_ids })}
        />
        <ThresholdEditor rows={draft.thresholds} onChange={(thresholds) => set({ thresholds })} />

        <div className="space-y-1.5">
          <FieldLabel
            id="policy-violation-label"
            label={t("pages.guardrailPolicies.sheet.onViolation")}
            info={t("pages.guardrailPolicies.sheet.onViolationInfo")}
          />
          <Segmented
            labelledBy="policy-violation-label"
            value={draft.on_violation}
            onChange={(on_violation) => set({ on_violation })}
            options={(["block", "annotate"] as const).map((value) => ({
              value,
              label: t(`pages.guardrailPolicies.onViolationValues.${value}`),
            }))}
          />
        </div>
        <div className="space-y-1.5">
          <FieldLabel
            id="policy-streaming-label"
            label={t("pages.guardrailPolicies.sheet.streaming")}
            info={t("pages.guardrailPolicies.sheet.streamingInfo")}
          />
          <Segmented
            labelledBy="policy-streaming-label"
            value={draft.streaming}
            onChange={(streaming) => set({ streaming })}
            options={(["reject", "buffer"] as const).map((value) => ({
              value,
              label: t(`pages.guardrailPolicies.streamingValues.${value}`),
            }))}
          />
        </div>

        <SwitchRow
          title={t("pages.guardrailPolicies.sheet.enabled")}
          hint={t("pages.guardrailPolicies.sheet.enabledHint")}
          checked={draft.enabled}
          onChange={(enabled) => set({ enabled })}
        />
      </SheetBody>
      <SheetFooter>
        <SheetError message={save.isError ? errorText(save.error, t) : undefined} />
        <SheetActions>
          <Button variant="ghost" disabled={locked} onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button
            disabled={!canSave}
            onClick={() => {
              ux.submitted();
              save.mutate(toInput(draft));
            }}
          >
            {policy
              ? t("pages.guardrailPolicies.sheet.save")
              : t("pages.guardrailPolicies.sheet.create")}
          </Button>
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
export default superadminOnly(
  GuardrailPoliciesScreen,
  "errors.resources.guardrailPolicies",
  "guardrail_policy:read",
);
