import * as React from "react";
import { useTranslation } from "react-i18next";

import { Loader2 } from "lucide-react";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { Button } from "@/components/ui/button";
import { SubmitForm, SubmitFormStatus } from "@/components/ui/submit-form";
import {
  Sheet,
  SheetActions,
  SheetBody,
  SheetError,
  SheetFooter,
  SheetHeader,
} from "@/components/ui/sheet";
import { useFormTelemetry } from "@/lib/ux-react";

// shared shell for a create/edit form (#584): every editor sheet in the
// dashboard (ModelSheet, ProviderSheet, ProviderGroupSheet) hand-assembles the
// same header/body/footer/dirty-guard structure. This factors that shell out
// so pages migrating off the center-popup Dialog editor don't re-derive it —
// callers still own their own draft state and save mutation, this only owns
// the chrome around it.
export interface EditorSheetProps {
  /**
   * Stable key for this form in the UX stream (#1730) — `virtual-key-create`,
   * `alert-channel`. Required rather than optional on purpose: the shell is
   * rendered from thirteen screens, and an optional name would mean the
   * instrumentation is only as complete as the last person who remembered it.
   * Never derived from what was typed; `sanitizeKey` drops anything that is
   * not a plausible key, so a title or an error message here costs the event.
   */
  name: string;
  open: boolean;
  /**
   * Hide the sheet without ending the form: a confirmation opened over it keeps
   * the drafts and the telemetry's open state, so the sheet going out of sight
   * is not an abandon (#3043). `open` stays true throughout.
   */
  suspended?: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  subtitle: string;
  /** true when the draft differs from what it was seeded with; gates the
   * discard-changes confirmation on scrim/Escape/Cancel (see DiscardGuard) */
  dirty: boolean;
  errorMessage?: string;
  /** the translated next step, between `errorMessage` and `errorDetail` */
  errorHint?: string;
  /**
   * What the control plane said, shown under `errorMessage` as its detail.
   * The lead is the caller's translated line; this stays as the server wrote it.
   */
  errorDetail?: string;
  /** overrides the shared `common.cancel` label; already-translated when passed */
  cancelLabel?: string;
  saveLabel: string;
  /**
   * false while the form cannot be saved. The button stays pressable
   * (`aria-disabled`): a press, or Enter in a field, then asks for the reasons
   * instead of doing nothing (#1205)
   */
  canSave: boolean;
  /**
   * what to say after a refused submit when no field is at fault, so the
   * status line has something to name (a pick that is not an input)
   */
  blockedMessage?: string;
  saving: boolean;
  /** the form's submit: runs on the Save button and on Enter in a field, once nothing is at fault */
  onSubmit: () => void;
  children: React.ReactNode;
}

export function EditorSheet({
  name,
  open,
  suspended = false,
  onOpenChange,
  title,
  subtitle,
  dirty,
  errorMessage,
  errorHint,
  errorDetail,
  cancelLabel,
  saveLabel,
  canSave,
  blockedMessage,
  saving,
  onSubmit,
  children,
}: EditorSheetProps) {
  const { t } = useTranslation();

  const { guard, close, locked, prompt } = useDiscardGuard({ dirty, saving, onOpenChange });

  // UX stream (#805, #1730); the screen key comes from the enclosing
  // UxScreenProvider, so a sheet rendered outside one is silent rather than
  // mislabelled. `open` is what makes abandonment measurable: closed without a
  // submit is an abandon, and `dirty` is what says which kind — a sheet closed
  // untouched is a misclick, one closed with a draft in it is somebody who
  // filled it in and gave up (#1731)
  const ux = useFormTelemetry(name, open, { dirty });

  // the outcome of a save the caller owns, read off the only two props that
  // report it. `saving` falling back to false is the round trip settling;
  // `errorMessage` set in that same commit is how the caller says it failed
  const wasSaving = React.useRef(false);
  React.useEffect(() => {
    if (saving) {
      wasSaving.current = true;
      return;
    }
    if (!wasSaving.current) return;
    wasSaving.current = false;
    if (errorMessage) ux.failed();
    else ux.saved();
  }, [saving, errorMessage, ux]);

  const save = () => {
    ux.submitted();
    onSubmit();
  };

  return (
    <Sheet open={open && !suspended} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
      <SubmitForm onSubmit={save} canSubmit={canSave} busy={saving}>
        <SheetBody>{children}</SheetBody>
        <SheetFooter>
          <SubmitFormStatus fallback={blockedMessage} />
          <SheetError message={errorMessage} hint={errorHint} detail={errorDetail} />
          <SheetActions>
            <Button variant="ghost" disabled={locked} onClick={close}>
              {cancelLabel ?? t("common.cancel")}
            </Button>
            <Button
              type="submit"
              disabled={saving}
              aria-disabled={!canSave || undefined}
              className="aria-disabled:opacity-50"
            >
              {saving && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" aria-hidden />}
              {saveLabel}
            </Button>
          </SheetActions>
        </SheetFooter>
      </SubmitForm>
      {prompt}
    </Sheet>
  );
}
