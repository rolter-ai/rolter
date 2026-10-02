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
import { cn } from "@/lib/utils";
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
  /**
   * The request the confirm started is on the wire.
   *
   * Passing it at all is also what says the confirm *runs* a request, and so
   * has a landing worth reporting as `save_confirmed` once the caller closes
   * the dialog. Leave it unset only for a confirmation that finishes the moment
   * it is pressed — the discard prompt closes a sheet and is done.
   */
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
  pending,
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

  const hasChildren = React.Children.toArray(children).length > 0;
  const busy = pending ?? false;
  const runsRequest = pending !== undefined;

  // the caller owns the mutation, so its outcome is read off the props that
  // report it. the press arms the read rather than a `pending` edge (#1761): a
  // request that settles in the tick it started hands react-query's pending
  // and error to one notify batch, so `pending={true}` is never committed and
  // an edge-triggered read saw no failure at all. the read is keyed on the
  // error's identity rather than on whether there is one, so a retry refused
  // the same way is a new error and is reported too. `pressed` holds the error
  // standing at the press — a retry's, with the last refusal still on screen —
  // since only a different one can be this press's answer
  const pressed = React.useRef<{ error: unknown } | null>(null);
  const wasOpen = React.useRef(open);
  React.useEffect(() => {
    // a fresh opening owes nothing to a request left behind by the last one
    if (open && !wasOpen.current) pressed.current = null;
    wasOpen.current = open;
    const press = pressed.current;
    if (!press || busy) return;
    const failed = error !== undefined && error !== null;
    if (failed && error !== press.error) {
      pressed.current = null;
      ux.failed();
    } else if (!open) {
      // the caller closes the dialog from `onSuccess`, so closed with the
      // request settled and nothing refused is how a landed confirm looks from
      // here. the hand-rolled dialogs this replaced reported it as
      // `save_confirmed`, and the save-latency query reads no other row
      pressed.current = null;
      if (runsRequest && !failed) ux.saved();
    }
  }, [open, busy, error, runsRequest, ux]);

  const confirm = () => {
    pressed.current = { error };
    ux.submitted();
    onConfirm();
  };

  // Escape, the scrim and the close button still close the dialog mid-flight.
  // neither the fetch nor the control plane times a request out, so a delete
  // stuck behind a row lock would otherwise hold the operator in a full-page
  // modal until a reload. the dismissal disarms the read above first: the
  // caller resets the mutation on close, and the read would take that silence
  // for a landing. the press stays on record and nothing after it does
  const dismiss = (next: boolean) => {
    if (!next && busy) pressed.current = null;
    onOpenChange(next);
  };

  return (
    <Dialog open={open} onOpenChange={dismiss}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {children}
      {/* the control plane's own message, never a gloss on it — see
          docs/dev-docs/development/error-states.md */}
      {error !== undefined && error !== null && (
        <p
          role="alert"
          className={cn(
            "text-xs text-[color:var(--status-danger-text)]",
            // the gap the children keep between themselves (gap-2.5), so the
            // message is not flush against the last of them (#2359)
            hasChildren && "mt-2.5",
          )}
        >
          {error instanceof Error ? error.message : String(error)}
        </p>
      )}
      {/* ui-primitives-allow: this is the one confirmation every other screen is sent to */}
      <DialogFooter>
        {/* cancel is disabled mid-flight too: the request is already on the
            wire, so a button that looks like it calls it back would lie */}
        <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
          {t("common.cancel")}
        </Button>
        <Button
          variant={tone === "danger" ? "destructive" : "default"}
          disabled={busy || confirmDisabled}
          onClick={confirm}
        >
          {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {confirmLabel}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
