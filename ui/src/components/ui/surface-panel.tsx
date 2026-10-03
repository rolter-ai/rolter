import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * A bordered panel on the surface-card ground, with its own header (#1711).
 *
 * `TwoFactorPanel`, `SingleSignOn` and `UserProvisioning` each opened a
 * `section` with these classes and laid a header and body inside. It owns the
 * frame only; the header and body stay with the caller, because they differ
 * (an icon, a divider, an action).
 */
export function SurfacePanel({ className, ...props }: React.HTMLAttributes<HTMLElement>) {
  return (
    <section
      className={cn(
        "rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-card)]",
        className,
      )}
      {...props}
    />
  );
}
