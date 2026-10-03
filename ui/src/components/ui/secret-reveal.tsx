import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { CopyableText } from "@/components/ui/copyable-value";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { FieldLabel } from "@/components/ui/field-label";
import { cn } from "@/lib/utils";

// the one place a value that is shown once is revealed (#2217).
//
// a minted key, a SCIM bearer token and an invitation link are all stored only
// as a digest or not at all: the dialog they appear in is the last time the
// value is on screen. three screens each laid that out by hand, and each one
// copied silently into a clipboard a plain-http dashboard never has, closed on
// Escape with the value uncopied, and ended at "Done" with no next step.
//
// three parts, so a reveal that lives inside a sheet can take the body and the
// guard without the dialog shell:
//
// - `SecretValue`: the value, its copy button, and what a failed copy says
// - `useSecretCloseGuard`: the question asked before an uncopied value goes
// - `SecretRevealDialog`: a dialog made of the two, with a slot for the next step

/**
 * The value, selectable in one gesture, beside a button that copies it: the
 * box is `CopyableText`'s, which a value that can be copied again shares.
 *
 * A failed copy stays on screen as a line under the value that says the copy
 * did not work and offers to select it (#2327); a tooltip and an icon that
 * resets after a moment are easy to miss where losing the value costs
 * something. The value is also selected as soon as the copy fails, so copying
 * it by hand is one keystroke. A value copied that way counts as copied, which
 * is why closing afterwards does not ask.
 */
export function SecretValue({
  value,
  label,
  copyLabel,
  onCopied,
  testId,
  className,
}: {
  value: string;
  /** a visible name, for a reveal whose surrounding copy does not name the value */
  label?: string;
  /** names the copy button; already translated */
  copyLabel: string;
  /** the value reached the clipboard, by the button or by hand */
  onCopied: () => void;
  testId?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const labelId = React.useId();
  const code = React.useRef<HTMLElement>(null);
  const [outcome, setOutcome] = React.useState<"failed" | "by-hand" | null>(null);
  const copied = React.useRef(onCopied);
  copied.current = onCopied;

  const selectValue = React.useCallback(() => {
    const node = code.current;
    const selection = window.getSelection();
    if (!node || !selection) return;
    const range = document.createRange();
    range.selectNodeContents(node);
    selection.removeAllRanges();
    selection.addRange(range);
  }, []);

  // a copy made with the keyboard or the context menu, the way to get the
  // value out when the clipboard api is withheld. only a selection that holds
  // the whole value counts: half a key on the clipboard is not a copy of it
  React.useEffect(() => {
    if (!value) return;
    const onCopy = () => {
      if (!window.getSelection()?.toString().includes(value)) return;
      setOutcome("by-hand");
      copied.current();
    };
    document.addEventListener("copy", onCopy);
    return () => document.removeEventListener("copy", onCopy);
  }, [value]);

  return (
    <div
      role="group"
      aria-labelledby={label ? labelId : undefined}
      className={cn("flex min-w-0 flex-col gap-1.5", className)}
    >
      {label && <FieldLabel id={labelId} label={label} />}
      <CopyableText
        ref={code}
        value={value}
        copyLabel={copyLabel}
        testId={testId}
        persistFailure
        onCopyStateChange={(state) => {
          if (state === "copied") {
            setOutcome(null);
            copied.current();
          } else if (state === "failed") {
            setOutcome("failed");
            selectValue();
          } else {
            setOutcome((current) => (current === "failed" ? null : current));
          }
        }}
      />
      {outcome === "failed" && (
        <div
          role="alert"
          className="flex flex-col gap-1.5 text-xs text-[color:var(--status-danger-text)]"
        >
          <p>{t("common.copyFailed")}</p>
          <div className="flex flex-wrap items-center gap-2">
            <span>{t("common.secret.copyByHand")}</span>
            <Button type="button" variant="outline" size="sm" onClick={selectValue}>
              {t("common.secret.select")}
            </Button>
          </div>
        </div>
      )}
      {outcome === "by-hand" && (
        <p role="status" className="text-xs text-[color:var(--status-success-text)]">
          {t("common.copied")}
        </p>
      )}
    </div>
  );
}

/**
 * Ask before an uncopied value is closed over (#2217).
 *
 * The shape is `useDiscardGuard`'s: `guard` answers a sheet's `onDismiss`
 * (Escape and the scrim), `close` is for the close button and Done, and
 * `prompt` renders among the modal's children. A value that has been copied
 * closes without a question, since a prompt that always appears is one people
 * learn to click through.
 *
 * `uncopied` is "the value is on screen and has not reached the clipboard".
 */
export function useSecretCloseGuard({
  name,
  uncopied,
  onOpenChange,
}: {
  /** stable key for the UX stream; the prompt reports as `<name>-close` */
  name: string;
  uncopied: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const [asking, setAsking] = React.useState(false);

  const guard = React.useCallback(() => {
    if (!uncopied) return true;
    // idempotent, so Escape twice raises one prompt rather than two
    setAsking(true);
    return false;
  }, [uncopied]);

  const close = React.useCallback(() => {
    if (guard()) onOpenChange(false);
  }, [guard, onOpenChange]);

  // a value copied under the prompt, or closed by its owner, has nothing left
  // to ask about
  React.useEffect(() => {
    if (!uncopied) setAsking(false);
  }, [uncopied]);

  const prompt = (
    <ConfirmDialog
      name={`${name}-close`}
      open={asking}
      onOpenChange={setAsking}
      title={t("common.secret.closeTitle")}
      description={t("common.secret.closeBody")}
      confirmLabel={t("common.secret.closeConfirm")}
      // closing is the question, not a removal: the value is gone either way
      tone="default"
      onConfirm={() => {
        setAsking(false);
        onOpenChange(false);
      }}
    />
  );

  return { guard, close, prompt };
}

/**
 * A dialog that reveals a value shown once, with its copy button and, below it,
 * the step that comes next.
 *
 * Mount it with `open` driven by the secret's own state, so the value is
 * discarded when the dialog closes. `children` is the next step: a snippet
 * that uses the value, or who to send it to. A snippet is a document, so the
 * dialog that carries one takes `size="lg"`: a line broken mid-token reads
 * worse than a wider panel (#948).
 */
export function SecretRevealDialog({
  name,
  open,
  onOpenChange,
  title,
  description,
  secret,
  secretLabel,
  copyLabel,
  doneLabel,
  size,
  children,
}: {
  /** stable key for the UX stream, such as `virtual-key-created`; never the value */
  name: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  secret: string;
  /** a visible name for the value, when the title and description do not name it */
  secretLabel?: string;
  copyLabel: string;
  /** overrides the shared `common.done` */
  doneLabel?: string;
  size?: "md" | "lg";
  /** the next step, below the value */
  children?: React.ReactNode;
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = React.useState(false);
  const { close, prompt } = useSecretCloseGuard({
    name,
    uncopied: open && !copied,
    onOpenChange,
  });

  // the next value starts uncopied. reset on the way out rather than on the
  // way in, so a reopened dialog never shows one frame of the last one's state
  React.useEffect(() => {
    if (!open) setCopied(false);
  }, [open]);

  const markCopied = React.useCallback(() => setCopied(true), []);

  return (
    <>
      <Dialog
        open={open}
        // Escape, the scrim and the header's close button all arrive here
        onOpenChange={(next) => (next ? onOpenChange(true) : close())}
        size={size}
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <SecretValue
            value={secret}
            label={secretLabel}
            copyLabel={copyLabel}
            onCopied={markCopied}
          />
          {children}
        </div>
        <DialogFooter>
          <Button onClick={close}>{doneLabel ?? t("common.done")}</Button>
        </DialogFooter>
      </Dialog>
      {prompt}
    </>
  );
}
