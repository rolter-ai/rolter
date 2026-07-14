import * as React from "react";

import { cn } from "@/lib/utils";

// label + control + helper/error wrapper, mirrors the Rolter Design System
// Field. Composes around whatever control is passed as children (Input,
// Select, Textarea, Switch, ...).
export interface FieldProps extends React.HTMLAttributes<HTMLDivElement> {
  label?: string;
  htmlFor?: string;
  error?: string;
  hint?: string;
}

export function Field({
  label,
  htmlFor,
  error,
  hint,
  className,
  children,
  ...props
}: FieldProps) {
  return (
    <div className={cn("space-y-1.5", className)} {...props}>
      {label && (
        <label htmlFor={htmlFor} className="text-sm font-medium leading-none">
          {label}
        </label>
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
