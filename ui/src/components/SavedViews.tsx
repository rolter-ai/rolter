import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bookmark, Pencil, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadError } from "@/components/LoadError";
import { ListSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { Sheet, SheetBody, SheetHeader } from "@/components/ui/sheet";
import {
  ApiError,
  createSavedView,
  deleteSavedView,
  fetchSavedViews,
  updateSavedView,
  type SavedView,
  type SavedViewFilters,
  type SavedViewSurface,
  type SavedViewUnavailable,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";

// private filter presets for one screen (#1825, #2452). the control plane keeps
// them per account, so this is the same list on any browser the account signs
// in from. applying hands the parent `effective_filters`, never `filters`: the
// stored set may still name a key, unit or customer the account can no longer
// read, and the response says which in `unavailable`. those ids are not
// readable, so the notice counts them rather than naming them. saving sends
// whatever the parent says is applied now, so a dropped entry is never written
// back by a re-save

const SURFACE_TITLE: Record<SavedViewSurface, string> = {
  llm_logs: "savedViews.surface.llm_logs",
  dashboard: "savedViews.surface.dashboard",
};

// how many of each filter a view had to leave out, in the order the notice names them
const DROPPED_ORDER: SavedViewUnavailable["filter"][] = ["key", "business_unit", "customer"];

function countDropped(
  unavailable: SavedViewUnavailable[],
): [SavedViewUnavailable["filter"], number][] {
  return DROPPED_ORDER.map((filter): [SavedViewUnavailable["filter"], number] => [
    filter,
    unavailable.filter((u) => u.filter === filter).length,
  ]).filter(([, count]) => count > 0);
}

export interface SavedViewsProps {
  surface: SavedViewSurface;
  /** the filters applied right now, with `all` statuses and empty values already left out */
  current: SavedViewFilters;
  /** the view's `effective_filters`; the screen writes them into its address */
  onApply: (filters: SavedViewFilters) => void;
}

export function SavedViews({ surface, current, onApply }: SavedViewsProps) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const [open, setOpen] = React.useState(false);
  const [dropped, setDropped] = React.useState<[SavedViewUnavailable["filter"], number][]>([]);
  const [name, setName] = React.useState("");
  const [renaming, setRenaming] = React.useState<{ id: string; name: string } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<SavedView | null>(null);
  const nameInput = React.useRef<HTMLInputElement>(null);
  const renameError = React.useId();

  const key = ["saved-views", surface];
  const views = useQuery({
    queryKey: key,
    queryFn: () => fetchSavedViews(surface),
    enabled: open,
    retry: false,
  });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: key });

  const save = useMutation({
    mutationFn: () => createSavedView({ surface, name: name.trim(), filters: current }),
    onSuccess: (view) => {
      setName("");
      toast.push({ tone: "success", title: t("savedViews.saved", { name: view.name }) });
      return invalidate();
    },
  });
  const rename = useMutation({
    // the name alone: a `filters` value would replace the stored set
    mutationFn: (input: { id: string; name: string }) =>
      updateSavedView(input.id, { name: input.name.trim() }),
    onSuccess: () => {
      setRenaming(null);
      return invalidate();
    },
  });
  const remove = useMutation({
    mutationFn: (id: string) => deleteSavedView(id),
    onSuccess: invalidate,
  });

  const apply = (view: SavedView) => {
    onApply(view.effective_filters);
    setDropped(countDropped(view.unavailable));
    setOpen(false);
  };

  // a duplicate name and a full list are both a 409, told apart by the
  // control plane's own words, which sit under the lead
  const refusal = (error: unknown) =>
    error instanceof ApiError && error.status === 409
      ? t("savedViews.errors.conflict")
      : t("savedViews.errors.failed");
  const refusalDetail = (error: unknown) =>
    error instanceof ApiError && error.status === 409 ? undefined : errorDetail(error);

  const droppedText = dropped
    .map(([filter, count]) => t(`savedViews.dropped.${filter}`, { count }))
    .join(", ");

  return (
    <>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
          <Bookmark aria-hidden className="h-3.5 w-3.5" />
          {t("savedViews.open")}
        </Button>
        {dropped.length > 0 && (
          <p
            role="status"
            className="inline-flex items-center gap-1.5 text-xs text-[color:var(--status-warning-text)]"
          >
            {t("savedViews.notice", { dropped: droppedText })}
            <button
              type="button"
              title={t("savedViews.dismissNotice")}
              aria-label={t("savedViews.dismissNotice")}
              onClick={() => setDropped([])}
              className="rounded-sm text-[color:var(--text-subtle)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            >
              <X aria-hidden className="h-3.5 w-3.5" />
            </button>
          </p>
        )}
      </div>

      <Sheet open={open} onOpenChange={setOpen}>
        <SheetHeader
          title={t("savedViews.title")}
          subtitle={t(SURFACE_TITLE[surface])}
          onClose={() => setOpen(false)}
        />
        <SheetBody>
          {views.isPending ? (
            <ListSkeleton rows={3} />
          ) : views.isError ? (
            <LoadError
              error={views.error}
              resource={t("errors.resources.savedViews")}
              onRetry={() => void views.refetch()}
              target="saved-views"
            />
          ) : (
            <>
              <form
                className="flex flex-col gap-2"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (name.trim() && !save.isPending) save.mutate();
                }}
              >
                <Field
                  label={t("savedViews.save.label")}
                  hint={t("savedViews.save.hint")}
                  error={save.isError ? refusal(save.error) : undefined}
                >
                  <Input
                    ref={nameInput}
                    value={name}
                    maxLength={80}
                    placeholder={t("savedViews.save.placeholder")}
                    autoComplete="off"
                    onChange={(event) => {
                      setName(event.target.value);
                      if (save.isError) save.reset();
                    }}
                  />
                </Field>
                {save.isError && refusalDetail(save.error) && (
                  <p className="break-words font-mono text-xs text-[color:var(--text-subtle)]">
                    {refusalDetail(save.error)}
                  </p>
                )}
                <Button
                  type="submit"
                  size="sm"
                  className="w-fit"
                  disabled={!name.trim() || save.isPending}
                >
                  {save.isPending ? t("common.saving") : t("savedViews.save.action")}
                </Button>
              </form>

              {views.data.length === 0 ? (
                <EmptyState
                  uxTarget="saved-views"
                  title={t("savedViews.empty.title")}
                  description={t("savedViews.empty.description")}
                  actions={
                    <Button size="sm" variant="outline" onClick={() => nameInput.current?.focus()}>
                      {t("savedViews.empty.action")}
                    </Button>
                  }
                />
              ) : (
                <ul aria-label={t("savedViews.list")} className="flex flex-col gap-2">
                  {views.data.map((view) => (
                    <li
                      key={view.id}
                      className="flex flex-col gap-1.5 rounded-lg border border-[color:var(--border-subtle)] px-3 py-2.5"
                    >
                      {renaming?.id === view.id ? (
                        <form
                          className="flex flex-col gap-2"
                          onSubmit={(event) => {
                            event.preventDefault();
                            if (renaming.name.trim() && !rename.isPending) rename.mutate(renaming);
                          }}
                        >
                          <Input
                            autoFocus
                            value={renaming.name}
                            maxLength={80}
                            aria-label={t("savedViews.renameField", { name: view.name })}
                            aria-invalid={rename.isError || undefined}
                            aria-describedby={describedBy(rename.isError && renameError)}
                            onChange={(event) => {
                              setRenaming({ id: view.id, name: event.target.value });
                              if (rename.isError) rename.reset();
                            }}
                          />
                          <FieldError
                            id={renameError}
                            error={rename.isError ? refusal(rename.error) : undefined}
                          />
                          <div className="flex gap-2">
                            <Button
                              type="submit"
                              size="sm"
                              disabled={!renaming.name.trim() || rename.isPending}
                            >
                              {rename.isPending ? t("common.saving") : t("common.save")}
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => {
                                setRenaming(null);
                                rename.reset();
                              }}
                            >
                              {t("common.cancel")}
                            </Button>
                          </div>
                        </form>
                      ) : (
                        <div className="flex items-center gap-2">
                          <span className="min-w-0 flex-1 truncate text-sm font-medium">
                            {view.name}
                          </span>
                          <Button
                            size="sm"
                            variant="outline"
                            aria-label={t("savedViews.applyAria", { name: view.name })}
                            onClick={() => apply(view)}
                          >
                            {t("savedViews.apply")}
                          </Button>
                          <button
                            type="button"
                            title={t("savedViews.renameAria", { name: view.name })}
                            aria-label={t("savedViews.renameAria", { name: view.name })}
                            onClick={() => {
                              rename.reset();
                              setRenaming({ id: view.id, name: view.name });
                            }}
                            className="flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                          >
                            <Pencil aria-hidden className="h-3.5 w-3.5" />
                          </button>
                          <DeleteIconButton
                            label={t("savedViews.deleteAria", { name: view.name })}
                            onClick={() => setDeleteTarget(view)}
                          />
                        </div>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </SheetBody>
      </Sheet>

      <ConfirmDialog
        name="saved-view-delete"
        open={!!deleteTarget}
        onOpenChange={(next) => {
          if (next) return;
          setDeleteTarget(null);
          remove.reset();
        }}
        title={t("savedViews.confirm.deleteTitle", { name: deleteTarget?.name ?? "" })}
        description={t("savedViews.confirm.deleteBody")}
        confirmLabel={t("savedViews.confirm.deleteConfirm")}
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
          });
        }}
      />
    </>
  );
}
