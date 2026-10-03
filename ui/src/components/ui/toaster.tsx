import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { useToast, type ToastTone } from "@/lib/toast";
import { cn } from "@/lib/utils";

const ICON: Record<ToastTone, typeof Info> = {
  success: CircleCheck,
  error: CircleAlert,
  info: Info,
};

const ACCENT: Record<ToastTone, string> = {
  success: "text-[color:var(--status-success-text)]",
  error: "text-[color:var(--status-danger-text)]",
  info: "text-[color:var(--status-info-text)]",
};

// the stack in the bottom-right corner. two live regions rather than one:
// a failure is asserted, a success only mentioned, so a screen reader is
// interrupted by the former and not by the latter
export function Toaster() {
  const { t } = useTranslation();
  const { toasts, dismiss } = useToast();
  const errors = toasts.filter((toast) => toast.tone === "error");
  const rest = toasts.filter((toast) => toast.tone !== "error");

  return (
    <div
      className="pointer-events-none fixed bottom-4 right-4 z-[90] flex w-[min(380px,calc(100vw-2rem))] flex-col gap-2"
      // a bare <div> may not carry aria-label at all — the role has to support
      // a name, and the APG pattern for a toast stack is a named region around
      // the live regions rather than on them (#1181)
      role="region"
      aria-label={t("toast.region")}
      // drawn above every sheet and dialog, so it stays live while one makes
      // the rest of the page inert — an inert live region announces nothing
      // (see lib/modal-a11y.ts)
      data-above-modals
    >
      <div role="status" aria-live="polite" className="contents">
        {rest.map((toast) => (
          <ToastCard key={toast.id} {...toast} onDismiss={() => dismiss(toast.id)} />
        ))}
      </div>
      <div role="alert" aria-live="assertive" className="contents">
        {errors.map((toast) => (
          <ToastCard key={toast.id} {...toast} onDismiss={() => dismiss(toast.id)} />
        ))}
      </div>
    </div>
  );
}

function ToastCard({
  tone,
  title,
  detail,
  duration,
  onDismiss,
}: {
  tone: ToastTone;
  title: string;
  detail?: string;
  duration: number | null;
  onDismiss: () => void;
}) {
  const { t } = useTranslation();
  const Icon = ICON[tone];

  // the card's own clock, stopped while the pointer or focus is on it so it is
  // never taken away mid-read (WCAG 2.2.1), and restarted when it leaves
  const [hovered, setHovered] = React.useState(false);
  const [focused, setFocused] = React.useState(false);
  const paused = hovered || focused;
  const dismiss = React.useRef(onDismiss);
  dismiss.current = onDismiss;
  React.useEffect(() => {
    if (duration === null || paused) return;
    const timer = setTimeout(() => dismiss.current(), duration);
    return () => clearTimeout(timer);
  }, [duration, paused]);

  return (
    <div
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
      onFocus={() => setFocused(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false);
      }}
      className={cn(
        "rl-fade-in pointer-events-auto flex items-start gap-2.5 rounded-[10px] border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] px-3.5 py-3 shadow-[var(--shadow-lg)]",
      )}
    >
      <Icon aria-hidden className={cn("mt-0.5 h-4 w-4 flex-none", ACCENT[tone])} />
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-foreground">{title}</p>
        {detail && (
          <p className="mt-0.5 break-words font-mono text-xs text-muted-foreground">{detail}</p>
        )}
      </div>
      <button
        type="button"
        onClick={onDismiss}
        aria-label={t("toast.dismiss")}
        className="-mr-1 -mt-1 flex rounded-sm p-1 text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}
