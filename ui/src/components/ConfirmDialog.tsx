import { Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

// one confirmation for every destructive action (#1179).
//
// before this, nine controls deleted on a single click while eight others
// confirmed through a hand-rolled `Dialog`, and four more fell back to
// `window.confirm` — unstyled, untranslatable, and in a test runner a modal that
// never resolves. The pattern was inconsistent exactly where being wrong is
// unrecoverable.
//
// The dialog deliberately does **not** close itself on confirm. The caller
// closes it from the mutation's `onSuccess`, so a request that fails leaves the
// dialog open with `error` rendered beside the button that caused it. Closing on
// click would drop the only place the failure could be reported.
export interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** already translated, and names the thing: "Delete channel ops-slack?" */
  title: string;
  /** one sentence on the consequence; a node so a name can be set in mono */
  description: React.ReactNode;
  /** already translated verb, e.g. t("common.delete") */
  confirmLabel: string;
  /** `danger` paints the confirm button destructive, `default` leaves it primary */
  tone?: "danger" | "default";
  pending?: boolean;
  /** the mutation's thrown value, rendered verbatim when the confirm failed */
  error?: unknown;
  onConfirm: () => void;
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  tone = "danger",
  pending = false,
  error,
  onConfirm,
}: ConfirmDialogProps) {
  const { t } = useTranslation();

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {/* the control plane's own message, never a gloss on it — see
          docs/dev-docs/development/error-states.md */}
      {error !== undefined && error !== null && (
        <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
          {error instanceof Error ? error.message : String(error)}
        </p>
      )}
      <DialogFooter>
        {/* cancel is disabled mid-flight too: the request is already on the
            wire, so a button that looks like it calls it back would lie */}
        <Button variant="outline" disabled={pending} onClick={() => onOpenChange(false)}>
          {t("common.cancel")}
        </Button>
        <Button
          variant={tone === "danger" ? "destructive" : "default"}
          disabled={pending}
          onClick={onConfirm}
        >
          {pending && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {confirmLabel}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
