import * as React from "react";
import { useTranslation } from "react-i18next";

import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

// the shapes a screen shows while its query is in flight (#1180).
//
// before this, 21 screens rendered the untranslated word "Loading…" — a
// sentence that says the request has not answered yet and nothing else. it
// gives the reader no idea whether one row or forty are on the way, and the
// layout jumps the moment the data lands because the placeholder occupied a
// single line. a skeleton shaped like the content it stands in for says both
// things at once, and holds the space so nothing moves.
//
// every shape wraps in one `role="status"` region carrying the translated
// label, so a screen reader hears one announcement instead of one per bar and
// a story can assert the screen is busy without reaching for a class name.

function LoadingRegion({
  className,
  children,
}: {
  className?: string;
  children: React.ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <div role="status" aria-busy="true" aria-label={t("common.loading")} className={className}>
      {children}
    </div>
  );
}

/** stacked rows, for a list or a grid-table body */
export function ListSkeleton({ rows = 4, className }: { rows?: number; className?: string }) {
  return (
    <LoadingRegion className={cn("flex flex-col gap-2", className)}>
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton key={i} height={38} radius={8} />
      ))}
    </LoadingRegion>
  );
}

/** the auto-fill card grid most screens lay their rows out in */
export function CardGridSkeleton({
  cards = 4,
  height = 148,
  min = 360,
  className,
}: {
  cards?: number;
  /** card height, matched to the real card so the grid does not reflow */
  height?: number;
  /** the `minmax()` floor of the grid this stands in for */
  min?: number;
  className?: string;
}) {
  return (
    <LoadingRegion className={cn("grid gap-3.5", className)}>
      {/* inline template rather than a class: the floor differs per screen and
          tailwind cannot generate an arbitrary value from a runtime number */}
      <div
        className="grid gap-3.5"
        style={{ gridTemplateColumns: `repeat(auto-fill,minmax(${min}px,1fr))` }}
      >
        {Array.from({ length: cards }, (_, i) => (
          <Skeleton key={i} height={height} radius={10} />
        ))}
      </div>
    </LoadingRegion>
  );
}

/** label + control pairs, for a settings form */
export function FormSkeleton({ fields = 4, className }: { fields?: number; className?: string }) {
  return (
    <LoadingRegion className={cn("flex flex-col gap-4", className)}>
      {Array.from({ length: fields }, (_, i) => (
        <div key={i} className="flex flex-col gap-1.5">
          <Skeleton width={120} height={11} radius={4} />
          <Skeleton height={34} radius={8} />
        </div>
      ))}
    </LoadingRegion>
  );
}

/** a bordered panel per settings section, for a screen built out of cards */
export function PanelSkeleton({
  panels = 3,
  height = 132,
  className,
}: {
  panels?: number;
  height?: number;
  className?: string;
}) {
  return (
    <LoadingRegion className={cn("flex flex-col gap-3.5", className)}>
      {Array.from({ length: panels }, (_, i) => (
        <Skeleton key={i} height={height} radius={10} />
      ))}
    </LoadingRegion>
  );
}

/** a header bar over row bars, for a real `<table>` or a `ListTable` */
export function TableSkeleton({ rows = 4, className }: { rows?: number; className?: string }) {
  return (
    <LoadingRegion
      className={cn(
        "overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]",
        className,
      )}
    >
      <div className="border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-4 py-[9px]">
        <Skeleton width={180} height={11} radius={4} />
      </div>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="border-b border-[color:var(--border-subtle)] px-4 py-[13px] last:border-b-0">
          <Skeleton height={13} radius={4} />
        </div>
      ))}
    </LoadingRegion>
  );
}

/** the stat-card strip an analytics screen opens with */
export function StatGridSkeleton({
  cards = 4,
  className,
}: {
  cards?: number;
  className?: string;
}) {
  return (
    <LoadingRegion
      className={cn("grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(220px,100%),1fr))]", className)}
    >
      {Array.from({ length: cards }, (_, i) => (
        <Skeleton key={i} height={96} radius={10} />
      ))}
    </LoadingRegion>
  );
}

/** one form control standing in for itself, for a select whose options are still loading */
export function ControlSkeleton({
  width = 164,
  className,
}: {
  width?: number | string;
  className?: string;
}) {
  return (
    <LoadingRegion className={cn("inline-block", className)}>
      <Skeleton width={width} height={32} radius={6} />
    </LoadingRegion>
  );
}
