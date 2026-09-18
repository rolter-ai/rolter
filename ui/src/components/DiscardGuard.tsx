import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";

// dirty-editor dismissal, shared by every sheet that owns a draft (#1463).
//
// #1179 moved the destructive actions onto ConfirmDialog but left the discard
// guards on `window.confirm` — unstyled, outside the design system, focus
// handled by the browser rather than by useModalA11y, and in a test runner a
// modal that never resolves.
//
// the browser prompt answered synchronously, which is why `Sheet.onDismiss` is
// a boolean. this hook keeps that contract by *refusing* the dismissal and
// raising the product dialog instead; the sheet is closed from the dialog's
// confirm, one tick later.
export interface DiscardGuardOptions {
  /** the draft differs from what it was seeded with */
  dirty: boolean;
  /** a save is on the wire */
  saving?: boolean;
  onOpenChange: (open: boolean) => void;
}

export interface DiscardGuard {
  /** for `Sheet.onDismiss` — Escape and the scrim */
  guard: () => boolean;
  /** for the header's close button and for Cancel */
  close: () => void;
  /** dismissal is refused outright; disable the controls that offer it */
  locked: boolean;
  /** render among the sheet's children */
  prompt: React.ReactNode;
}

export function useDiscardGuard({
  dirty,
  saving = false,
  onOpenChange,
}: DiscardGuardOptions): DiscardGuard {
  const { t } = useTranslation();
  const [asking, setAsking] = React.useState(false);

  const guard = React.useCallback(() => {
    // the request is already on the wire and nothing here can call it back, so
    // a sheet that vanished now would leave the operator unable to tell whether
    // the mutation landed. dismissal waits for the save to settle instead
    if (saving) return false;
    if (!dirty) return true;
    // idempotent on purpose: Escape twice, or Escape then a scrim click, must
    // raise one prompt rather than stack two
    setAsking(true);
    return false;
  }, [dirty, saving]);

  const close = React.useCallback(() => {
    if (guard()) onOpenChange(false);
  }, [guard, onOpenChange]);

  // the draft belongs to the caller and is re-seeded on the next open, so
  // confirming discards it simply by closing. cancelling touches nothing, and
  // useModalA11y returns focus to the control that raised the prompt
  const discard = React.useCallback(() => {
    setAsking(false);
    onOpenChange(false);
  }, [onOpenChange]);

  // a draft that stopped being dirty under the prompt — a save that landed
  // while it was up — has nothing left to discard
  React.useEffect(() => {
    if (!dirty) setAsking(false);
  }, [dirty]);

  const prompt = (
    <ConfirmDialog
      // one key for every editor that raises it: the question is how often a
      // dirty draft is thrown away, not which sheet it was thrown away from
      name="discard-changes"
      open={asking}
      onOpenChange={setAsking}
      title={t("common.discardChanges")}
      description={t("common.discardBody")}
      confirmLabel={t("common.discardConfirm")}
      onConfirm={discard}
    />
  );

  return { guard, close, locked: saving, prompt };
}
