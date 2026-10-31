import * as React from "react";

import { isEmptyAnswer, type ReadState } from "@/lib/read-state";
import { cn } from "@/lib/utils";

// data-driven table: pass columns + rows (mono/align/render column options,
// hover rows)
export interface TableColumn<T> {
  key: string;
  header?: React.ReactNode;
  align?: "left" | "right" | "center";
  mono?: boolean;
  /**
   * The cell names its row: a `<th scope="row">`, so a screen reader reads the
   * row's other cells under that name. At most one column per table.
   */
  rowHeader?: boolean;
  width?: string | number;
  /**
   * Classes for this column's header and body cells, for what both share:
   * hiding below a container width (`hidden @min-[720px]:table-cell`).
   */
  className?: string;
  /** classes for this column's header cell alone, after `className` */
  headerClassName?: string;
  /**
   * Classes for this column's body cells alone, after `className`: truncating,
   * a colour. A colour belongs here, since a header keeps the header's own.
   */
  cellClassName?: string;
  /**
   * Classes for this column's `<col>`. A table where any column sets one draws a
   * `<colgroup>`, which is what sizes the columns of a `table-fixed` table and
   * hides one by the table's own width.
   */
  colClassName?: string;
  /**
   * Where the cell goes once the row is stacked (`stacked` on the table): grid
   * placement written out whole, since tailwind cannot see a class built from
   * parts. A column with none keeps the cell's place in the grid's flow.
   */
  stackedClassName?: string;
  /** attributes for this column's body cell, such as the `title` of a clipped value */
  cellProps?: (row: T, index: number) => React.TdHTMLAttributes<HTMLTableCellElement>;
  render?: (value: unknown, row: T, index: number) => React.ReactNode;
}

interface TableBaseProps<T> extends React.HTMLAttributes<HTMLDivElement> {
  columns: TableColumn<T>[];
  data: T[];
  hover?: boolean;
  /** the table's name for assistive technology, which a visible title above it does not give it */
  caption?: string;
  rowKey?: keyof T | ((row: T, index: number) => string);
  /**
   * Draw the table with no frame of its own: no border, rounded corners or
   * scroll container, so a screen that owns the scroll area (a log beside its
   * drawer) can put the table straight in it. `className` and the other props
   * then land on the `<table>`. The table is not a keyboard stop of its own, and
   * a row's rule stays under the last row.
   */
  bare?: boolean;
  /**
   * Compact cells for a dense table: tighter padding and monospace text, with
   * the first colour left to each column's `className`. The padding widens at
   * 480px of the table's own width, so the table sits in an `@container`.
   */
  compact?: boolean;
  /**
   * Keep the header in view while the rows scroll. It sticks to the nearest
   * scrolling ancestor, so it is for a `bare` table; the framed one scrolls
   * sideways inside its own frame, which would hold the header to that.
   */
  stickyHeader?: boolean;
  /**
   * Restack each row into a grid below 480px of the table's own width, the
   * header taken out of sight but kept for assistive technology. Columns place
   * their cell with `stackedClassName`; the table sits in an `@container`.
   */
  stacked?: boolean;
  /** classes for the `<table>` itself, such as `table-fixed` or a minimum width */
  tableClassName?: string;
  /**
   * Attributes for a row: `aria-selected` and `onClick` for a selectable one,
   * `className` for the state it is in. The class is added to the row's own.
   */
  rowProps?: (row: T, index: number) => React.HTMLAttributes<HTMLTableRowElement>;
  /**
   * What opens under a row, in a full-width row of its own, or null for a row
   * with nothing open. The detail sits beside the row it describes, not after
   * the table, so a reader never has to work out which row it belongs to.
   */
  detailRow?: (row: T, index: number) => React.ReactNode;
}

// `empty` comes with the read it describes, or not at all: the type is what
// stops a new caller from handing over the placeholder alone
type TableEmptyProps =
  | { empty?: undefined; read?: undefined }
  | {
      /**
       * What to show instead of the rows once the read answered with none
       * (#1180).
       *
       * Without it the table renders its header over nothing at all, which
       * reads as a screen that is still loading rather than one that loaded and
       * found no rows. Rendered in a single full-width cell so the placeholder
       * stays inside the table's border instead of floating beneath it. The cell
       * spans the table, and the placeholder is held to the width of the frame
       * the table scrolls in, so on a phone it is centred on what is visible.
       */
      empty: React.ReactNode;
      /**
       * The read `data` came from — a `useQuery` result as it stands. `empty`
       * renders only once it succeeded: a failed or pending read holds no rows
       * either, and an empty state under its `LoadError` states an outage as a
       * deployment with nothing in it (#2211).
       */
      read: ReadState;
    };

export type TableProps<T> = TableBaseProps<T> & TableEmptyProps;

const ALIGN: Record<string, string> = {
  left: "text-left",
  right: "text-right",
  center: "text-center",
};

// written out whole: tailwind cannot see a class built from parts
const STACKED_TABLE = "@max-[479px]:block";
const STACKED_ROW =
  "@max-[479px]:grid @max-[479px]:grid-cols-[minmax(0,1fr)_auto_1.75rem] @max-[479px]:items-center @max-[479px]:gap-x-2 @max-[479px]:border-b @max-[479px]:border-[color:var(--border-subtle)] @max-[479px]:px-3 @max-[479px]:py-2";
const STACKED_CELL = "@max-[479px]:border-b-0 @max-[479px]:p-0";

export function Table<T extends object>({
  columns = [],
  data = [],
  hover = true,
  caption,
  rowKey,
  bare = false,
  compact = false,
  stickyHeader = false,
  stacked = false,
  tableClassName,
  rowProps,
  detailRow,
  empty,
  read,
  className,
  ...props
}: TableProps<T>) {
  const hasColgroup = columns.some((c) => c.colClassName);
  const keyOf = (row: T, i: number) =>
    typeof rowKey === "function" ? rowKey(row, i) : rowKey ? String(row[rowKey]) : String(i);
  const cellPad = compact
    ? "px-2 py-[9px] font-mono text-xs @min-[480px]:px-3"
    : "px-4 py-2 text-[color:var(--text-secondary)]";
  const table = (
    <table
      className={cn(
        "w-full border-collapse text-sm",
        stacked && STACKED_TABLE,
        tableClassName,
        bare && className,
      )}
      {...(bare ? (props as React.HTMLAttributes<HTMLTableElement>) : undefined)}
    >
      {caption && <caption className="sr-only">{caption}</caption>}
      {hasColgroup && (
        <colgroup>
          {columns.map((c) => (
            <col key={c.key} className={c.colClassName} />
          ))}
        </colgroup>
      )}
      <thead className={cn(stacked && "@max-[479px]:sr-only")}>
        <tr>
          {columns.map((c) => (
            <th
              key={c.key}
              scope="col"
              className={cn(
                "whitespace-nowrap border-b border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] text-xs font-medium text-muted-foreground",
                compact ? "px-2 py-2.5 @min-[480px]:px-3" : "px-4 py-2",
                stickyHeader && "sticky top-0 z-[1]",
                c.align ? ALIGN[c.align] : "text-left",
                c.className,
                c.headerClassName,
              )}
              style={c.width ? { width: c.width } : undefined}
            >
              {c.header}
            </th>
          ))}
        </tr>
      </thead>
      <tbody className={cn(stacked && STACKED_TABLE)}>
        {empty && read && isEmptyAnswer(read, data.length) && (
          <tr>
            <td colSpan={columns.length} className="p-0">
              {/* the cell spans the whole table, which is wider than the
                  frame below the columns' width. this box is the frame's
                  width and sticks to its left edge, so the placeholder is
                  centred on what the reader sees and stays there while the
                  columns scroll beneath it */}
              <div className="sticky left-0 w-[100cqw]">{empty}</div>
            </td>
          </tr>
        )}
        {data.map((row, i) => {
          const detail = detailRow?.(row, i);
          const { className: rowClass, ...rowRest } = rowProps?.(row, i) ?? {};
          return (
            <React.Fragment key={keyOf(row, i)}>
              <tr
                {...rowRest}
                className={cn(
                  "transition-colors",
                  !detail && !bare && "[&:last-child>:is(td,th)]:border-b-0",
                  hover && "hover:bg-muted",
                  stacked && STACKED_ROW,
                  rowClass,
                )}
              >
                {columns.map((c) => {
                  const Cell = c.rowHeader ? "th" : "td";
                  const { className: cellClass, ...cellRest } = c.cellProps?.(row, i) ?? {};
                  return (
                    <Cell
                      key={c.key}
                      scope={c.rowHeader ? "row" : undefined}
                      {...cellRest}
                      className={cn(
                        "border-b border-[color:var(--border-subtle)] align-middle",
                        cellPad,
                        c.mono && "font-mono text-xs text-foreground",
                        // a sized column is sized to its content: the slack goes to
                        // the unsized ones instead of stretching every cell
                        c.width && "whitespace-nowrap",
                        // a row header is a th, which would otherwise be bold and centred
                        c.rowHeader && "font-normal",
                        c.align ? ALIGN[c.align] : c.rowHeader ? "text-left" : undefined,
                        stacked && STACKED_CELL,
                        c.className,
                        c.cellClassName,
                        stacked && c.stackedClassName,
                        cellClass,
                      )}
                    >
                      {c.render
                        ? c.render((row as Record<string, unknown>)[c.key], row, i)
                        : ((row as Record<string, unknown>)[c.key] as React.ReactNode)}
                    </Cell>
                  );
                })}
              </tr>
              {detail && (
                <tr className="[&:last-child>td]:border-b-0">
                  <td
                    colSpan={columns.length}
                    className="border-b border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] p-0"
                  >
                    {/* held to the frame's width, like the empty placeholder, so
                        the detail is read without scrolling the columns */}
                    <div className="sticky left-0 w-[100cqw] max-w-full px-4 py-3">{detail}</div>
                  </td>
                </tr>
              )}
            </React.Fragment>
          );
        })}
      </tbody>
    </table>
  );
  if (bare) return table;
  return (
    <div
      // a scroll container has to be reachable from the keyboard, or the part
      // of the table past the right edge is mouse-only (#1181)
      tabIndex={0}
      className={cn(
        // a size container, so the placeholder can be as wide as what the
        // reader sees (`100cqw`) and not as wide as the table, which scrolls
        // sideways inside this frame below its columns' width (#2420).
        // containment takes the frame's own width off its content: `w-full`
        // gives it one, but a caller that swaps that for `w-auto` in a row
        // flex would collapse it to its border
        "relative w-full overflow-x-auto rounded-lg border border-[color:var(--border-default)] [container-type:inline-size]",
        "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        className,
      )}
      {...props}
    >
      {table}
    </div>
  );
}
