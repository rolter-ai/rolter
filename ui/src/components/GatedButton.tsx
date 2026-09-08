import { Button, type ButtonProps } from "@/components/ui/button";
import { useGate, type Capability } from "@/lib/can";
import { cn } from "@/lib/utils";

/**
 * A `Button` that disables itself when the caller may not do the thing (#1183).
 *
 * `gate` is the `resource:action` pair from the control plane's capability
 * table (crates/rolter-control/src/rbac_matrix.rs) — the same string the
 * effective-permissions endpoint answers with, so there is no second
 * vocabulary to keep in step.
 *
 * It is a real `disabled`, not an `aria-disabled`: a control that still takes
 * the click and then explains the 403 has already wasted the operator's
 * attention, and a screen reader that is told "button" without "disabled"
 * learns nothing. The `title` says which role the action takes, because
 * "disabled" on its own is the same non-answer the 403 was.
 */
export function GatedButton({
  gate,
  className,
  disabled,
  title,
  style,
  ...props
}: ButtonProps & { gate: Capability }) {
  const { denied, reason } = useGate(gate);

  return (
    <Button
      {...props}
      className={cn(denied && "cursor-not-allowed", className)}
      // the button variants set `disabled:pointer-events-none`, which also
      // suppresses the native tooltip — so the one explanation the control has
      // would never be readable. an inline style is the only thing that
      // reliably outranks the variant; `disabled` still swallows the click
      style={denied ? { ...style, pointerEvents: "auto" } : style}
      disabled={disabled || denied}
      title={denied ? reason : title}
    />
  );
}
