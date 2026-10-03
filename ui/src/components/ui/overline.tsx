import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * The small uppercase label above a stat or a field value (#1711).
 *
 * `Health`, `Teams`, `Alerting` and `McpLogs` each wrote the same tracking and
 * size by hand. `as` picks the element, since the label is a `dt` in a
 * description list, an `h3` over a block and a `div` above a figure.
 */
export function Overline({
  as: Tag = "div",
  className,
  ...props
}: React.HTMLAttributes<HTMLElement> & { as?: "div" | "dt" | "h3" | "span" }) {
  return (
    <Tag
      className={cn(
        "mb-0.5 text-[0.6875rem] uppercase tracking-[0.05em] text-[color:var(--text-subtle)]",
        className,
      )}
      {...props}
    />
  );
}
