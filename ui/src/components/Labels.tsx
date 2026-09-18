import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Sparkles, Tag, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import {
  createLabel,
  createModelLabel,
  deleteLabel,
  deleteModelLabel,
  fetchLabels,
  fetchModelLabels,
  updateLabel,
  updateModelLabel,
  type LabelFilter,
  type LabelRow,
} from "@/lib/api";
import { type Capability } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { errorDetail, useToast } from "@/lib/toast";

// Labels on providers, provider groups and routes (#1329; the API is #985).
//
// The one thing this file exists to keep straight is that a `custom` label is
// something an operator said and an `auto` label is something a subsystem
// observed at a moment in time and may withdraw by observing again. The two can
// carry the same key on the same subject deliberately, so a screen that shows
// them alike reads as a duplicate rather than as two different kinds of claim —
// and an auto label has no edit or delete affordance at all, because the API
// answers 404 to both.

export const LABELS_QUERY_KEY = ["labels"];

/** `key=value`, or the bare key for a label that carries no value */
export function labelText(label: Pick<LabelRow, "key" | "value">): string {
  return label.value ? `${label.key}=${label.value}` : label.key;
}

/**
 * One label, as a chip.
 *
 * Auto and custom differ in three ways at once — tone, a leading icon, and the
 * word in the accessible name — because colour alone is not a distinction a
 * colour-blind reader or a screen reader can act on.
 */
export function LabelChip({ label }: { label: LabelRow }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const auto = label.source === "auto";
  const observed = auto && label.observed_at ? fmt.dateTime(label.observed_at) : undefined;
  return (
    <Badge
      tone={auto ? "info" : "outline"}
      className="max-w-[18rem] gap-1"
      data-testid={`label-${label.source}`}
      // an observation is only true of the moment it was made, so the tooltip
      // says when and what rather than presenting it as a standing fact
      title={
        auto
          ? [label.observation, observed].filter(Boolean).join(" · ") || t("labels.autoTitle")
          : undefined
      }
      aria-label={t(auto ? "labels.autoChipAria" : "labels.customChipAria", {
        label: labelText(label),
      })}
    >
      {auto ? (
        <Sparkles className="h-3 w-3 flex-none" aria-hidden />
      ) : (
        <Tag className="h-3 w-3 flex-none" aria-hidden />
      )}
      <span className="truncate font-mono">{labelText(label)}</span>
    </Badge>
  );
}

/** the chips for one subject, in a row that wraps */
export function LabelChips({ labels }: { labels: LabelRow[] }) {
  if (labels.length === 0) return null;
  return (
    <span className="flex flex-wrap items-center gap-1">
      {labels.map((label) => (
        <LabelChip key={label.id} label={label} />
      ))}
    </span>
  );
}

export type LabelSubject = "provider" | "provider_group" | "route" | "model";

/**
 * The two surfaces over one table, as the dashboard uses them.
 *
 * A provider, group or route belongs to an org and is addressed by its row id;
 * a model belongs to the deployment-wide pricing catalog and is addressed by
 * its name, which is also why writing one is a superadmin's act. Everything
 * above this line is the same for both, so the difference lives here.
 */
function endpoints(subjectType: LabelSubject, orgId: string) {
  const model = subjectType === "model";
  return {
    list: (filter: LabelFilter) => (model ? fetchModelLabels(filter) : fetchLabels(orgId, filter)),
    create: (subjectId: string, key: string, value?: string) =>
      model
        ? createModelLabel({ model: subjectId, key, value })
        : createLabel(orgId, { subject_type: subjectType, subject_id: subjectId, key, value }),
    update: (id: string, value?: string) =>
      model ? updateModelLabel(id, value) : updateLabel(orgId, id, value),
    remove: (id: string) => (model ? deleteModelLabel(id) : deleteLabel(orgId, id)),
    createGate: (model ? "model_label:create" : "label:create") as Capability,
    deleteGate: (model ? "model_label:delete" : "label:delete") as Capability,
  };
}

export interface LabelSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** unused for a model, which is deployment-wide */
  orgId: string;
  subjectType: LabelSubject;
  /** the row id, or the model's name */
  subjectId: string;
  /** the subject's own name, which is what the panel is about */
  subjectName: string;
}

/**
 * Read, add, retitle and remove the labels on one subject.
 *
 * Its own query rather than a slice of the caller's list: a label written here
 * has to show up here, and the screens that render chips read the whole org's
 * labels in one request.
 */
export function LabelSheet({
  open,
  onOpenChange,
  orgId,
  subjectType,
  subjectId,
  subjectName,
}: LabelSheetProps) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [key, setKey] = React.useState("");
  const [value, setValue] = React.useState("");
  const [deleting, setDeleting] = React.useState<LabelRow | null>(null);

  const api = endpoints(subjectType, orgId);
  const labels = useQuery({
    queryKey: [...LABELS_QUERY_KEY, orgId, subjectType, subjectId],
    queryFn: () => api.list({ subject_type: subjectType, subject_id: subjectId }),
    enabled: open && (subjectType === "model" || !!orgId),
  });

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: LABELS_QUERY_KEY });
  };

  const add = useMutation({
    mutationFn: () => api.create(subjectId, key.trim(), value.trim() || undefined),
    onSuccess: () => {
      invalidate();
      setKey("");
      setValue("");
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("labels.addFailed"),
        detail: errorDetail(error),
      }),
  });

  const remove = useMutation({
    mutationFn: (row: LabelRow) => api.remove(row.id),
    onSuccess: (_result, row) => {
      invalidate();
      toast.push({ tone: "success", title: t("labels.removed", { label: labelText(row) }) });
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("labels.removeFailed"),
        detail: errorDetail(error),
      }),
  });

  const rename = useMutation({
    mutationFn: ({ row, next }: { row: LabelRow; next: string }) =>
      api.update(row.id, next.trim() || undefined),
    onSuccess: invalidate,
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("labels.saveFailed"),
        detail: errorDetail(error),
      }),
  });

  const rows = labels.data ?? [];
  const custom = rows.filter((l) => l.source === "custom");
  const auto = rows.filter((l) => l.source === "auto");
  // the API's own 409: that key is already set on this subject. Named here
  // because "conflict" is not a sentence an operator can act on
  const duplicate = custom.some((l) => l.key === key.trim());

  return (
    <>
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetHeader
          title={t("labels.sheetTitle")}
          subtitle={subjectName}
          onClose={() => onOpenChange(false)}
        />
        <SheetBody>
          {labels.isLoading && <ListSkeleton rows={3} />}
          {labels.error && (
            <LoadError
              error={labels.error}
              resource={t("errors.resources.labels")}
              onRetry={() => void labels.refetch()}
            />
          )}
          {!labels.isLoading && !labels.error && rows.length === 0 && (
            <EmptyState
              uxTarget="labels"
              icon={<Tag />}
              title={t("labels.emptyTitle")}
              description={t("labels.emptyBody")}
            />
          )}

          {custom.length > 0 && (
            <div className="flex flex-col gap-2">
              <p className="text-xs uppercase tracking-wide text-muted-foreground">
                {t("labels.customHeading")}
              </p>
              {custom.map((row) => (
                <div
                  key={row.id}
                  className="flex items-center gap-2 rounded-[10px] border border-[color:var(--border-default)] px-3 py-2"
                >
                  <span className="w-40 flex-none truncate font-mono text-xs">{row.key}</span>
                  <Input
                    aria-label={t("labels.valueOf", { key: row.key })}
                    defaultValue={row.value ?? ""}
                    placeholder={t("labels.valuePlaceholder")}
                    onBlur={(e) => {
                      const next = e.target.value;
                      if (next !== (row.value ?? "")) rename.mutate({ row, next });
                    }}
                  />
                  <GatedButton
                    gate={api.deleteGate}
                    variant="ghost"
                    size="sm"
                    aria-label={t("labels.removeOne", { label: labelText(row) })}
                    onClick={() => setDeleting(row)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </GatedButton>
                </div>
              ))}
            </div>
          )}

          {auto.length > 0 && (
            <div className="flex flex-col gap-2">
              <p className="text-xs uppercase tracking-wide text-muted-foreground">
                {t("labels.autoHeading")}
              </p>
              {/* no edit, no delete: the API answers 404 to both, and the only
                  way one of these changes is the subsystem observing again */}
              <p className="text-xs leading-snug text-muted-foreground">
                {t("labels.autoExplainer")}
              </p>
              {auto.map((row) => (
                <div
                  key={row.id}
                  className="flex flex-col gap-1 rounded-[10px] border border-dashed border-[color:var(--border-default)] px-3 py-2"
                >
                  <LabelChips labels={[row]} />
                  <p className="text-xs text-muted-foreground">
                    {row.observed_at
                      ? t("labels.observedAt", {
                          what: row.observation ?? t("labels.observationUnknown"),
                          when: fmt.dateTime(row.observed_at),
                        })
                      : (row.observation ?? t("labels.observationUnknown"))}
                  </p>
                </div>
              ))}
            </div>
          )}
        </SheetBody>
        <SheetFooter>
          <div className="flex flex-col gap-2 px-[22px] py-3.5">
            <div className="flex items-end gap-2">
              <Field label={t("labels.keyLabel")} className="flex-1">
                <Input
                  value={key}
                  placeholder={t("labels.keyPlaceholder")}
                  onChange={(e) => setKey(e.target.value)}
                />
              </Field>
              <Field label={t("labels.valueLabel")} className="flex-1">
                <Input
                  value={value}
                  placeholder={t("labels.valuePlaceholder")}
                  onChange={(e) => setValue(e.target.value)}
                />
              </Field>
              <GatedButton
                gate={api.createGate}
                className="mb-[1px]"
                disabled={!key.trim() || duplicate || add.isPending}
                onClick={() => add.mutate()}
              >
                {t("labels.add")}
              </GatedButton>
            </div>
            {duplicate && (
              <p className="text-xs text-[color:var(--status-danger-text)]">
                {t("labels.duplicateKey", { key: key.trim() })}
              </p>
            )}
          </div>
        </SheetFooter>
      </Sheet>

      <ConfirmDialog
        open={!!deleting}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={t("labels.confirm.title", { label: deleting ? labelText(deleting) : "" })}
        description={t("labels.confirm.body", { subject: subjectName })}
        confirmLabel={t("common.delete")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() =>
          deleting && remove.mutate(deleting, { onSuccess: () => setDeleting(null) })
        }
      />
    </>
  );
}

/** the distinct `key=value` pairs in a set of labels, for a filter control */
export function labelOptions(labels: LabelRow[]): string[] {
  return [...new Set(labels.map(labelText))].sort((a, b) => a.localeCompare(b));
}

export interface SubjectLabels {
  /** the labels on one subject, by its id */
  bySubject: (id: string) => LabelRow[];
  /** the `key=value` pairs present, for the filter control */
  options: string[];
  /** whether a subject carries the chosen `key=value`; "" matches everything */
  matches: (id: string, filter: string) => boolean;
}

/**
 * Every label of one kind in the org, in one request rather than one per row.
 *
 * `retry: false` and no error surface: a caller without `label:read` gets a 403
 * that will not improve, and the screens these hang off are about providers,
 * groups and routes — they keep working without the labels.
 */
export function useSubjectLabels(
  orgId: string | undefined,
  subjectType: LabelSubject,
): SubjectLabels {
  const labels = useQuery({
    queryKey: [...LABELS_QUERY_KEY, orgId, subjectType],
    queryFn: () =>
      subjectType === "model"
        ? fetchModelLabels()
        : fetchLabels(orgId as string, { subject_type: subjectType }),
    enabled: subjectType === "model" || !!orgId,
    retry: false,
  });
  const map = React.useMemo(() => {
    const out = new Map<string, LabelRow[]>();
    for (const label of labels.data ?? []) {
      out.set(label.subject_id, [...(out.get(label.subject_id) ?? []), label]);
    }
    return out;
  }, [labels.data]);
  const bySubject = React.useCallback((id: string) => map.get(id) ?? [], [map]);
  return {
    bySubject,
    options: labelOptions(labels.data ?? []),
    matches: (id, filter) => !filter || bySubject(id).some((l) => labelText(l) === filter),
  };
}

/** the toolbar control that narrows a list to one `key=value` */
export function LabelFilterSelect({
  value,
  onChange,
  options,
}: {
  value: string;
  onChange: (value: string) => void;
  options: string[];
}) {
  const { t } = useTranslation();
  return (
    <Combobox
      size="sm"
      clearable
      className="w-56"
      value={value}
      onChange={onChange}
      placeholder={t("labels.filter")}
      aria-label={t("labels.filter")}
      options={options.map((l) => ({ value: l, label: l }))}
    />
  );
}
