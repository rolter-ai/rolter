import { X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

// mono-font removable tag, mirrors the Rolter Design System Tag
export interface TagProps extends React.HTMLAttributes<HTMLSpanElement> {
  onRemove?: () => void;
  /**
   * Accessible name of the remove button, already translated.
   *
   * A screen reader hearing "Remove" on every chip in a list learns nothing
   * about which one it is on, so a caller that renders more than one names the
   * thing being removed.
   */
  removeLabel?: string;
}

export function Tag({ onRemove, removeLabel, className, children, ...props }: TagProps) {
  const { t } = useTranslation();
  return (
    <span
      className={cn(
        "inline-flex items-center gap-2 h-[22px] px-2 rounded border border-border bg-muted font-mono text-[0.625rem] text-muted-foreground whitespace-nowrap",
        className,
      )}
      {...props}
    >
      {children}
      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          aria-label={removeLabel ?? t("common.remove")}
          className="-mr-0.5 inline-flex h-3.5 w-3.5 items-center justify-center rounded text-muted-foreground/70 hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <X className="h-2.5 w-2.5" />
        </button>
      )}
    </span>
  );
}
