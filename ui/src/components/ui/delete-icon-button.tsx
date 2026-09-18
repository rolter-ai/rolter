import { Loader2, Trash2 } from "lucide-react";
import * as React from "react";

import { cn } from "@/lib/utils";

/**
 * The trailing delete control on a list row (#1686).
 *
 * `Providers`, `ProviderGroups` and `Models` each carried a byte-identical
 * fourteen-class `<button>` for this, and the copies had already drifted: only
 * `Models` swapped the bin for a spinner while the delete was in flight, so the
 * same action looked dead on two screens and alive on the third. That is the
 * `duplicated-shape` rule's reason for existing, found by the rule itself.
 *
 * It is a plain button rather than a `Button` variant because the row wants an
 * icon square that turns danger-red on hover, not one of the four button sizes.
 * `label` is mandatory: the control has no text, so without it the row reads as
 * an unnamed button to a screen reader.
 */
export interface DeleteIconButtonProps extends Omit<
  React.ButtonHTMLAttributes<HTMLButtonElement>,
  "children" | "type"
> {
  /** the accessible name, which names the row — "Delete provider openai" */
  label: string;
  /**
   * The hover title, when it differs from `label`.
   *
   * A gated row uses this to say *why* the control is dead, which the name
   * cannot carry without changing what the button is called.
   */
  title?: string;
  /** the delete is in flight: the bin becomes a spinner and the button locks */
  pending?: boolean;
}

export function DeleteIconButton({
  label,
  title,
  pending = false,
  disabled,
  className,
  ...props
}: DeleteIconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={title ?? label}
      disabled={disabled || pending}
      className={cn(
        "flex flex-none rounded-[6px] border border-[color:var(--border-subtle)] p-1.5 text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--status-danger)] hover:text-[color:var(--status-danger-text)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50",
        className,
      )}
      {...props}
    >
      {pending ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : (
        <Trash2 className="h-3.5 w-3.5" />
      )}
    </button>
  );
}
