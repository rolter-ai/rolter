import { ArrowDown, ArrowUp, Search } from "lucide-react";
import * as React from "react";

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
  return (
    <div className={cn("relative max-w-[320px] flex-1", className)}>
      <Search className="pointer-events-none absolute left-[11px] top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-[color:var(--text-subtle)]" />
      <input
        className="h-9 w-full rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] pl-[34px] pr-3 text-sm outline-none placeholder:text-[color:var(--text-subtle)]"
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
// over css-grid data rows, columns supplied per screen
export function ListTable({
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn(
        "overflow-hidden rounded-[10px] border border-[color:var(--border-subtle)]",
        className,
      )}
      {...props}
    />
  );
}

export function ListHeader({
  grid,
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { grid: string }) {
  return (
    <div
      className={cn(
        "grid items-center gap-3 border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-4 py-[9px] text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]",
        className,
      )}
      style={{ gridTemplateColumns: grid }}
      {...props}
    />
  );
}

export function ListRow({
  grid,
  className,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { grid: string }) {
  return (
    <div
      className={cn(
        "grid items-center gap-3 border-b border-[color:var(--border-subtle)] px-4 py-[11px] last:border-b-0",
        className,
      )}
      style={{ gridTemplateColumns: grid }}
      {...props}
    />
  );
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
        "flex select-none items-center gap-[3px] uppercase tracking-[0.07em] transition-colors hover:text-[color:var(--text-secondary)]",
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
export function RowIconButton({
  danger,
  className,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { danger?: boolean }) {
  return (
    <button
      type="button"
      className={cn(
        "flex flex-none items-center justify-center rounded-[6px] border border-[color:var(--border-subtle)] bg-transparent p-[5px] transition-colors",
        danger
          ? "text-[color:var(--status-danger)] hover:bg-[color:var(--red-tint)]"
          : "text-muted-foreground hover:text-foreground",
        className,
      )}
      {...props}
    />
  );
}
