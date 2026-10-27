import { Info } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { AnchoredPanel } from "@/components/ui/anchored-panel";
import { cn } from "@/lib/utils";

// small info affordance: an (i) button that reveals a short note on hover,
// focus, or click. no radix, so it stays dependency- and CDN-free.
// accessible: focusable button, aria-describedby wiring, dismissable on blur.
//
// the note is an `AnchoredPanel` (#487): fixed and placed from the button's
// rectangle, so the scroll area of a dialog or sheet cannot clip it, and it
// slides in or turns over to stay on screen. it used to be `absolute` and
// centred on the button, which cut it off at the edge of a dialog for a field
// whose label is short or that sits in the last column
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
  const [trigger, setTrigger] = React.useState<HTMLButtonElement | null>(null);
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
    // (WCAG 1.4.13 hoverable) without it closing. the note is a DOM child of
    // the wrapper even though it is drawn fixed, so moving onto it is not
    // leaving
    <span
      className="relative inline-flex"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button
        ref={setTrigger}
        type="button"
        aria-label={label ?? t("common.moreInfo")}
        aria-expanded={open}
        aria-describedby={open ? id : undefined}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          "-m-[5px] inline-flex items-center justify-center p-[5px] text-muted-foreground transition-colors hover:text-foreground focus-visible:text-foreground focus-visible:outline-none",
          className,
        )}
      >
        <Info className="h-3.5 w-3.5" />
      </button>
      {open && trigger && (
        <AnchoredPanel
          anchor={trigger}
          side="below"
          align="center"
          role="tooltip"
          id={id}
          onClose={() => setOpen(false)}
          className="w-56 rounded-md px-2.5 py-1.5 text-xs font-normal normal-case leading-snug tracking-normal text-foreground shadow-md"
        >
          {/* bridge the gap on whichever side the button is, so the pointer can
              reach the note when it has turned over */}
          <span aria-hidden className="absolute inset-x-0 -top-1.5 h-1.5" />
          <span aria-hidden className="absolute inset-x-0 -bottom-1.5 h-1.5" />
          {text}
        </AnchoredPanel>
      )}
    </span>
  );
}
