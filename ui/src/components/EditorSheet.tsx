import * as React from "react";
import { useTranslation } from "react-i18next";

import { Loader2 } from "lucide-react";
import { useDiscardGuard } from "@/components/DiscardGuard";
import { Button } from "@/components/ui/button";
import { Sheet, SheetBody, SheetError, SheetFooter, SheetHeader } from "@/components/ui/sheet";
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
  onOpenChange: (open: boolean) => void;
  title: string;
  subtitle: string;
  /** true when the draft differs from what it was seeded with; gates the
   * discard-changes confirmation on scrim/Escape/Cancel (see DiscardGuard) */
  dirty: boolean;
  errorMessage?: string;
  /** overrides the shared `common.cancel` label; already-translated when passed */
  cancelLabel?: string;
  saveLabel: string;
  canSave: boolean;
  saving: boolean;
  onSave: () => void;
  children: React.ReactNode;
}

export function EditorSheet({
  name,
  open,
  onOpenChange,
  title,
  subtitle,
  dirty,
  errorMessage,
  cancelLabel,
  saveLabel,
  canSave,
  saving,
  onSave,
  children,
}: EditorSheetProps) {
  const { t } = useTranslation();

  const { guard, close, locked, prompt } = useDiscardGuard({ dirty, saving, onOpenChange });

  // UX stream (#805, #1730); the screen key comes from the enclosing
  // UxScreenProvider, so a sheet rendered outside one is silent rather than
  // mislabelled. `open` is what makes abandonment measurable: closed without a
  // submit is an abandon, and the dwell time separates "opened by mistake"
  // from "filled it in and gave up".
  const ux = useFormTelemetry(name, open);

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
    onSave();
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader title={title} subtitle={subtitle} onClose={close} closeDisabled={locked} />
      <SheetBody>{children}</SheetBody>
      <SheetFooter>
        <SheetError message={errorMessage} />
        <div className="flex items-center justify-end gap-2.5 px-[22px] py-3.5">
          <Button variant="ghost" disabled={locked} onClick={close}>
            {cancelLabel ?? t("common.cancel")}
          </Button>
          <Button disabled={!canSave || saving} onClick={save}>
            {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden />}
            {saveLabel}
          </Button>
        </div>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
