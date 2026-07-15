import * as React from "react";

import { cn } from "@/lib/utils";

// data-driven table: pass columns + rows. mirrors the Rolter Design System
// display/Table (mono/align/render column options, hover rows).
export interface TableColumn<T> {
  key: string;
  header?: React.ReactNode;
  align?: "left" | "right" | "center";
  mono?: boolean;
  width?: string | number;
  render?: (value: unknown, row: T, index: number) => React.ReactNode;
}

export interface TableProps<T> extends React.HTMLAttributes<HTMLDivElement> {
  columns: TableColumn<T>[];
  data: T[];
  hover?: boolean;
  rowKey?: keyof T;
}

const ALIGN: Record<string, string> = {
  left: "text-left",
  right: "text-right",
  center: "text-center",
};

export function Table<T extends Record<string, unknown>>({
  columns = [],
  data = [],
  hover = true,
  rowKey,
  className,
  ...props
}: TableProps<T>) {
  return (
    <div
      className={cn(
        "w-full overflow-x-auto rounded-lg border border-[color:var(--border-default)]",
        className,
      )}
      {...props}
    >
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr>
            {columns.map((c) => (
              <th
                key={c.key}
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
          {data.map((row, i) => (
            <tr
              key={rowKey ? String(row[rowKey]) : i}
              className={cn(
                "transition-colors [&:last-child>td]:border-b-0",
                hover && "hover:bg-muted",
              )}
            >
              {columns.map((c) => (
                <td
                  key={c.key}
                  className={cn(
                    "border-b border-[color:var(--border-subtle)] px-4 py-2 align-middle text-[color:var(--text-secondary)]",
                    c.mono && "font-mono text-xs text-foreground",
                    c.align ? ALIGN[c.align] : undefined,
                  )}
                >
                  {c.render ? c.render(row[c.key], row, i) : (row[c.key] as React.ReactNode)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
