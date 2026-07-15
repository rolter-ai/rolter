import * as React from "react";

import { InfoHint } from "@/components/ui/info-hint";
import { cn } from "@/lib/utils";

// label + control + helper/error wrapper, mirrors the Rolter Design System
// Field. Composes around whatever control is passed as children (Input,
// Select, Textarea, Switch, ...).
export interface FieldProps extends React.HTMLAttributes<HTMLDivElement> {
  label?: string;
  htmlFor?: string;
  error?: string;
  hint?: string;
  // optional explanatory note surfaced via an (i) button beside the label:
  // what the field is and what values to use
  info?: React.ReactNode;
}

export function Field({
  label,
  htmlFor,
  error,
  hint,
  info,
  className,
  children,
  ...props
}: FieldProps) {
  return (
    <div className={cn("space-y-1.5", className)} {...props}>
      {label && (
        <div className="flex items-center gap-1.5">
          <label htmlFor={htmlFor} className="text-sm font-medium leading-none">
            {label}
          </label>
          {info && <InfoHint text={info} label={`About ${label}`} />}
        </div>
      )}
      {children}
      {error ? (
        <p className="text-xs text-destructive">{error}</p>
      ) : hint ? (
        <p className="text-xs text-muted-foreground">{hint}</p>
      ) : null}
    </div>
  );
}
