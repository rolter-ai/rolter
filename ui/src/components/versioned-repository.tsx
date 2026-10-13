import { Check, ChevronDown, Clock3, Diff, RotateCcw } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LoadingRegion } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useFormat } from "@/lib/i18n/format";
import { cn } from "@/lib/utils";

// the kit the Prompt Repository and the Skills Repository are both built from
// (#2221): a named thing with immutable versions, one of which is live.
//
// the two screens used to carry a copy each of the index, the version rail, the
// make-live confirmation and the delete-by-slug dialog, and five issues were the
// same fix twice. what differs between them stays with the screen: the editor,
// the copy that names the thing (a template is applied by the gateway, a skill is
// resolved by callers) and what a make-live confirmation says about its impact.
// everything that is the same lives here once, the version comparison (#2112)
// and the phone layout (#3118) included

export type LiveAction = "publish" | "rollback";

// publishing and rolling back move the same pointer and differ only in the
// audit row, so the verb follows the direction: a version newer than the live
// one is published, an older one is rolled back to, and with nothing live every
// version is a publish (#2110)
export function liveAction(version: number, published?: number | null): LiveAction {
  return version > (published ?? 0) ? "publish" : "rollback";
}

/**
 * The value held through the render that closes a dialog.
 *
 * A confirmation's `name` keys its UX stream rows, and its landing or its
 * abandon is reported on the render that closes it, by which time the parent has
 * already let go of the request it was opened with. Latching it keeps the title,
 * the direction and the stream name those of the request that was confirmed.
 */
export function useLatched<T>(value: T | undefined): T | undefined {
  const [latched, setLatched] = React.useState(value);
  if (value !== undefined && value !== latched) setLatched(value);
  return value ?? latched;
}

export function SectionHeading({
  id,
  title,
  description,
  action,
}: {
  /** the heading's id, for a section that names itself by it */
  id?: string;
  title: string;
  description: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="mb-3 flex items-start justify-between gap-3">
      <div>
        <h3 id={id} className="text-sm font-semibold">
          {title}
        </h3>
        <p className="mt-0.5 text-xs leading-5 text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  );
}

/**
 * The three-column workspace the screens load into, which is layout-specific
 * enough that none of the named shapes in LoadingState.tsx fits. The region is
 * not optional: bare `Skeleton`s are `aria-hidden`, so without it a screen
 * reader heard nothing at all while the list was out (#1605)
 */
export function RepositoryLoadingState() {
  return (
    <LoadingRegion className="grid gap-4 p-5 lg:grid-cols-[15rem_minmax(0,1fr)_17rem]">
      <Skeleton width="100%" height={460} radius={12} />
      <Skeleton width="100%" height={620} radius={12} />
      <Skeleton width="100%" height={460} radius={12} />
    </LoadingRegion>
  );
}

/**
 * The page: the index, the workbench and the version rail in one grid.
 *
 * From `lg` the rail sits under the workbench (and from `xl` beside it). On a
 * phone it comes *before* the workbench, folded to one line, so the history is
 * reachable without scrolling past the whole editor (#3118).
 */
export function RepositoryShell({
  index,
  workbench,
  rail,
  wide,
  children,
}: {
  index: React.ReactNode;
  workbench: React.ReactNode;
  /** a `VersionRail`; absent while no row is selected */
  rail?: React.ReactNode;
  /** the workbench takes the rail's column too, for the empty state */
  wide?: boolean;
  /** dialogs and prompts, outside the grid */
  children?: React.ReactNode;
}) {
  return (
    <div className="h-full min-h-0 overflow-y-auto p-4 sm:p-5">
      <div className="mx-auto grid min-h-full max-w-[1500px] gap-4 lg:grid-cols-[15rem_minmax(0,1fr)] xl:grid-cols-[14rem_minmax(0,1fr)_15rem] 2xl:grid-cols-[15rem_minmax(0,1fr)_17rem]">
        {index}
        <div
          className={cn(
            "flex min-w-0 flex-col max-lg:order-3 [&>*]:flex-1",
            wide && "xl:col-span-2",
          )}
        >
          {workbench}
        </div>
        {rail}
      </div>
      {children}
    </div>
  );
}

export interface RepositoryIndexItem {
  id: string;
  name: string;
  slug: string;
  /** the version the row is at, or "draft": already set as a node */
  badge: React.ReactNode;
}

/** the list of rows, one button each, with the create control in its header */
export function RepositoryIndex({
  label,
  count,
  empty,
  create,
  items,
  selectedId,
  onSelect,
}: {
  label: string;
  /** already worded, "3 in this org" */
  count: string;
  empty: string;
  /**
   * the create control, a `GatedButton` the screen draws: each gated control
   * names itself with a literal `control` slug at its own call site (#1750)
   */
  create: React.ReactNode;
  items: RepositoryIndexItem[];
  selectedId?: string;
  onSelect: (id: string) => void;
}) {
  return (
    <aside
      aria-label={label}
      className="overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]"
    >
      <div className="flex items-center justify-between border-b border-[color:var(--border-subtle)] px-3 py-2.5">
        <div>
          <p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">
            {label}
          </p>
          <p className="mt-0.5 text-xs text-muted-foreground">{count}</p>
        </div>
        {create}
      </div>
      <div className="max-h-[26rem] overflow-y-auto p-1.5 lg:max-h-[calc(100vh-14rem)]">
        {items.length === 0 ? (
          <p className="px-2 py-5 text-center text-xs text-muted-foreground">{empty}</p>
        ) : (
          items.map((item) => (
            <button
              key={item.id}
              type="button"
              aria-current={selectedId === item.id ? "true" : undefined}
              onClick={() => onSelect(item.id)}
              className={cn(
                "w-full rounded-lg px-2.5 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                selectedId === item.id
                  ? "bg-[color:var(--surface-selected)] text-foreground"
                  : "text-[color:var(--text-secondary)] hover:bg-[color:var(--surface-hover)] hover:text-foreground",
              )}
            >
              <span className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate text-sm font-medium">{item.name}</span>
                {item.badge}
              </span>
              <span className="mt-0.5 block truncate font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
                {item.slug}
              </span>
            </button>
          ))
        )}
      </div>
    </aside>
  );
}

/** the badge an index row ends in: the live version, or "draft" before the first publish */
export function VersionBadge({ version, draft }: { version?: number | null; draft: string }) {
  return version ? (
    <span className="text-[0.6875rem] tabular-nums text-[color:var(--status-success-text)]">
      v{version}
    </span>
  ) : (
    <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">{draft}</span>
  );
}

/**
 * The version history: one row per immutable version, the live one badged, and
 * a publish or roll back on every other.
 *
 * Below `lg` it folds to a single line (the selected version and how many there
 * are) that opens the list, and the shell puts it above the editor, so on a
 * phone the history is one tap away instead of a scroll past everything else
 * (#3118). From `lg` it is the static panel it always was.
 */
export function VersionRail<V extends { version: number; created_at: string }>({
  label,
  hint,
  versions,
  publishedVersion,
  selectedVersion,
  loading,
  loadingTestId,
  emptyText,
  renderMeta,
  renderMakeLive,
  onSelect,
}: {
  label: string;
  hint: string;
  versions: V[];
  publishedVersion?: number | null;
  selectedVersion?: number;
  loading: boolean;
  loadingTestId: string;
  emptyText: string;
  /** what a version's row says under its date: counts, a source, scopes */
  renderMeta: (version: V) => React.ReactNode;
  /**
   * the publish or roll back control of a version that is not the live one,
   * a `GatedButton` the screen draws with its own literal `control` slug, or
   * nothing while the row is retired. Its label is `MakeLiveLabel`
   */
  renderMakeLive: (version: number, action: LiveAction) => React.ReactNode;
  onSelect: (version: number) => void;
}) {
  const { t } = useTranslation();
  const format = useFormat();
  const bodyId = React.useId();
  const [open, setOpen] = React.useState(false);
  return (
    <aside
      aria-label={label}
      className="overflow-hidden rounded-xl border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)] max-lg:order-2 lg:col-start-2 xl:col-start-3 xl:row-start-1"
    >
      <button
        type="button"
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setOpen((current) => !current)}
        className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring lg:hidden"
      >
        <span className="min-w-0">
          <span className="block text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">
            {label}
          </span>
          <span className="mt-1 block truncate text-xs text-muted-foreground">
            {selectedVersion === undefined
              ? emptyText
              : t("repository.historySummary", {
                  count: versions.length,
                  version: selectedVersion,
                })}
          </span>
        </span>
        <ChevronDown
          aria-hidden
          className={cn(
            "h-4 w-4 shrink-0 text-[color:var(--text-subtle)] transition-transform duration-[120ms]",
            open && "rotate-180",
          )}
        />
      </button>
      <div className="hidden border-b border-[color:var(--border-subtle)] px-4 py-3 lg:block">
        <p className="text-xs font-semibold uppercase tracking-[0.12em] text-[color:var(--text-subtle)]">
          {label}
        </p>
        <p className="mt-1 text-xs text-muted-foreground">{hint}</p>
      </div>
      <div
        id={bodyId}
        className={cn(
          "max-h-[30rem] space-y-1 overflow-y-auto p-2 xl:max-h-[calc(100vh-14rem)]",
          !open && "max-lg:hidden",
          open && "max-lg:border-t max-lg:border-[color:var(--border-subtle)]",
        )}
      >
        {loading ? (
          <LoadingRegion>
            <Skeleton width="100%" height={180} radius={8} data-testid={loadingTestId} />
          </LoadingRegion>
        ) : versions.length === 0 ? (
          <p className="px-2 py-5 text-center text-xs text-muted-foreground">{emptyText}</p>
        ) : (
          versions.map((version) => {
            const published = publishedVersion === version.version;
            const action = liveAction(version.version, publishedVersion);
            return (
              <div
                key={version.version}
                className={cn(
                  "rounded-lg border p-2.5",
                  selectedVersion === version.version
                    ? "border-[color:var(--red-folk)] bg-[color:var(--surface-selected)]"
                    : "border-transparent hover:bg-[color:var(--surface-hover)]",
                )}
              >
                <button
                  type="button"
                  className="w-full rounded-md text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  aria-pressed={selectedVersion === version.version}
                  onClick={() => onSelect(version.version)}
                >
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-sm font-semibold tabular-nums">v{version.version}</span>
                    {published && <Badge tone="success">{t("repository.published")}</Badge>}
                  </div>
                  <p className="mt-1.5 flex items-center gap-1 text-[0.6875rem] text-muted-foreground">
                    <Clock3 className="h-3 w-3" />
                    {format.date(version.created_at, { dateStyle: "medium", timeStyle: "short" })}
                  </p>
                  {renderMeta(version)}
                </button>
                {!published && renderMakeLive(version.version, action)}
              </div>
            );
          })
        )}
      </div>
    </aside>
  );
}

/** the icon and words of a publish or roll back control, the same wherever one is drawn */
export function MakeLiveLabel({
  action,
  version,
  size = "md",
}: {
  action: LiveAction;
  version: number;
  size?: "sm" | "md";
}) {
  const { t } = useTranslation();
  const icon = size === "sm" ? "h-3.5 w-3.5" : "h-4 w-4";
  return action === "publish" ? (
    <>
      <Check className={icon} /> {t("repository.publishVersion", { version })}
    </>
  ) : (
    <>
      <RotateCcw className={icon} /> {t("repository.rollbackTo", { version })}
    </>
  );
}

/** the line a version's row ends in, in the muted small type every rail uses */
export function VersionMeta({ children }: { children: React.ReactNode }) {
  return (
    <p className="mt-1 flex items-center gap-1 text-[0.6875rem] text-[color:var(--text-subtle)]">
      {children}
    </p>
  );
}

/**
 * The one confirmation for making a version live, forwards or back.
 *
 * It does not know what the thing is: the screen words the title and the
 * body, and puts what the move changes in `children` (the prompt repository's
 * impact on callers, the comparison with the live version).
 */
export function MakeLiveDialog({
  uxName,
  open,
  action,
  version,
  title,
  description,
  pending,
  error,
  onOpenChange,
  onConfirm,
  children,
}: {
  /** the stem of the dialog's UX stream key, `prompt-template` for `prompt-template-publish` */
  uxName: string;
  open: boolean;
  /** latched through the closing render, see `useLatched` */
  action?: LiveAction;
  version?: number;
  title: string;
  description: React.ReactNode;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  children?: React.ReactNode;
}) {
  const { t } = useTranslation();
  const publishing = action !== "rollback";
  return (
    <ConfirmDialog
      name={`${uxName}-${publishing ? "publish" : "rollback"}`}
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={description}
      confirmLabel={
        publishing
          ? t("repository.publishVersion", { version })
          : t("repository.rollbackTo", { version })
      }
      tone="default"
      pending={pending}
      error={error}
      onConfirm={onConfirm}
    >
      {children}
    </ConfirmDialog>
  );
}

/**
 * What differs between the live version and another, under one heading.
 *
 * In a make-live confirmation it is folded behind a button, so the impact the
 * screen states stays the first thing read; in the compare dialog it is open.
 * With nothing different it says so and shows no empty diff.
 */
export function ChangesPanel({
  live,
  target,
  changed,
  collapsible = false,
  children,
}: {
  live: number;
  target: number;
  /** how many differences there are, 0 for two identical versions */
  changed: number;
  collapsible?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(!collapsible);
  const id = React.useId();
  const headingId = React.useId();
  return (
    <section aria-labelledby={headingId} className="space-y-2">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h3 id={headingId} className="text-xs font-medium text-muted-foreground">
            {t("repository.changes.title", { live, version: target })}
          </h3>
          <p className="mt-0.5 text-sm">
            {changed === 0
              ? t("repository.changes.identical", { live, version: target })
              : t("repository.changes.count", { count: changed })}
          </p>
        </div>
        {collapsible && changed > 0 && (
          <Button
            variant="outline"
            aria-expanded={open}
            aria-controls={id}
            onClick={() => setOpen((current) => !current)}
          >
            {open ? t("repository.changes.hide") : t("repository.changes.show")}
          </Button>
        )}
      </div>
      {changed > 0 && open && (
        <div id={id} className="space-y-4">
          {children}
        </div>
      )}
    </section>
  );
}

/** the control that opens the comparison of a version with the live one */
export function CompareButton({ live, onClick }: { live: number; onClick: () => void }) {
  const { t } = useTranslation();
  return (
    <Button variant="ghost" onClick={onClick}>
      <Diff className="h-4 w-4" /> {t("repository.compare.action", { live })}
    </Button>
  );
}

/** the comparison on its own, for a version that is not about to be made live */
export function VersionCompareDialog({
  open,
  live,
  target,
  onOpenChange,
  children,
}: {
  open: boolean;
  live?: number;
  target?: number;
  onOpenChange: (open: boolean) => void;
  children: React.ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg">
      <DialogHeader>
        <DialogTitle>{t("repository.compare.title", { live, version: target })}</DialogTitle>
        <DialogDescription>{t("repository.compare.description", { live })}</DialogDescription>
      </DialogHeader>
      <DialogBody className="space-y-4">{children}</DialogBody>
    </Dialog>
  );
}

/**
 * Deleting takes every immutable version with it, so the operator retypes the
 * slug rather than let one stray click drop live content. The screen words the
 * title and the consequence.
 */
export function DeleteBySlugDialog({
  uxName,
  open,
  slug,
  title,
  description,
  slugLabel,
  confirmLabel,
  pending,
  error,
  onOpenChange,
  onConfirm,
}: {
  uxName: string;
  open: boolean;
  slug?: string;
  title: string;
  description: React.ReactNode;
  slugLabel: string;
  confirmLabel: string;
  pending: boolean;
  error: Error | null;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const [confirmation, setConfirmation] = React.useState("");
  React.useEffect(() => {
    if (!open) setConfirmation("");
  }, [open]);
  return (
    <ConfirmDialog
      name={uxName}
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      description={description}
      confirmLabel={confirmLabel}
      pending={pending}
      error={error}
      confirmDisabled={confirmation.trim() !== slug}
      onConfirm={onConfirm}
    >
      <Field label={slugLabel}>
        <Input
          value={confirmation}
          onChange={(event) => setConfirmation(event.target.value)}
          placeholder={slug}
        />
      </Field>
    </ConfirmDialog>
  );
}
