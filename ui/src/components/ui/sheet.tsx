import { X } from "lucide-react";
import * as React from "react";
import { createPortal } from "react-dom";

import { cn } from "@/lib/utils";

// minimal dependency-free right-side slide-over (no radix) — scrim + panel,
// mirrors the Rolter Design System sheet. Controlled via `open`/`onOpenChange`;
// `onDismiss` intercepts scrim/Escape closes (return false to keep it open),
// used for unsaved-changes guards.
export interface SheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onDismiss?: () => boolean;
  children: React.ReactNode;
}

export function Sheet({ open, onOpenChange, onDismiss, children }: SheetProps) {
  const dismiss = React.useCallback(() => {
    if (onDismiss && !onDismiss()) return;
    onOpenChange(false);
  }, [onDismiss, onOpenChange]);

  React.useEffect(() => {
    if (!open) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") dismiss();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open, dismiss]);

  if (!open) return null;

  return createPortal(
    <div className="fixed inset-0 z-[80] flex justify-end">
      <div
        className="absolute inset-0 bg-black/50 rl-fade-in"
        onClick={dismiss}
        aria-hidden
      />
      <div
        role="dialog"
        aria-modal="true"
        className={cn(
          "rl-sheet-in relative flex h-full w-full max-w-[580px] flex-col",
          "border-l border-[color:var(--border-default)] bg-[color:var(--surface-base)]",
          "shadow-[-14px_0_44px_rgba(0,0,0,0.42)]",
        )}
      >
        {children}
      </div>
    </div>,
    document.body,
  );
}

export function SheetHeader({
  title,
  subtitle,
  onClose,
}: {
  title: string;
  subtitle: string;
  onClose: () => void;
}) {
  return (
    <div className="flex flex-none items-start gap-3 border-b border-[color:var(--border-subtle)] px-[22px] py-[18px]">
      <div className="min-w-0 flex-1">
        <h2 className="text-lg font-semibold tracking-tight">{title}</h2>
        <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
          {subtitle}
        </p>
      </div>
      <button
        type="button"
        title="Close"
        aria-label="Close"
        onClick={onClose}
        className="flex flex-none rounded-md border border-[color:var(--border-subtle)] p-1.5 text-muted-foreground transition-colors hover:bg-[color:var(--surface-hover)] hover:text-foreground"
      >
        <X className="h-[17px] w-[17px]" />
      </button>
    </div>
  );
}

export function SheetBody({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-[22px] py-4">
      {children}
    </div>
  );
}

export function SheetFooter({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex-none border-t border-[color:var(--border-subtle)] bg-[color:var(--surface-base)]">
      {children}
    </div>
  );
}
