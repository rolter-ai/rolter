import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * The wrapper every gated control sits in, which makes a refusal readable
 * without a pointer (#2005).
 *
 * A refused control is a real `disabled` element, and a disabled element takes
 * no focus, so the reason it carried in a `title` reached the mouse and nothing
 * else: a keyboard user tabbed straight past the control, a touch user has no
 * hover. While refused, the wrapper becomes a focusable group named by the
 * control it wraps and described by the reason, which sits beside it as visually
 * hidden text. Tab lands on it and a screen reader says "Add provider, group,
 * Requires the Admin role". While allowed it is `display: contents`, as before,
 * so nothing about the layout or the tab order changes.
 *
 * `title` stays on the control: it is still the hover tooltip for the mouse.
 */
export function RefusalWrap({
  denied,
  reason,
  controlId,
  children,
  ...rest
}: {
  denied: boolean;
  /** the sentence naming the role the control needs */
  reason: string | undefined;
  /** the `id` of the wrapped control, which gives the group its name */
  controlId: string;
  children: React.ReactNode;
} & React.HTMLAttributes<HTMLSpanElement>) {
  const reasonId = React.useId();
  const shown = denied && !!reason;
  return (
    // `display: contents` while allowed so the wrapper is on the event path
    // (a disabled button never dispatches the click itself, see
    // `useRefusedClick`) without being in the layout
    <span
      {...rest}
      className={cn(
        shown
          ? "inline-flex rounded-[6px] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          : "contents",
      )}
      {...(shown
        ? {
            tabIndex: 0,
            role: "group",
            "aria-labelledby": controlId,
            "aria-describedby": reasonId,
          }
        : {})}
    >
      {children}
      {shown ? (
        <span id={reasonId} className="sr-only">
          {reason}
        </span>
      ) : null}
    </span>
  );
}
