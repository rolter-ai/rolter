import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * A titled group of settings controls (#1682).
 *
 * The deployment-settings screens are built from these: a bordered section, a
 * title, one line saying what the group is for, and a row of controls that can
 * be switched off as a block. `ModelSettings` and `Performance` each grew their
 * own copy of it under the name `Card`, which collided with `ui/card.tsx`'s
 * unrelated `Card` — a bare bordered div with separate `CardHeader` /
 * `CardTitle` / `CardDescription` / `CardContent` parts. Two components, one
 * name, and a reader had to work out which was which every time.
 *
 * This is the settings-panel half, named for what it is. `Card` stays what it
 * was.
 */
export function SettingsPanel({
  title,
  description,
  dimmed = false,
  className,
  children,
  ...props
}: Omit<React.HTMLAttributes<HTMLElement>, "title"> & {
  title: React.ReactNode;
  /** one line on what this group of settings is for */
  description?: React.ReactNode;
  /**
   * Switch the whole group off.
   *
   * The controls inside carry their own `disabled` too; this is what makes the
   * group read as one unit rather than as several independently dead fields.
   */
  dimmed?: boolean;
}) {
  return (
    <section
      className={cn(
        "flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4",
        className,
      )}
      {...props}
    >
      <div>
        <span className="text-sm font-medium">{title}</span>
        {description && <p className="mt-1 text-sm text-muted-foreground">{description}</p>}
      </div>
      {/* a disabled fieldset rather than a dimmed div: every control inside
          already carries `disabled`, and fading a live div drags its labels and
          hints below 4.5:1 while telling assistive tech nothing (#1181).
          `min-w-0` because a fieldset carries a default `min-width: min-content`
          that the div this replaced on Performance did not */}
      <fieldset
        className="flex min-w-0 flex-wrap gap-4"
        disabled={dimmed}
        style={{ opacity: dimmed ? 0.55 : 1 }}
      >
        {children}
      </fieldset>
    </section>
  );
}
