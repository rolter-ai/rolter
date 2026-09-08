import * as React from "react";
import { useTranslation } from "react-i18next";

import { Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";

// shared shell for a create/edit form (#584): every editor sheet in the
// dashboard (ModelSheet, ProviderSheet, ProviderGroupSheet) hand-assembles the
// same header/body/footer/dirty-guard structure. This factors that shell out
// so pages migrating off the center-popup Dialog editor don't re-derive it —
// callers still own their own draft state and save mutation, this only owns
// the chrome around it.
export interface EditorSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  subtitle: string;
  /** true when the draft differs from what it was seeded with; gates the
   * discard-changes confirmation on scrim/Escape/Cancel */
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

  const guard = React.useCallback(() => {
    if (!dirty) return true;
    return window.confirm(t("common.discardChanges"));
  }, [dirty, t]);

  const close = React.useCallback(() => {
    if (guard()) onOpenChange(false);
  }, [guard, onOpenChange]);

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader title={title} subtitle={subtitle} onClose={close} />
      <SheetBody>{children}</SheetBody>
      <SheetFooter>
        {errorMessage && (
          <p className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">{errorMessage}</p>
        )}
        <div className="flex items-center justify-end gap-2.5 px-[22px] py-3.5">
          <Button variant="ghost" onClick={close}>
            {cancelLabel ?? t("common.cancel")}
          </Button>
          <Button disabled={!canSave || saving} onClick={onSave}>
            {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden />}
            {saveLabel}
          </Button>
        </div>
      </SheetFooter>
    </Sheet>
  );
}
