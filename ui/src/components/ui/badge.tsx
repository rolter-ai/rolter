import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/lib/utils";

// tones mirror the Rolter Design System Badge (7 tones + optional leading dot).
// every tone reads its colour from the design tokens: the /15 tint comes off
// the status fill hue, the label off the matching --status-*-text token that is
// contrast-checked against both the tint and --surface-base (#1199). the theme
// is dark-only, so there are no tailwind `dark` variants here — see
// docs/dev-docs/development/dashboard-theme.md
const badgeVariants = cva(
  "inline-flex items-center gap-1 h-5 px-2 rounded-sm border text-[0.625rem] font-medium leading-none whitespace-nowrap",
  {
    variants: {
      tone: {
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
      },
    },
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

export { badgeVariants };
