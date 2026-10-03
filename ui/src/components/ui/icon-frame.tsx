import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * The 34px square frame around an icon beside a row title (#1711).
 *
 * `Connectors`, `Alerting`, `Teams`, `ComplexityRouter` and `Rbac` carried the
 * same ten classes. The icon takes the secondary text colour; a caller that
 * wants another one (a danger-tinted router) passes a `text-*` class.
 */
export function IconFrame({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      className={cn(
        "flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--text-secondary)]",
        className,
      )}
      {...props}
    />
  );
}
