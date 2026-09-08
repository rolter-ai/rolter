import { X } from "lucide-react";
import * as React from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { useModalA11y } from "@/lib/modal-a11y";
import { cn } from "@/lib/utils";

// minimal dependency-free dialog (no radix) — overlay + centered panel,
// mirrors the Rolter Design System Dialog. Controlled via `open`/`onOpenChange`.
// focus management, the Tab trap and Escape come from useModalA11y; the title
// registers itself through context so the panel is labelled by it
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

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      <div className="fixed inset-0 bg-black/60" onClick={close} aria-hidden />
      <div
        ref={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        className={cn(
          "relative z-10 w-full rounded-lg border bg-[color:var(--surface-elevated)] p-6 shadow-lg focus-visible:outline-none",
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
    </div>,
    document.body,
  );
}

export function DialogHeader({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn("mb-4 space-y-1", className)} {...props} />;
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

export function DialogFooter({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("mt-6 flex justify-end gap-2", className)} {...props} />
  );
}
