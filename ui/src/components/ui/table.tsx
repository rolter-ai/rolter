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
  render?: (value: unknown, row: T, index: number) => React.ReactNode;
}

interface TableBaseProps<T> extends React.HTMLAttributes<HTMLDivElement> {
  columns: TableColumn<T>[];
  data: T[];
  hover?: boolean;
  /** the table's name for assistive technology, which a visible title above it does not give it */
  caption?: string;
  rowKey?: keyof T;
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

export function Table<T extends Record<string, unknown>>({
  columns = [],
  data = [],
  hover = true,
  caption,
  rowKey,
  detailRow,
  empty,
  read,
  className,
  ...props
}: TableProps<T>) {
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
      <table className="w-full border-collapse text-sm">
        {caption && <caption className="sr-only">{caption}</caption>}
        <thead>
          <tr>
            {columns.map((c) => (
              <th
                key={c.key}
                scope="col"
                className={cn(
                  "whitespace-nowrap border-b border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-2 text-xs font-medium text-muted-foreground",
                  c.align ? ALIGN[c.align] : "text-left",
                )}
                style={c.width ? { width: c.width } : undefined}
              >
                {c.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
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
            return (
              <React.Fragment key={rowKey ? String(row[rowKey]) : i}>
                <tr
                  className={cn(
                    "transition-colors",
                    !detail && "[&:last-child>td]:border-b-0",
                    hover && "hover:bg-muted",
                  )}
                >
                  {columns.map((c) => {
                    const Cell = c.rowHeader ? "th" : "td";
                    return (
                      <Cell
                        key={c.key}
                        scope={c.rowHeader ? "row" : undefined}
                        className={cn(
                          "border-b border-[color:var(--border-subtle)] px-4 py-2 align-middle text-[color:var(--text-secondary)]",
                          c.mono && "font-mono text-xs text-foreground",
                          // a sized column is sized to its content: the slack goes to
                          // the unsized ones instead of stretching every cell
                          c.width && "whitespace-nowrap",
                          // a row header is a th, which would otherwise be bold and centred
                          c.rowHeader && "font-normal",
                          c.align ? ALIGN[c.align] : c.rowHeader ? "text-left" : undefined,
                        )}
                      >
                        {c.render ? c.render(row[c.key], row, i) : (row[c.key] as React.ReactNode)}
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
    </div>
  );
}
