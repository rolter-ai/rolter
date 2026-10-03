import * as React from "react";

import { Button, type ButtonProps } from "@/components/ui/button";
import { RefusalWrap } from "@/components/ui/refusal-wrap";
import { useGate, type Capability } from "@/lib/can";
import { useRefusedClick } from "@/lib/ux-react";
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
 * "disabled" on its own is the same non-answer the 403 was. A disabled button takes
 * no focus, so the reason is also handed to `RefusalWrap`, which makes it
 * reachable by keyboard and screen reader (#2005).
 *
 * `control` names the button in the UX stream when it is refused (#1731) — a
 * stable slug such as `provider-new`, never the label and never anything read
 * off a row (docs/dev-docs/development/ux-telemetry.md). It is required, the
 * way `EditorSheet`'s `name` is, so a new call site cannot forget it (#1750);
 * the `button` fallback stays for anything that reaches it untyped, so that
 * call site still records the capability that refused it and the screen.
 */
export function GatedButton({
  gate,
  control = "button",
  className,
  disabled,
  title,
  style,
  ...props
}: ButtonProps & { gate: Capability; control: string }) {
  const { denied, reason } = useGate(gate);
  const refusal = useRefusedClick(denied, control, gate);
  const generated = React.useId();
  const id = props.id ?? generated;

  return (
    <RefusalWrap denied={denied} reason={reason} controlId={id} {...refusal}>
      <Button
        {...props}
        id={id}
        className={cn(denied && "cursor-not-allowed", className)}
        // the button variants set `disabled:pointer-events-none`, which also
        // suppresses the native tooltip — so the one explanation the control has
        // would never be readable. an inline style is the only thing that
        // reliably outranks the variant; `disabled` still swallows the click
        style={denied ? { ...style, pointerEvents: "auto" } : style}
        disabled={disabled || denied}
        title={denied ? reason : title}
      />
    </RefusalWrap>
  );
}
