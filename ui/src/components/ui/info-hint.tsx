import { Info } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

// small info affordance: an (i) button that reveals a short note on hover,
// focus, or click. hand-rolled (no radix) to stay dependency- and CDN-free.
// accessible: focusable button, aria-describedby wiring, dismissable on blur.
export interface InfoHintProps {
  // the explanatory note; keep it short — what the field is and what values fit
  text: React.ReactNode;
  // accessible label for the trigger; defaults to a generic phrasing
  label?: string;
  className?: string;
}

export function InfoHint({ text, label, className }: InfoHintProps) {
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(false);
  const id = React.useId();
  // WCAG 1.4.13 dismissible: Escape closes the note wherever focus is
  React.useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // the note closes, the Sheet or Dialog around it does not
      event.stopPropagation();
      setOpen(false);
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [open]);
  return (
    // hover lives on the wrapper so the pointer can travel onto the note
    // (WCAG 1.4.13 hoverable) without it closing
    <span
      className="relative inline-flex"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button
        type="button"
        aria-label={label ?? t("common.moreInfo")}
        aria-expanded={open}
        aria-describedby={open ? id : undefined}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "inline-flex items-center justify-center text-muted-foreground transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:outline-none",
          className,
        )}
      >
        <Info className="h-3.5 w-3.5" />
      </button>
      {open && (
        <span
          role="tooltip"
          id={id}
          className="absolute left-1/2 top-full z-50 mt-1.5 w-56 -translate-x-1/2 rounded-md border border-border bg-[color:var(--surface-elevated)] px-2.5 py-1.5 text-xs font-normal leading-snug text-foreground shadow-md"
        >
          {/* bridges the gap so the pointer can reach the note */}
          <span aria-hidden className="absolute inset-x-0 -top-1.5 h-1.5" />
          {text}
        </span>
      )}
    </span>
  );
}
