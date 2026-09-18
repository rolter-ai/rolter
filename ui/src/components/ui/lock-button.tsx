import { Lock, LockOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

// the per-row padlock beside a parameter or a header: locked means the value
// is enforced server-side and a client request cannot override it. it is a
// toggle, so it reports `aria-pressed` rather than looking like a link
export interface LockButtonProps {
  locked: boolean;
  onToggle: () => void;
  disabled?: boolean;
}

export function LockButton({ locked, onToggle, disabled }: LockButtonProps) {
  const { t } = useTranslation();
  // the name says what the state *is* and what a press does, because the icon
  // alone leaves both to be guessed
  const label = locked ? t("common.lock.locked") : t("common.lock.unlocked");

  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onToggle}
      aria-pressed={locked}
      title={label}
      aria-label={label}
      className={cn(
        "flex h-8 w-8 flex-none items-center justify-center rounded-md border transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        locked
          ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-[color:var(--red-folk-text)]"
          : "border-[color:var(--border-subtle)] bg-transparent text-[color:var(--text-subtle)]",
      )}
    >
      {locked ? <Lock className="h-3.5 w-3.5" /> : <LockOpen className="h-3.5 w-3.5" />}
    </button>
  );
}
