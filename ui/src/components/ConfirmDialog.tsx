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
import { useFormTelemetry } from "@/lib/ux-react";

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
  /**
   * Stable key for this confirmation in the UX stream (#1730) —
   * `provider-delete`, `session-revoke`. Required for the same reason
   * `EditorSheet` requires one: the dialog backs the destructive action on
   * fifteen screens, and an optional name would leave whichever call site was
   * added last silently uninstrumented. Never the row's own name — that is
   * data, not a key.
   */
  name: string;
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
  /**
   * What the caller must supply before the action can run — a `Field`, a
   * checkbox, a code input (#1078).
   *
   * It sits below the description rather than inside it, because the
   * description is a `<p>` and a control nested in one is invalid markup that
   * screen readers flatten. Most confirmations want nothing here: an action
   * that needs a form is a form, and only an action the *server* gates on a
   * second credential belongs in a confirmation at all.
   */
  children?: React.ReactNode;
  /**
   * Refuse the action until `children` is filled in — a confirmation that
   * carries an input can be incomplete, and spending a round trip to be told
   * so is worse than a button that waits.
   */
  confirmDisabled?: boolean;
}

export function ConfirmDialog({
  name,
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  tone = "danger",
  pending = false,
  error,
  onConfirm,
  children,
  confirmDisabled = false,
}: ConfirmDialogProps) {
  const { t } = useTranslation();

  // UX stream (#805, #1730). A confirmation raised and then dismissed is the
  // record of a delete somebody thought better of, which is exactly the signal
  // a dialog that emitted nothing threw away: `open` going false without a
  // confirm is the abandon, and the dwell time says whether it was a misclick
  // or a decision.
  const ux = useFormTelemetry(name, open);

  // the caller owns the mutation, so the only outcome visible from here is
  // `error` arriving after `pending` — the dialog deliberately stays open on
  // failure, which is what makes that observable at all
  const wasPending = React.useRef(false);
  const failed = error !== undefined && error !== null;
  React.useEffect(() => {
    if (pending) {
      wasPending.current = true;
      return;
    }
    if (!wasPending.current) return;
    wasPending.current = false;
    if (failed) ux.failed();
  }, [pending, failed, ux]);

  const confirm = () => {
    ux.submitted();
    onConfirm();
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {children}
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
          disabled={pending || confirmDisabled}
          onClick={confirm}
        >
          {pending && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {confirmLabel}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
