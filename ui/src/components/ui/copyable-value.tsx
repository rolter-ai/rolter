import { AlertTriangle } from "lucide-react";
import * as React from "react";

import { CopyButton, type CopyState } from "@/components/CopyButton";
import { FieldLabel } from "@/components/ui/field-label";
import { cn } from "@/lib/utils";

// the one place a value that can be copied again is laid out (#2418, #2366).
//
// a SCIM base URL, a redirect URI, the collector-config address and a request id
// were each a mono value beside a `CopyButton`, hand-built four times with four
// borders, fills and text sizes. a value shown once is `SecretValue`
// (`secret-reveal.tsx`), which composes `CopyableText` for the same box and adds
// the failed-copy message that stays and the close guard a repeatable value does
// not need.
//
// two parts:
//
// - `CopyableText`: the value and its copy button, boxed or inline, no label
// - `CopyableValue`: a labelled group around the box, with a hint, a note, and
//   what stands in for the value while there is none

// mono and wrapping rather than truncated: an address or an id is checked by its
// end. `select-all` makes one click take the whole value, which is how it gets
// copied on a plain-http dashboard where the clipboard api is withheld
const VALUE = "min-w-0 select-all break-all font-mono text-foreground";

// the bordered box. its height is the copy button's plus the padding, so a
// placeholder or a skeleton of `COPYABLE_BOX_HEIGHT` holds the same space
const BOX =
  "flex min-w-0 items-start justify-between gap-2 rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] py-1.5 pl-3 pr-1.5";

/** The boxed value's rendered height in px, for the skeleton that stands in for it. */
export const COPYABLE_BOX_HEIGHT = 46;

/**
 * A value, selectable in one click, beside the button that copies it.
 *
 * `boxed` is the bordered box a value sits in on its own. `inline` is the bare
 * value and a small button, for a value that is already one cell of a
 * description list, where a box per row would be a grid of boxes; it takes the
 * text size of the row around it.
 */
export function CopyableText({
  value,
  copyLabel,
  variant = "boxed",
  testId,
  className,
  ref,
  persistFailure,
  onCopyStateChange,
}: {
  value: string;
  /** names the copy button; already translated */
  copyLabel: string;
  variant?: "boxed" | "inline";
  /** on the value itself, so a story reads the value and not the button's name */
  testId?: string;
  className?: string;
  /** the value's element, for a caller that selects it itself */
  ref?: React.Ref<HTMLElement>;
  /** passed to `CopyButton`: hold a failed copy instead of resetting it */
  persistFailure?: boolean;
  /** passed to `CopyButton`: told every state the copy moves to */
  onCopyStateChange?: (state: CopyState) => void;
}) {
  if (variant === "inline") {
    return (
      <span className={cn("flex min-w-0 items-start gap-0.5", className)}>
        <code ref={ref} data-testid={testId} className={VALUE}>
          {value}
        </code>
        {/* lifted by the difference between the 24px button and a 16px line,
            so its icon sits on the value's first line */}
        <CopyButton
          value={value}
          label={copyLabel}
          persistFailure={persistFailure}
          onStateChange={onCopyStateChange}
          className="-mt-1 h-6 flex-none px-1"
        />
      </span>
    );
  }
  return (
    <div className={cn(BOX, className)}>
      {/* padded to the button's height so a one-line value sits level with
          its icon, and a wrapped one keeps the button on its first line */}
      <code ref={ref} data-testid={testId} className={cn(VALUE, "py-1 text-sm")}>
        {value}
      </code>
      <CopyButton
        value={value}
        label={copyLabel}
        persistFailure={persistFailure}
        onStateChange={onCopyStateChange}
      />
    </div>
  );
}

/**
 * A labelled value to copy out of the dashboard and into something else: an
 * identity provider's connector, a collector's tooling, a terminal.
 *
 * The group is named by `label` and described by `hint`. `value` is null while
 * there is nothing to copy; `empty` then says why inside the box, at the box's
 * own height, so the row does not jump when the value arrives. `status`
 * replaces the box outright, for a value that is still being read (a skeleton)
 * or could not be (a `LoadError`), so nothing that might be wrong is offered.
 * `note` is a caution under the hint: the value is only a default, or only part
 * of what is needed.
 */
export function CopyableValue({
  label,
  value,
  copyLabel,
  hint,
  note,
  empty,
  status,
  besideFields = false,
  testId,
  className,
}: {
  label: string;
  value: string | null;
  /** names the copy button; already translated */
  copyLabel: string;
  hint?: React.ReactNode;
  /** a caution about the value, shown as a note under the hint */
  note?: React.ReactNode;
  /** why there is no value yet, shown in the box while `value` is null */
  empty?: React.ReactNode;
  /** stands in for the box: a skeleton while the value loads, an error when it failed */
  status?: React.ReactNode;
  /** the row sits among `Field` rows, so its label takes their size */
  besideFields?: boolean;
  testId?: string;
  className?: string;
}) {
  const labelId = React.useId();
  const hintId = React.useId();
  return (
    <div
      role="group"
      aria-labelledby={labelId}
      aria-describedby={hint ? hintId : undefined}
      className={cn("flex min-w-0 flex-col gap-1.5", className)}
    >
      {besideFields ? (
        <p id={labelId} className="text-sm font-medium leading-none">
          {label}
        </p>
      ) : (
        <FieldLabel id={labelId} label={label} />
      )}
      {status ??
        (value ? (
          <CopyableText value={value} copyLabel={copyLabel} testId={testId} />
        ) : (
          <div className={BOX}>
            <span className="py-1.5 text-sm text-muted-foreground">{empty}</span>
          </div>
        ))}
      {hint && (
        <p id={hintId} className="text-xs text-muted-foreground">
          {hint}
        </p>
      )}
      {note && (
        <p
          role="note"
          className="flex items-start gap-1.5 text-xs text-[color:var(--status-warning-text)]"
        >
          <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
          <span>{note}</span>
        </p>
      )}
    </div>
  );
}
