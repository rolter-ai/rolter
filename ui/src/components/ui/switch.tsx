import * as React from "react";

import { cn } from "@/lib/utils";

// the track has no text in it, so nothing gives the button an accessible name
// on its own — and a wrapping <label> does not help either: `label` names form
// controls, never a `role="switch"` button. axe reported this as `button-name`
// on 18 stories (#1181). requiring one of the two aria props in the type is
// what stops the twentieth call site from regressing it
type SwitchLabel =
  | { "aria-label": string; "aria-labelledby"?: never }
  | { "aria-labelledby": string; "aria-label"?: never };

export type SwitchProps = Omit<
  React.ButtonHTMLAttributes<HTMLButtonElement>,
  "onChange" | "aria-label" | "aria-labelledby"
> & {
  checked: boolean;
  onCheckedChange?: (checked: boolean) => void;
} & SwitchLabel;

// minimal switch primitive (no radix dependency); mirrors the Rolter Design
// System Switch — track + thumb, brand-red when on
const Switch = React.forwardRef<HTMLButtonElement, SwitchProps>(
  ({ checked, onCheckedChange, disabled, className, ...props }, ref) => (
    <button
      ref={ref}
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onCheckedChange?.(!checked)}
      className={cn(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-transparent transition-colors",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
        "disabled:cursor-not-allowed disabled:opacity-50",
        checked ? "bg-[color:var(--red-folk)]" : "bg-muted",
        className,
      )}
      {...props}
    >
      <span
        className={cn(
          "inline-block h-4 w-4 rounded-full bg-foreground shadow transition-transform",
          checked ? "translate-x-4" : "translate-x-0.5",
        )}
      />
    </button>
  ),
);
Switch.displayName = "Switch";

export { Switch };
