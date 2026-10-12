import { Check, Minus } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

export type CheckboxProps = Omit<React.InputHTMLAttributes<HTMLInputElement>, "type" | "size"> & {
  /** the mixed state of a parent over partly selected children; `checked` is ignored for the look */
  indeterminate?: boolean;
};

// a real `<input type="checkbox">` drawn by the tokens, so the browser keeps the
// keyboard (Space), the form value and the `label` association. the box is
// neutral when checked (--foreground on --background): a selection is not a
// brand moment, and red on every ticked row breaks the red budget in DESIGN.md
// (#2223). name it with a wrapping <label>, `htmlFor`, or `aria-label`
const Checkbox = React.forwardRef<HTMLInputElement, CheckboxProps>(
  ({ className, indeterminate = false, ...props }, forwardedRef) => {
    const inner = React.useRef<HTMLInputElement | null>(null);
    const setRef = React.useCallback(
      (node: HTMLInputElement | null) => {
        inner.current = node;
        if (typeof forwardedRef === "function") forwardedRef(node);
        else if (forwardedRef) forwardedRef.current = node;
      },
      [forwardedRef],
    );
    // `indeterminate` is a DOM property with no attribute, so it is written
    // from an effect; assistive tech then reads it as aria-checked="mixed"
    React.useEffect(() => {
      if (inner.current) inner.current.indeterminate = indeterminate;
    }, [indeterminate]);
    return (
      <span className={cn("relative inline-flex h-4 w-4 shrink-0", className)}>
        <input
          ref={setRef}
          type="checkbox"
          data-indeterminate={indeterminate || undefined}
          className={cn(
            "peer h-4 w-4 cursor-pointer appearance-none rounded-[4px] border border-[color:var(--zinc-500)] bg-[color:var(--surface-subtle)] transition-colors",
            "checked:border-foreground checked:bg-foreground data-[indeterminate]:border-foreground data-[indeterminate]:bg-foreground",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
            "disabled:cursor-not-allowed disabled:opacity-50",
          )}
          {...props}
        />
        <Check
          aria-hidden
          strokeWidth={3}
          className="pointer-events-none absolute inset-0.5 hidden h-3 w-3 text-background peer-checked:block peer-data-[indeterminate]:hidden"
        />
        <Minus
          aria-hidden
          strokeWidth={3}
          className="pointer-events-none absolute inset-0.5 hidden h-3 w-3 text-background peer-data-[indeterminate]:block"
        />
      </span>
    );
  },
);
Checkbox.displayName = "Checkbox";

export { Checkbox };
