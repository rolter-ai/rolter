import { useTranslation } from "react-i18next";

import { Dialog, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { KbdChord } from "@/components/ui/kbd";
import { SHORTCUTS } from "@/lib/shortcuts";

// `?` — what can I press here (#1676).
//
// The list is `SHORTCUTS` mapped, not prose: the shell dispatches off the same
// table, so a shortcut cannot work and be missing from this sheet. The only
// thing written by hand per row is its name, and that is a catalog key derived
// from the id, which `shortcuts.test.ts` checks against every locale.
//
// No loading, empty or error state: there is no request behind it. The table is
// a module constant, it is never empty, and a build where it were would fail
// `shortcuts.test.ts` before it reached a reader.

export interface ShortcutHelpProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** pin the platform glyph; stories set it so `⌘` does not follow the runner */
  apple?: boolean;
}

export function ShortcutHelp({ open, onOpenChange, apple }: ShortcutHelpProps) {
  const { t } = useTranslation();

  return (
    <Dialog open={open} onOpenChange={onOpenChange} initialFocus="panel">
      <DialogHeader>
        <DialogTitle>{t("shell.shortcuts.title")}</DialogTitle>
        <DialogDescription>{t("shell.shortcuts.description")}</DialogDescription>
      </DialogHeader>
      {/* a description list, not a table: two cells per row where one names
          the other is exactly what `dl` is, and it needs no header row */}
      <dl className="divide-y divide-[color:var(--border-subtle)]">
        {SHORTCUTS.map((shortcut) => (
          <div key={shortcut.id} className="flex items-center justify-between gap-4 py-2.5">
            <dt className="min-w-0 text-sm text-foreground">
              {t(`shell.shortcuts.items.${shortcut.id}`)}
            </dt>
            <dd className="flex-none">
              <KbdChord chord={shortcut.chord} apple={apple} />
            </dd>
          </div>
        ))}
      </dl>
    </Dialog>
  );
}
