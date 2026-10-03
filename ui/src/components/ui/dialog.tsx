import { X } from "lucide-react";
import * as React from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { useModalA11y } from "@/lib/modal-a11y";
import { cn } from "@/lib/utils";

// minimal dependency-free dialog (no radix) — overlay + centered panel.
// Controlled via `open`/`onOpenChange`.
// focus management, the Tab trap and Escape come from useModalA11y; the title
// registers itself through context so the panel is labelled by it
//
// a dialog has to fit a short window too: 1280×720 at 200 % zoom is 640×360,
// and a centered panel taller than that used to lose its title and close
// button above the top edge, with the page's scroll locked (#2003). so the
// overlay is the scroll container and the panel centers with `m-auto`, which
// falls back to the top instead of past it when there is no room. a panel
// that puts its fields in a `DialogBody` goes one step further: it is capped at
// the window, and the body scrolls between a header and a footer that stay put
export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: React.ReactNode;
  /**
   * where focus lands on open — `"panel"` for a confirmation whose first
   * control is the destructive button, `"first"` (default) for a form
   */
  initialFocus?: "first" | "panel";
  /**
   * panel width. `md` (default) is a form or a confirmation; `lg` is a
   * *document* — a code snippet, a config file — where a long line wrapped
   * mid-token costs the reader more than the extra width does (#948)
   */
  size?: "md" | "lg";
}

const SIZES = { md: "max-w-md", lg: "max-w-3xl" } as const;

const LabelContext = React.createContext<{ titleId: string; descriptionId: string } | null>(null);

export function Dialog({ open, onOpenChange, children, initialFocus, size = "md" }: DialogProps) {
  const { t } = useTranslation();
  const panel = React.useRef<HTMLDivElement>(null);
  const titleId = React.useId();
  const descriptionId = React.useId();
  const close = React.useCallback(() => onOpenChange(false), [onOpenChange]);
  const a11y = useModalA11y(panel, { open, onEscape: close, initialFocus });
  const ids = React.useMemo(() => ({ titleId, descriptionId }), [titleId, descriptionId]);

  if (!open) return null;

  // the outer box is the fixed layer useModalA11y keeps live (`layerOf`) and
  // the one that scrolls. the scrim sits inside the scrolled content rather
  // than being fixed itself: a wheel or a drag over a fixed element scrolls the
  // window, not the overlay it belongs to. a press that starts in the panel
  // and ends on the scrim clicks their common parent, which closes nothing.
  // the layer paints above an editor sheet (z-80) and below the toaster (z-90):
  // a confirmation raised from a sheet, the discard prompt included, sat under
  // the sheet's own scrim at z-50 with its action half covered by the panel
  return createPortal(
    <div className="fixed inset-0 z-[85] overflow-y-auto overscroll-contain">
      <div className="relative flex min-h-full p-4">
        <div className="absolute inset-0 bg-black/60" onClick={close} aria-hidden />
        <div
          ref={panel}
          role="dialog"
          aria-modal="true"
          aria-labelledby={titleId}
          aria-describedby={descriptionId}
          className={cn(
            "relative z-10 m-auto w-full rounded-lg border bg-[color:var(--surface-elevated)] p-6 shadow-lg focus-visible:outline-none",
            // only a panel with a body to scroll is capped. one without keeps
            // its natural height and the overlay scrolls the whole of it: a
            // cap with nothing inside to shrink would push the footer out past
            // the panel's own border
            "has-[>[data-slot=dialog-body]]:flex has-[>[data-slot=dialog-body]]:max-h-[calc(100dvh-2rem)] has-[>[data-slot=dialog-body]]:flex-col",
            SIZES[size],
          )}
          {...a11y}
        >
          <LabelContext.Provider value={ids}>
            <button
              type="button"
              onClick={close}
              aria-label={t("common.close")}
              className="absolute right-4 top-4 text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
            >
              <X className="h-4 w-4" />
            </button>
            {children}
          </LabelContext.Provider>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export function DialogHeader({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("mb-4 shrink-0 space-y-1", className)} {...props} />;
}

/**
 * The part of a dialog that scrolls when the window is too short for it.
 *
 * Put a form's fields here, between `DialogHeader` and `DialogFooter`. The
 * panel then caps itself at the window's height and this is the only part
 * that shrinks, so the title, the close button and the primary action stay on
 * screen however short the window gets (#2003). Without one the panel keeps
 * its natural height and the overlay scrolls the whole of it instead.
 */
export function DialogBody({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      data-slot="dialog-body"
      // the gutter on both sides keeps a field's focus ring inside the clip a
      // scroll box imposes, without moving the fields off the header's edge
      className={cn("-mx-1 min-h-0 flex-1 overflow-y-auto px-1", className)}
      {...props}
    />
  );
}

export function DialogTitle({ className, ...props }: React.HTMLAttributes<HTMLHeadingElement>) {
  const ids = React.useContext(LabelContext);
  return (
    <h2
      id={ids?.titleId}
      className={cn("text-lg font-semibold leading-none", className)}
      {...props}
    />
  );
}

export function DialogDescription({
  className,
  ...props
}: React.HTMLAttributes<HTMLParagraphElement>) {
  const ids = React.useContext(LabelContext);
  return (
    <p
      id={ids?.descriptionId}
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  );
}

// wraps rather than running past the panel's edge: a long translated label
// beside Cancel is wider than a 375px phone's panel. the primary action is
// last, so a wrapped row leaves it bottom right, where it was
export function DialogFooter({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("mt-6 flex shrink-0 flex-wrap justify-end gap-2", className)} {...props} />
  );
}
