import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Sparkles, Tag, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import {
  createLabel,
  deleteLabel,
  fetchLabels,
  updateLabel,
  type LabelRow,
} from "@/lib/api";
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
          ? [label.observation, observed].filter(Boolean).join(" · ") ||
            t("labels.autoTitle")
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

export interface LabelSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  subjectType: "provider" | "provider_group" | "route";
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

  const labels = useQuery({
    queryKey: [...LABELS_QUERY_KEY, orgId, subjectType, subjectId],
    queryFn: () => fetchLabels(orgId, { subject_type: subjectType, subject_id: subjectId }),
    enabled: open && !!orgId,
  });

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: LABELS_QUERY_KEY });
  };

  const add = useMutation({
    mutationFn: () =>
      createLabel(orgId, {
        subject_type: subjectType,
        subject_id: subjectId,
        key: key.trim(),
        value: value.trim() || undefined,
      }),
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
    mutationFn: (row: LabelRow) => deleteLabel(orgId, row.id),
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
      updateLabel(orgId, row.id, next.trim() || undefined),
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
                    gate="label:delete"
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
                gate="label:create"
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
