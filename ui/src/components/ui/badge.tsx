import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/lib/utils";

// 7 tones + optional leading dot.
// every tone reads its colour from the design tokens: the /15 tint comes off
// the status fill hue, the label off the matching --status-*-text token that is
// contrast-checked against both the tint and --surface-base (#1199). the theme
// is dark-only, so there are no tailwind `dark` variants here — see
// docs/dev-docs/development/dashboard-theme.md
// the tone classes, shared with `Pill` (components/screen.tsx), so a status
// tint has one source whichever shape carries it (#1989)
// the label is `text-2xs`, 11px: the floor for any text in the dashboard, so a
// badge can carry data (a price, a cap) and not only a status word (#2230)
export const TONE_CLASSES = {
  neutral: "border-border bg-muted text-muted-foreground",
  outline: "border-border bg-transparent text-muted-foreground",
  success:
    "border-transparent bg-[color:var(--status-success)]/15 text-[color:var(--status-success-text)]",
  warning:
    "border-transparent bg-[color:var(--status-warning)]/15 text-[color:var(--status-warning-text)]",
  danger:
    "border-transparent bg-[color:var(--status-danger)]/15 text-[color:var(--status-danger-text)]",
  info: "border-transparent bg-[color:var(--status-info)]/15 text-[color:var(--status-info-text)]",
  // accent is not a status: it borrows the folkloric red thread, in the
  // text half of it — --red-500 on the tint is 4.16:1 (#1181)
  accent: "border-transparent bg-[color:var(--red-tint)] text-[color:var(--red-folk-text)]",
} as const;

export type BadgeTone = keyof typeof TONE_CLASSES;

const badgeVariants = cva(
  "inline-flex items-center gap-1 h-5 px-2 rounded-sm border text-2xs font-medium leading-none whitespace-nowrap",
  {
    variants: { tone: TONE_CLASSES },
    defaultVariants: { tone: "neutral" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {
  dot?: boolean;
}

export function Badge({ tone, dot = false, className, children, ...props }: BadgeProps) {
  return (
    <span className={cn(badgeVariants({ tone, className }))} {...props}>
      {dot && <span className="h-1.5 w-1.5 rounded-full bg-current" />}
      {children}
    </span>
  );
}

/**
 * The verdict of an HTTP status: a 4xx is the caller's to fix, a 5xx (and 0,
 * a request that got no answer at all) is the gateway's or the upstream's.
 */
export function httpStatusTone(status: number): "success" | "warning" | "danger" {
  if (status === 0 || status >= 500) return "danger";
  if (status >= 400) return "warning";
  return "success";
}

/**
 * An HTTP status in its verdict's tone: the one pill the Dashboard's recent
 * requests and the LLM Logs table and drawer share (#1989). `children`
 * replaces the number, for a status 0 the reader is told about in words.
 */
export function HttpStatusBadge({
  status,
  className,
  children,
}: {
  status: number;
  className?: string;
  children?: React.ReactNode;
}) {
  return (
    <Badge
      tone={httpStatusTone(status)}
      className={cn("text-2xs font-semibold", children == null && "font-mono", className)}
    >
      {children ?? status}
    </Badge>
  );
}

export { badgeVariants };
