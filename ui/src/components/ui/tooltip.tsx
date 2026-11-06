import * as React from "react";
import { createPortal } from "react-dom";

import { AnchoredPanel } from "@/components/ui/anchored-panel";
import { cn } from "@/lib/utils";

// the note behind a short value: a full id behind its prefix, the admins behind
// "Ana +2", the exact time behind "3 min ago", what a badge means (#3655).
//
// a native `title` shows only to a mouse that waits, so a keyboard or a finger
// never reads it. this one opens on hover, on focus and on a tap, stays open
// while the pointer travels onto it (WCAG 1.4.13 hoverable) and closes on
// Escape (dismissible). the note is always in the document, hidden while
// closed, so a screen reader is given it as the trigger's description whether
// it is open or not. closed, it waits in a portal on `body` rather than beside
// the trigger: inside the row it would join the row's text, and a search for
// the row's words would find the note as well
//
// `Tooltip` is the plain case: the visible text becomes a focusable span with a
// dotted underline. a control that is already focusable (a link, a badge inside
// one) takes `useTooltip` instead and spreads `triggerProps` onto itself, so
// there is never an interactive element nested inside another

/** how long the note waits after the pointer leaves, so it can cross the gap */
const LEAVE_DELAY_MS = 120;

export interface TooltipTriggerProps<T extends HTMLElement> {
  ref: (node: T | null) => void;
  "aria-describedby": string;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onFocus: () => void;
  onBlur: () => void;
  onClick: () => void;
}

export function useTooltip<T extends HTMLElement = HTMLElement>(
  content: React.ReactNode,
): { triggerProps: TooltipTriggerProps<T>; note: React.ReactNode; open: boolean } {
  const [open, setOpen] = React.useState(false);
  const [anchor, setAnchor] = React.useState<T | null>(null);
  const id = React.useId();
  const leave = React.useRef<number | undefined>(undefined);
  const show = React.useCallback(() => {
    window.clearTimeout(leave.current);
    setOpen(true);
  }, []);
  const hide = React.useCallback(() => {
    window.clearTimeout(leave.current);
    setOpen(false);
  }, []);
  const hideSoon = React.useCallback(() => {
    window.clearTimeout(leave.current);
    leave.current = window.setTimeout(() => setOpen(false), LEAVE_DELAY_MS);
  }, []);
  React.useEffect(() => () => window.clearTimeout(leave.current), []);
  // the note closes, the Sheet or Dialog around it does not
  React.useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      hide();
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [open, hide]);

  const triggerProps: TooltipTriggerProps<T> = {
    ref: setAnchor,
    "aria-describedby": id,
    onMouseEnter: show,
    onMouseLeave: hideSoon,
    onFocus: show,
    onBlur: hide,
    // a tap focuses the trigger on most phones, but not on every one
    onClick: show,
  };
  const note =
    open && anchor ? (
      <AnchoredPanel
        anchor={anchor}
        side="below"
        align="start"
        role="tooltip"
        id={id}
        onClose={hide}
        onMouseEnter={show}
        onMouseLeave={hideSoon}
        className="max-w-80 whitespace-pre-line break-words rounded-md px-2.5 py-1.5 text-xs font-normal normal-case leading-snug tracking-normal text-foreground shadow-md"
      >
        {content}
      </AnchoredPanel>
    ) : (
      createPortal(
        <span id={id} hidden>
          {content}
        </span>,
        document.body,
      )
    );
  return { triggerProps, note, open };
}

export interface TooltipProps extends Omit<
  React.HTMLAttributes<HTMLSpanElement>,
  "content" | "title"
> {
  /**
   * The whole value, or what the visible text means; lines split on `\n`.
   * Empty or absent, the children render as a plain span with no trigger, so a
   * call site with a note only some of the time needs no branch of its own.
   */
  content: React.ReactNode;
  /** the short value on screen */
  children: React.ReactNode;
}

export function Tooltip({ content, ...props }: TooltipProps) {
  if (content == null || content === "") return <span {...props} />;
  return <TooltipTrigger content={content} {...props} />;
}

function TooltipTrigger({ content, children, className, ...props }: TooltipProps) {
  const { triggerProps, note } = useTooltip<HTMLSpanElement>(content);
  return (
    <>
      <span
        {...props}
        {...triggerProps}
        tabIndex={0}
        className={cn(
          "cursor-help rounded-sm underline decoration-[color:var(--text-subtle)] decoration-dotted underline-offset-2 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          className,
        )}
      >
        {children}
      </span>
      {note}
    </>
  );
}
