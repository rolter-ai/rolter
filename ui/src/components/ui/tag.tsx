import { X } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

// mono-font removable tag, mirrors the Rolter Design System Tag
export interface TagProps extends React.HTMLAttributes<HTMLSpanElement> {
  onRemove?: () => void;
}

export function Tag({ onRemove, className, children, ...props }: TagProps) {
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
          aria-label="Remove"
          className="-mr-0.5 inline-flex h-3.5 w-3.5 items-center justify-center rounded text-muted-foreground/70 hover:bg-accent hover:text-foreground"
        >
          <X className="h-2.5 w-2.5" />
        </button>
      )}
    </span>
  );
}
