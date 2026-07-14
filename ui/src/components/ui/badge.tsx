import { cva, type VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/lib/utils";

// tones mirror the Rolter Design System Badge (7 tones + optional leading dot)
const badgeVariants = cva(
  "inline-flex items-center gap-1 h-5 px-2 rounded border text-[0.625rem] font-medium leading-none whitespace-nowrap",
  {
    variants: {
      tone: {
        neutral: "border-border bg-muted text-muted-foreground",
        outline: "border-border bg-transparent text-muted-foreground",
        success:
          "border-transparent bg-emerald-500/15 text-emerald-600 dark:text-emerald-400",
        warning:
          "border-transparent bg-amber-500/15 text-amber-600 dark:text-amber-400",
        danger: "border-transparent bg-destructive/15 text-destructive",
        info: "border-transparent bg-blue-500/15 text-blue-600 dark:text-blue-400",
        accent:
          "border-transparent bg-orange-500/15 text-orange-600 dark:text-orange-400",
      },
    },
    defaultVariants: { tone: "neutral" },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {
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
