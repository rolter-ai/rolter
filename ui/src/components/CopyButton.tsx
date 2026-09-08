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

/**
 * Small icon button that copies `value` to the clipboard and briefly shows a
 * checkmark. Used to make `provider-slug/model` addresses one-click copyable.
 */
export function CopyButton({
  value,
  /** overrides the shared `common.copy` label; already-translated when passed */
  label,
  className,
}: {
  value: string;
  label?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const [state, setState] = React.useState<"idle" | "copied" | "failed">("idle");
  const copied = state === "copied";
  const copyLabel = label ?? t("common.copy");
  const timer = React.useRef<ReturnType<typeof setTimeout> | null>(null);

  React.useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const copy = async () => {
    if (timer.current) clearTimeout(timer.current);
    try {
      await navigator.clipboard.writeText(value);
      setState("copied");
    } catch {
      // the clipboard api is withheld on an insecure origin (a plain http
      // dashboard on a lan): say so instead of a button that does nothing
      setState("failed");
    }
    timer.current = setTimeout(() => setState("idle"), 1600);
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
