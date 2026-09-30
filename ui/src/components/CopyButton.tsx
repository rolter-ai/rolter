import { Check, Copy, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";

/**
 * Naming the value only helps while the value is a name.
 *
 * A row of eleven "Copy" buttons is unusable without it, which is why the
 * accessible name quotes the address. A code block's value is a whole payload,
 * and reading a thousand characters of JSON as a button's name is worse than
 * not naming it at all — past this length the label stands on its own (#949).
 */
const NAME_IN_LABEL_LIMIT = 80;

export type CopyState = "idle" | "copied" | "failed";

/**
 * Small icon button that copies `value` to the clipboard and briefly shows a
 * checkmark. Used to make `provider-slug/model` addresses one-click copyable.
 *
 * A failed copy shows a red glyph and a tooltip for 1.6 seconds, which is
 * enough for a value that can be copied again. Where the value is shown once
 * (#2327), `persistFailure` keeps the failure until the next press or until
 * `value` changes, and `onStateChange` lets the caller draw its own message
 * beside the value, since an icon button has no room for one.
 */
export function CopyButton({
  value,
  /** overrides the shared `common.copy` label; already-translated when passed */
  label,
  className,
  persistFailure = false,
  onStateChange,
}: {
  value: string;
  label?: string;
  className?: string;
  /** hold a failed copy instead of resetting it on a timer */
  persistFailure?: boolean;
  /**
   * Told every state the button moves to, including a failure repeated on the
   * same value, so a caller can react to each press rather than each change.
   */
  onStateChange?: (state: CopyState) => void;
}) {
  const { t } = useTranslation();
  const [state, setState] = React.useState<CopyState>("idle");
  const copied = state === "copied";
  const copyLabel = label ?? t("common.copy");
  const timer = React.useRef<ReturnType<typeof setTimeout> | null>(null);
  const notify = React.useRef(onStateChange);
  notify.current = onStateChange;

  const move = React.useCallback((next: CopyState) => {
    setState(next);
    notify.current?.(next);
  }, []);

  React.useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  // a held failure belongs to the value it was about; a different value has
  // not failed to copy yet
  React.useEffect(() => {
    if (persistFailure && state === "failed") move("idle");
    // only a new value clears it: the state is what this reads, not a trigger
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);

  const copy = async () => {
    if (timer.current) clearTimeout(timer.current);
    // the next press clears the last one's message before it tries again
    if (persistFailure && state === "failed") move("idle");
    try {
      await navigator.clipboard.writeText(value);
      move("copied");
    } catch {
      // the clipboard api is withheld on an insecure origin (a plain http
      // dashboard on a lan): say so instead of a button that does nothing
      move("failed");
      if (persistFailure) return;
    }
    timer.current = setTimeout(() => move("idle"), 1600);
  };
  const title =
    state === "copied"
      ? t("common.copied")
      : state === "failed"
        ? t("common.copyFailed")
        : copyLabel;

  return (
    <Button
      type="button"
      size="sm"
      variant="ghost"
      className={className}
      onClick={copy}
      aria-label={
        value.length <= NAME_IN_LABEL_LIMIT
          ? t("common.copyValue", { label: copyLabel, value })
          : copyLabel
      }
      title={title}
    >
      {copied ? (
        <Check className="h-3.5 w-3.5" />
      ) : state === "failed" ? (
        <X className="h-3.5 w-3.5 text-[color:var(--status-danger-text)]" />
      ) : (
        <Copy className="h-3.5 w-3.5" />
      )}
      <span aria-live="polite" className="sr-only">
        {state === "idle" ? "" : title}
      </span>
    </Button>
  );
}
