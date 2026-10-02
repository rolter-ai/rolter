import * as React from "react";

// one-shot feedback for the whole dashboard (#1197): a save that went through,
// a delete that did not. inline messages stay for field-level validation,
// which belongs next to the field; anything that would otherwise vanish when
// a sheet closes goes here instead
export type ToastTone = "success" | "error" | "info";

export interface Toast {
  id: number;
  tone: ToastTone;
  title: string;
  /** optional second line — the control plane's own message on a failure */
  detail?: string;
  /**
   * milliseconds the card stays up while nobody is reading it, or `null` for
   * until it is dismissed by hand. The `Toaster` runs the clock, so it can stop
   * it while the pointer or focus is on the card (#2005, WCAG 2.2.1)
   */
  duration: number | null;
}

export interface ToastInput {
  tone?: ToastTone;
  title: string;
  detail?: string;
  /**
   * milliseconds before auto-dismiss. A success or an info lasts a few seconds;
   * an error stays until dismissed, since it carries the control plane's message
   * and has to be read, so only a caller that says otherwise gives it a clock
   */
  duration?: number;
}

interface ToastApi {
  toasts: Toast[];
  push: (input: ToastInput) => number;
  dismiss: (id: number) => void;
}

const ToastContext = React.createContext<ToastApi | null>(null);

// a success is glanced at; a failure has no timer at all (see `ToastInput`)
export const SUCCESS_MS = 4000;
// how many stay on screen at once; older ones drop off first
const MAX_VISIBLE = 4;

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = React.useState<Toast[]>([]);
  const counter = React.useRef(0);

  const dismiss = React.useCallback((id: number) => {
    setToasts((all) => all.filter((t) => t.id !== id));
  }, []);

  const push = React.useCallback(({ tone = "info", title, detail, duration }: ToastInput) => {
    const id = ++counter.current;
    const ms = duration ?? (tone === "error" ? null : SUCCESS_MS);
    setToasts((all) => [...all, { id, tone, title, detail, duration: ms }].slice(-MAX_VISIBLE));
    return id;
  }, []);

  const api = React.useMemo(() => ({ toasts, push, dismiss }), [toasts, push, dismiss]);
  return <ToastContext.Provider value={api}>{children}</ToastContext.Provider>;
}

const NOOP: ToastApi = { toasts: [], push: () => 0, dismiss: () => {} };

/**
 * The toast queue. Outside a provider — a story rendered on its own, a unit
 * test — it is a no-op rather than a thrown error, so a screen never has to
 * know whether the shell is around it.
 */
export function useToast(): ToastApi {
  return React.useContext(ToastContext) ?? NOOP;
}

/** the message an `ApiError` or a thrown value carries, for a toast's detail */
export function errorDetail(error: unknown): string | undefined {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return undefined;
}
