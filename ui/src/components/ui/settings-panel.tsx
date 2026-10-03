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
 *
 * The title is a real heading (`<h2>` under the screen's `<h1>`, `headingLevel`
 * for a panel nested deeper) so a screen-reader user can move between sections
 * by heading. A switch that governs the panel goes in `action`, at the right of
 * the header, outside the fieldset so a switched-off group can still be switched
 * back on. A switched-off group is a disabled fieldset and nothing else: the
 * controls inside carry the design system's own disabled look, and fading the
 * panel with `opacity` is what DESIGN.md forbids (#1181, #2213).
 */
type HeadingLevel = 2 | 3 | 4;

export function SettingsPanel({
  title,
  description,
  badge,
  action,
  headingLevel = 2,
  dimmed = false,
  className,
  children,
  ...props
}: Omit<React.HTMLAttributes<HTMLElement>, "title"> & {
  title: React.ReactNode;
  /** what this group of settings is for, capped to a readable line length */
  description?: React.ReactNode;
  /** a status chip set beside the title */
  badge?: React.ReactNode;
  /** the control that governs the whole panel, usually a `Switch`, set in the header */
  action?: React.ReactNode;
  /** `h2` by default, under the screen's `h1` */
  headingLevel?: HeadingLevel;
  /**
   * Switch the whole group off.
   *
   * The controls inside carry their own `disabled` too; this is what makes the
   * group read as one unit rather than as several independently dead fields.
   */
  dimmed?: boolean;
}) {
  const Heading = `h${headingLevel}` as const;
  return (
    <section
      className={cn(
        "flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4",
        className,
      )}
      {...props}
    >
      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <Heading className="text-sm font-medium">{title}</Heading>
            {badge}
          </div>
          {description && (
            <p className="mt-1 max-w-[65ch] text-sm text-muted-foreground">{description}</p>
          )}
        </div>
        {action}
      </div>
      {/* a disabled fieldset rather than a dimmed div: every control inside
          already carries `disabled`, and fading a live div drags its labels and
          hints below 4.5:1 while telling assistive tech nothing (#1181).
          `min-w-0` because a fieldset carries a default `min-width: min-content`
          that the div this replaced on Performance did not */}
      {React.Children.toArray(children).length > 0 && (
        <fieldset className="flex min-w-0 flex-wrap gap-4" disabled={dimmed}>
          {children}
        </fieldset>
      )}
    </section>
  );
}
