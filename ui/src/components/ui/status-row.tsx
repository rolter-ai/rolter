import * as React from "react";

import { cn } from "@/lib/utils";

// colored ◆ glyph + uppercase label activity row. pending/running states gently
// breathe the diamond. mirrors the Rolter Design System feedback/StatusRow.
export type StatusKind =
  | "pending"
  | "running"
  | "success"
  | "error"
  | "warning"
  | "info"
  | "idle";

export interface StatusRowProps extends React.HTMLAttributes<HTMLElement> {
  status?: StatusKind;
  label: React.ReactNode;
  chevron?: boolean;
  colorText?: boolean;
  onClick?: React.MouseEventHandler<HTMLElement>;
}

const COLORS: Record<StatusKind, string> = {
  pending: "var(--status-warning)",
  running: "var(--status-info)",
  success: "var(--status-success)",
  error: "var(--status-danger)",
  warning: "var(--status-warning)",
  info: "var(--status-info)",
  idle: "var(--text-subtle)",
};

export function StatusRow({
  status = "idle",
  label,
  chevron = true,
  colorText = false,
  onClick,
  className,
  ...props
}: StatusRowProps) {
  const interactive = typeof onClick === "function";
  const color = COLORS[status] ?? COLORS.idle;
  const breathe = status === "pending" || status === "running";
  const Tag = (interactive ? "button" : "div") as React.ElementType;
  return (
    <Tag
      className={cn(
        "flex w-full items-center gap-1.5 border-none bg-transparent px-0 py-0.5 text-left text-[color:var(--text-secondary)]",
        interactive ? "cursor-pointer hover:opacity-80" : "cursor-default",
        className,
      )}
      onClick={onClick}
      type={interactive ? "button" : undefined}
      {...props}
    >
      <span
        className={cn("flex-none text-[10px] leading-none", breathe && "animate-pulse")}
        style={{ color }}
        aria-hidden
      >
        ◆
      </span>
      <span
        className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-xs uppercase tracking-wide"
        style={colorText ? { color } : undefined}
      >
        {label}
      </span>
      {chevron && (
        <span className="ml-auto flex-none text-[9px] text-[color:var(--text-subtle)]" aria-hidden>
          ▸
        </span>
      )}
    </Tag>
  );
}
