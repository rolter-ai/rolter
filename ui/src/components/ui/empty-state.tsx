import * as React from "react";

import { cn } from "@/lib/utils";
import { useEmptyState } from "@/lib/ux-react";

// centered empty/zero-data placeholder. one of the two sanctioned places the
// folkloric вышивка thread is allowed to show — a quiet cross-stitch rule under
// the message. mirrors the Rolter Design System feedback/EmptyState.
export interface EmptyStateProps extends Omit<React.HTMLAttributes<HTMLDivElement>, "title"> {
  icon?: React.ReactNode;
  title?: React.ReactNode;
  description?: React.ReactNode;
  actions?: React.ReactNode;
  thread?: boolean;
  /**
   * Stable name of the list that came back empty (`virtual-keys`, `plugins`),
   * recorded on the `empty_state` UX event (#805).
   *
   * Deliberately a separate prop rather than something derived from `title`:
   * the title is prose written for a human, and deriving a key from it would
   * put a sentence into a `LowCardinality` column. Omitting it still records
   * that *a* placeholder was shown on this screen.
   */
  uxTarget?: string;
}

export function EmptyState({
  icon,
  title,
  description,
  actions,
  thread = true,
  uxTarget,
  className,
  ...props
}: EmptyStateProps) {
  // no-ops outside a UxScreenProvider, which is what Storybook and the tests get
  useEmptyState(uxTarget);
  return (
    <div
      className={cn(
        "flex flex-col items-center gap-3 px-4 py-12 text-center text-muted-foreground",
        className,
      )}
      {...props}
    >
      {icon && (
        <span className="inline-flex h-11 w-11 items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-muted-foreground [&>svg]:h-5 [&>svg]:w-5">
          {icon}
        </span>
      )}
      {title && <div className="text-sm font-medium text-foreground">{title}</div>}
      {description && (
        <div className="max-w-[36ch] text-sm leading-snug text-muted-foreground">{description}</div>
      )}
      {thread && <div className="vyshivka-rule mt-1 h-2 w-16 opacity-70" aria-hidden />}
      {actions && <div className="mt-1.5 flex gap-2">{actions}</div>}
    </div>
  );
}
