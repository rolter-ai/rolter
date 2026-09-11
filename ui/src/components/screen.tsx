import { ArrowDown, ArrowUp, Search } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { useGate, type Capability } from "@/lib/can";
import { cn } from "@/lib/utils";

// shared building blocks for the control-plane screens ported from the design
// prototype: page body padding, toolbar search, status dots, mono pills, and
// the css-grid list table with sortable headers.

export function PageBody({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("flex flex-col gap-4 p-[22px]", className)} {...props} />
  );
}

export function SearchInput({
  className,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
  const { t } = useTranslation();
  return (
    <div className={cn("relative max-w-[320px] flex-1", className)}>
      <Search className="pointer-events-none absolute left-[11px] top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[color:var(--text-subtle)]" />
      <input
        type="search"
        aria-label={props.placeholder || t("common.search")}
        className="h-9 w-full rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] pl-[34px] pr-3 text-sm outline-none placeholder:text-[color:var(--text-subtle)] transition-colors hover:border-input focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        {...props}
      />
    </div>
  );
}

export type Health = "ok" | "degraded" | "down";

export const HEALTH_COLOR: Record<Health, string> = {
  ok: "var(--status-success)",
  degraded: "var(--status-warning)",
  down: "var(--status-danger)",
};

export function StatusDot({ color, className }: { color: string; className?: string }) {
  return (
    <span
      className={cn("h-[7px] w-[7px] flex-none rounded-full", className)}
      style={{ background: color }}
    />
  );
}

// mono uppercase pill (modality tags, origin tags, strategy tags)
export function Pill({
  color,
  tint,
  border,
  className,
  children,
}: {
  color: string;
  tint?: string;
  border?: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-[6px] px-2 py-0.5 font-mono text-[11px] uppercase tracking-[0.03em]",
        className,
      )}
      style={{
        color,
        background: tint,
        border: border ? `1px solid ${border}` : undefined,
      }}
    >
      {children}
    </span>
  );
}

// the bordered list-table container from the prototype: css-grid header row
// over css-grid data rows, columns supplied per screen.
//
// the columns have a width below which they stop being readable, so the table
// scrolls sideways inside its own border rather than squeezing them or letting
// the page scroll under the whole shell (#1203). `minWidth` is that floor; it
// lands on the rows through a child selector because the header and the rows
// are siblings, not one element the caller could size.
export function ListTable({
  className,
  minWidth = 760,
  style,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { minWidth?: number }) {
  return (
    <div
      // a scroll container has to be reachable from the keyboard, or the part
      // of the row past the right edge is mouse-only (#1181)
      tabIndex={0}
      className={cn(
        "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        // `relative` is load-bearing: the row buttons carry `sr-only` labels,
        // which are absolutely positioned. without a containing block here they
        // resolve against the page and drag the *document* out to the table's
        // unscrolled width, which is the overflow this scroll container exists
        // to prevent
        "relative overflow-x-auto rounded-[10px] border border-[color:var(--border-subtle)] [&>*]:min-w-[var(--rl-list-min-w)]",
        className,
      )}
      style={{ "--rl-list-min-w": `${minWidth}px`, ...style } as React.CSSProperties}
      {...props}
    />
  );
}

// the caller's `style` is merged over the grid template rather than replacing
// it: spreading props after `style` let a row's `style={{ opacity }}` drop the
// template, which stacked every cell of the Keys and Users lists into one column
export function ListHeader({
  grid,
  className,
  style,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { grid: string }) {
  return (
    <div
      className={cn(
        "grid items-center gap-3 border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-4 py-[9px] text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]",
        className,
      )}
      style={{ gridTemplateColumns: grid, ...style }}
      {...props}
    />
  );
}

export function ListRow({
  grid,
  className,
  style,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { grid: string }) {
  return (
    <div
      className={cn(
        "grid items-center gap-3 border-b border-[color:var(--border-subtle)] px-4 py-[11px] last:border-b-0",
        className,
      )}
      style={{ gridTemplateColumns: grid, ...style }}
      {...props}
    />
  );
}

// card grids use `[grid-template-columns:repeat(auto-fill,minmax(min(Npx,100%),1fr))]`:
// the inner min() caps the column minimum at the container width, so a 380px
// card does not force a 375px screen to scroll sideways (#1242)
// the row above a list: a description, a search box, a filter or two and the
// create button pushed to the end with `ml-auto`. it wraps, so a 375px screen
// stacks the button under the search instead of scrolling the page sideways
// (#1242); thirteen screens used to hand-write the same non-wrapping row
export function Toolbar({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("flex flex-wrap items-center gap-3", className)} {...props} />;
}

// tiny asc/desc/off sorter for the grid tables
export function useSort<K extends string>() {
  const [sort, setSort] = React.useState<{ col: K | null; dir: "asc" | "desc" | null }>({
    col: null,
    dir: null,
  });
  const cycle = (col: K) =>
    setSort((s) => {
      if (s.col !== col) return { col, dir: "asc" };
      if (s.dir === "asc") return { col, dir: "desc" };
      return { col: null, dir: null };
    });
  const apply = <T,>(rows: T[], accessors: Record<K, (row: T) => string | number>): T[] => {
    if (!sort.col || !sort.dir) return rows;
    const acc = accessors[sort.col];
    const out = [...rows].sort((a, b) => {
      const av = acc(a);
      const bv = acc(b);
      if (typeof av === "number" && typeof bv === "number") return av - bv;
      return String(av).localeCompare(String(bv));
    });
    if (sort.dir === "desc") out.reverse();
    return out;
  };
  return { sort, cycle, apply };
}

export function SortLabel({
  label,
  col,
  sort,
  onCycle,
  justify = "flex-start",
}: {
  label: string;
  col: string;
  sort: { col: string | null; dir: "asc" | "desc" | null };
  onCycle: (col: string) => void;
  justify?: "flex-start" | "flex-end";
}) {
  const active = sort.col === col && sort.dir != null;
  return (
    <button
      type="button"
      onClick={() => onCycle(col)}
      className={cn(
        "flex select-none items-center gap-[3px] uppercase tracking-[0.07em] transition-colors hover:text-[color:var(--text-secondary)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        active ? "text-[color:var(--text-secondary)]" : "text-[color:var(--text-subtle)]",
      )}
      style={{ justifyContent: justify }}
    >
      {label}
      {active &&
        (sort.dir === "asc" ? (
          <ArrowUp className="h-3 w-3" />
        ) : (
          <ArrowDown className="h-3 w-3" />
        ))}
    </button>
  );
}

// icon-button used across rows (edit / delete / view)
//
// `gate` is the `resource:action` the row control needs, and an icon button
// with no text needs the refusal spelled out in its `title` more than a
// labelled one does, not less (#1258).
export function RowIconButton({
  danger,
  className,
  gate,
  disabled,
  title,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  danger?: boolean;
  gate?: Capability;
}) {
  const { denied, reason } = useGate(gate);
  return (
    <button
      type="button"
      disabled={disabled || denied}
      title={denied ? reason : title}
      className={cn(
        denied && "cursor-not-allowed opacity-50",
        "flex flex-none items-center justify-center rounded-[6px] border border-[color:var(--border-subtle)] bg-transparent p-[5px] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        danger
          ? "text-[color:var(--status-danger-text)] hover:bg-[color:var(--red-tint)]"
          : "text-muted-foreground hover:text-foreground",
        className,
      )}
      {...props}
    />
  );
}
