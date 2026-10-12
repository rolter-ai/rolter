import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export interface SaveBarProps {
  /** how many fields are at fault; above zero the bar says so and Save reads as unavailable */
  invalid: number;
  /** a save is in flight */
  pending?: boolean;
  /** nothing to save yet (`!dirty`); unlike an invalid form, this really disables the button */
  disabled?: boolean;
  /** a quiet line while nothing is at fault, e.g. how many fields were edited */
  status?: React.ReactNode;
  /** pinned to the start of the bar, before the status */
  leading?: React.ReactNode;
  /** secondary buttons beside Save, such as Discard */
  children?: React.ReactNode;
  onSave: () => void;
}

/**
 * The footer of a settings screen (#2734): "N fields need attention" while the
 * form is invalid, and a Save that stays pressable then. A press on an invalid
 * form moves focus to the first field at fault (`useFieldErrors().submit`), so
 * the button is `aria-disabled`, not `disabled`: a disabled button cannot be
 * pressed and could not say why.
 */
export function SaveBar({
  invalid,
  pending = false,
  disabled = false,
  status,
  leading,
  children,
  onSave,
}: SaveBarProps) {
  const { t } = useTranslation();
  const statusId = React.useId();
  const message = invalid > 0 ? t("common.fieldsNeedAttention", { count: invalid }) : status;
  return (
    <div className="sticky bottom-0 flex flex-wrap items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
      {leading}
      {message && (
        <p
          id={statusId}
          role="status"
          className={cn(
            "min-w-0 text-xs",
            !leading && "sm:mr-auto",
            invalid > 0
              ? "text-[color:var(--status-danger-text)]"
              : "text-[color:var(--text-subtle)]",
          )}
        >
          {message}
        </p>
      )}
      {children}
      <Button
        disabled={disabled || pending}
        aria-disabled={invalid > 0 || undefined}
        aria-describedby={message ? statusId : undefined}
        className={invalid > 0 ? "opacity-50" : undefined}
        onClick={onSave}
      >
        {pending ? t("common.saving") : t("common.saveChanges")}
      </Button>
    </div>
  );
}
