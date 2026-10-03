import { ArrowDown, ArrowUp, Search } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { useGate, type Capability } from "@/lib/can";
import { isAwaiting, isEmptyAnswer, type ReadState } from "@/lib/read-state";
import { useRefusedClick } from "@/lib/ux-react";
import { cn } from "@/lib/utils";

// shared building blocks for the control-plane screens: page body padding,
// toolbar search, status dots, mono pills, and the css-grid list table with
// sortable headers.

export function PageBody({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("flex flex-col gap-4 p-[22px]", className)} {...props} />;
}

export function SearchInput({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
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
  title,
  children,
}: {
  color: string;
  tint?: string;
  border?: string;
  className?: string;
  /** hover text, for a pill whose label is shorter than what it stands for */
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <span
      title={title}
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

// the bordered list-table container: css-grid header row
// over css-grid data rows, columns supplied per screen.
//
// the columns have a width below which they stop being readable, so the table
// scrolls sideways inside its own border rather than squeezing them or letting
// the page scroll under the whole shell (#1203). `minWidth` is that floor; it
// lands on the header and body rowgroups through a child selector because they
// are siblings, not one element the caller could size.
//
// the css grid only draws the columns, so the roles are what make it a table
// to a screen reader (#2000): this is `role="table"` over two rowgroups, and a
// row's children are cells — `SortLabel`, `ListHeaderCell` or
// `ListActionsHeader` in the header, `ListCell` in the body. a bare span in a
// row is a generic node that is read with no column header, so it has no place
// there. `label` names the table, and since the table is also the focusable
// scroller, it is what focus announces.
//
// `ListHeader` is its own rowgroup; every other child — the rows, a loading or
// empty `ListStateRow` — is put in the body rowgroup here, the way a browser
// puts a `<tbody>` round rows written straight into a `<table>`, so no caller
// can leave it out. the header has to be a direct child to be told apart
export function ListTable({
  label,
  className,
  minWidth = 760,
  style,
  children,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { label: string; minWidth?: number }) {
  const parts = React.Children.toArray(children);
  const isHeader = (part: React.ReactNode) =>
    React.isValidElement(part) && part.type === ListHeader;
  return (
    <div
      role="table"
      aria-label={label}
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
    >
      {parts.filter(isHeader)}
      <div role="rowgroup">{parts.filter((part) => !isHeader(part))}</div>
    </div>
  );
}

// the header band: a rowgroup around the one row of column headers. the
// caller's `className` lands on the band, since the band is what a caller
// positions — McpLogs pins it with `sticky top-0`, and a sticky row inside a
// rowgroup of its own height would have nowhere to stick
export function ListHeader({
  grid,
  className,
  children,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { grid: string }) {
  return (
    <div
      role="rowgroup"
      className={cn(
        "border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]",
        className,
      )}
      {...props}
    >
      <div
        role="row"
        className="grid items-center gap-3 px-4 py-[9px]"
        style={{ gridTemplateColumns: grid }}
      >
        {children}
      </div>
    </div>
  );
}

// the caller's `style` is merged over the grid template rather than replacing
// it: spreading props after `style` let a row's `style={{ opacity }}` drop the
// template, which stacked every cell of the Keys and Users lists into one column
export function ListRow({
  grid,
  className,
  style,
  ...props
}: React.HTMLAttributes<HTMLDivElement> & { grid: string }) {
  return (
    <div
      role="row"
      className={cn(
        "grid items-center gap-3 border-b border-[color:var(--border-subtle)] px-4 py-[11px] last:border-b-0",
        className,
      )}
      style={{ gridTemplateColumns: grid, ...style }}
      {...props}
    />
  );
}

// a header cell and a body cell. each takes the place of the span or div a row
// used to hold, classes and all, so the cell is the grid item and nothing
// moves. a cell that wraps a component instead (a Pill, a Badge, a combobox)
// passes `grid`: the component is then laid out exactly as it was when it sat
// in the row's grid itself, stretched across its column
export function ListHeaderCell(props: React.HTMLAttributes<HTMLDivElement>) {
  return <div role="columnheader" {...props} />;
}

export function ListCell(props: React.HTMLAttributes<HTMLDivElement>) {
  return <div role="cell" {...props} />;
}

// the header over a row's buttons shows no text, but a column header with no
// name is announced as an empty column (axe `empty-table-header`), so it says
// what the column holds to a screen reader. a table whose buttons take more
// than one column passes `label` so each column has its own name
export function ListActionsHeader({ label }: { label?: string }) {
  const { t } = useTranslation();
  return (
    <ListHeaderCell>
      <span className="sr-only">{label ?? t("common.rowActions")}</span>
    </ListHeaderCell>
  );
}

// what the body shows in place of rows — the loading skeleton, the empty
// state — as one row holding one cell the width of the table. a skeleton's
// `role="status"` or an empty state's button placed straight in the rowgroup
// is content no row owns, which a screen reader reads outside the table and
// axe fails as `aria-required-children`
export function ListStateRow({ children }: { children: React.ReactNode }) {
  return (
    <div role="row">
      <div role="cell">{children}</div>
    </div>
  );
}

// the two state rows a list screen writes, each deciding from the read itself
// rather than from the rows (#2211). the rows cannot tell a list still coming
// or a read that failed from one that answered with nothing: all three hold an
// empty array, and `!query.isLoading && rows.length === 0` put "No providers
// yet" and its create button under the list's own `LoadError`.
//
// `read` is the screen's `useQuery` result. the loading row shows while the
// read is awaiting an answer, a parked retry included; the empty row only once
// it succeeded, and `rows` is what survived the screen's filters, so a search
// that matched nothing still gets its no-match copy
export function ListLoadingRow({ read, children }: { read: ReadState; children: React.ReactNode }) {
  return isAwaiting(read) ? <ListStateRow>{children}</ListStateRow> : null;
}

export function ListEmptyRow({
  read,
  rows,
  children,
}: {
  read: ReadState;
  rows: number;
  children: React.ReactNode;
}) {
  return isEmptyAnswer(read, rows) ? <ListStateRow>{children}</ListStateRow> : null;
}

// the count a screen states beside its list — "3 teams", "12 connectors". it
// renders only while the data it counts is held: `data?.length ?? 0` said
// "0 teams" while the read was in flight and again after it failed (#2211).
// `children` is handed the data instead of the caller reading it, so there is
// no `?? 0` left to write. a failed refetch keeps the rows on screen, and the
// count with them, so it keys on the data and not on `isSuccess`. `fallback` is
// for a summary that also explains the screen: the explanation without the
// count stays up while the count is unknown
export function ListSummary<T>({
  data,
  fallback,
  className,
  children,
}: {
  data: T | undefined;
  fallback?: React.ReactNode;
  className?: string;
  children: (data: T) => React.ReactNode;
}) {
  const content = data === undefined ? fallback : children(data);
  if (content === undefined || content === null) return null;
  return <span className={cn("text-sm text-muted-foreground", className)}>{content}</span>;
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

// a sortable column header, cell and button in one. the sort state is
// `aria-sort` on the header, where a screen reader announces it with the
// column, and a sortable column with no sort applied says `none`. the button's
// name is the column's label, already translated by the caller: an
// `aria-label` such as "sort by name" would become the header's name too, and
// that is read before every cell of the column. the arrow draws the same state
// for the eye and is hidden, so it is not read as an unlabelled image (#2000)
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
  const ariaSort = !active ? "none" : sort.dir === "asc" ? "ascending" : "descending";
  return (
    <div role="columnheader" aria-sort={ariaSort}>
      <button
        type="button"
        onClick={() => onCycle(col)}
        className={cn(
          // `w-full`: the button used to be the grid item and stretched across
          // the column; inside the header cell it has to ask, or `justify`
          // has no width to push the label to the right edge in
          "flex w-full select-none items-center gap-[3px] uppercase tracking-[0.07em] transition-colors hover:text-[color:var(--text-secondary)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          active ? "text-[color:var(--text-secondary)]" : "text-[color:var(--text-subtle)]",
        )}
        style={{ justifyContent: justify }}
      >
        {label}
        {active &&
          (sort.dir === "asc" ? (
            <ArrowUp aria-hidden="true" className="h-3 w-3" />
          ) : (
            <ArrowDown aria-hidden="true" className="h-3 w-3" />
          ))}
      </button>
    </div>
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
  control = "row-action",
  disabled,
  title,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  danger?: boolean;
  gate?: Capability;
  /**
   * names this control in the UX stream when it is refused (#1731) — a stable
   * slug, required so a new row action cannot forget it (#1750)
   */
  control: string;
}) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  return (
    <span className="contents" {...refusal}>
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
    </span>
  );
}
