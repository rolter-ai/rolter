import * as React from "react";

/**
 * What an inline detail drawer owes the keyboard: focus moves into it when it
 * opens (so the next Tab reads its content rather than the row after the one
 * just clicked), Escape closes it, and focus returns to whatever opened it.
 *
 * `subject` names what the drawer shows. A drawer already open on one row that
 * is switched to another stays open, so without it the second row's control
 * was never treated as an opener: focus stayed on that row, and closing sent it
 * back to the first row, which the user had left two steps earlier (#2050).
 * When the subject changes, the control that changed it becomes the opener and
 * focus moves into the panel again.
 *
 * Deliberately not a modal — the table beside it stays usable — so there is no
 * trap and no scrim; that is `useModalA11y`'s job for dialogs and sheets.
 */
export function useDrawerA11y(open: boolean, onClose: () => void, subject?: unknown) {
  const ref = React.useRef<HTMLElement | null>(null);
  const onCloseRef = React.useRef(onClose);
  onCloseRef.current = onClose;
  // held across a subject switch, so the switch hands nothing back on its own:
  // only the drawer closing returns focus, and to the latest opener
  const openerRef = React.useRef<HTMLElement | null>(null);

  React.useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // a modal raised over the drawer owns Escape
      if (document.querySelector('[aria-modal="true"]')) return;
      onCloseRef.current();
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      const opener = openerRef.current;
      openerRef.current = null;
      if (opener && opener.isConnected) opener.focus({ preventScroll: true });
    };
  }, [open]);

  React.useEffect(() => {
    if (!open) return;
    const panel = ref.current;
    if (!panel) return;
    const active = document.activeElement as HTMLElement | null;
    // a control inside the panel that switched the subject (a link to a related
    // row) is not where the user came from, so the earlier opener stands
    if (active && active !== document.body && !panel.contains(active)) {
      openerRef.current = active;
    } else if (!openerRef.current) {
      openerRef.current = active;
    }
    // a macrotask: the drawer may still be laying out on the frame it mounts
    const timer = setTimeout(() => panel.focus({ preventScroll: true }), 0);
    return () => clearTimeout(timer);
  }, [open, subject]);

  return { ref, tabIndex: -1 } as const;
}
