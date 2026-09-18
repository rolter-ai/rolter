/**
 * React bindings for the UX event stream (#805).
 *
 * `ux.ts` holds the queue and the emitters and knows nothing about React. This
 * is the layer that decides *when* the dashboard emits: route changes, screens
 * becoming interactive, forms opening and closing, placeholders rendering.
 *
 * The screen key travels through context rather than props. A shared component
 * such as `EmptyState` is rendered from forty-odd screens, and threading a
 * `screen` prop through every one of them would mean the instrumentation is
 * only as complete as the last person who remembered it. Context makes the
 * common case automatic and a missing provider merely silent.
 */
import * as React from "react";

import {
  setUxContext,
  trackBackOut,
  trackEmptyState,
  trackErrorState,
  trackFormAbandon,
  trackFormSubmit,
  trackNavigate,
  trackRefusedClick,
  trackRetrySubmit,
  trackSaveConfirmed,
  trackScreenView,
  trackTimeToInteractive,
  trackValidationError,
} from "@/lib/ux";

const ScreenContext = React.createContext<string>("");

/** Names the screen every nested emitter attributes its events to. */
export function UxScreenProvider({
  screen,
  children,
}: {
  screen: string;
  children: React.ReactNode;
}) {
  return <ScreenContext.Provider value={screen}>{children}</ScreenContext.Provider>;
}

/** The enclosing screen key, or `""` outside a provider (Storybook, tests). */
export function useUxScreen(): string {
  return React.useContext(ScreenContext);
}

/**
 * Whether the last history change was a browser back/forward rather than a
 * click in the dashboard.
 *
 * `popstate` fires before React Router re-renders, so a timestamp set here is
 * readable by the route effect that follows. The window is deliberately tight:
 * anything later than a frame or two is a different navigation.
 */
const POP_WINDOW_MS = 250;
let lastPopAt = 0;
if (typeof window !== "undefined") {
  window.addEventListener("popstate", () => {
    lastPopAt = Date.now();
  });
}

/**
 * Emit the navigation stream for the active screen.
 *
 * Called once, from the app shell — not per screen. It produces `screen_view`
 * on arrival, `navigate` with the previous screen so paths are queryable, and
 * `back_out` when the user left by pressing back rather than by acting.
 *
 * `back_out` carries the dwell time on the screen being left, which is the
 * number that makes it useful: backing out of a screen after eight seconds is a
 * navigation mistake, backing out after four minutes is a dead end.
 */
export function useRouteTelemetry(screen: string): void {
  const previous = React.useRef<string>("");
  const enteredAt = React.useRef<number>(Date.now());

  React.useEffect(() => {
    const from = previous.current;
    if (from === screen) return;

    if (from) {
      trackNavigate(screen, from);
      if (Date.now() - lastPopAt < POP_WINDOW_MS) {
        // attributed to the screen being *left*: "which screens do people back
        // out of" is the question, and answering it on the destination would
        // put the signal on the wrong row
        trackBackOut(from, screen, Date.now() - enteredAt.current);
      }
    }
    trackScreenView(screen, from || undefined);
    previous.current = screen;
    enteredAt.current = Date.now();
  }, [screen]);
}

/** Keep the grouping labels on emitted events in step with the scope picker. */
export function useUxContext(scope: { orgId?: string; teamId?: string; projectId?: string }): void {
  const { orgId, teamId, projectId } = scope;
  React.useEffect(() => {
    setUxContext({ orgId, teamId, projectId });
  }, [orgId, teamId, projectId]);
}

/**
 * Emit `time_to_interactive` when a screen's primary data lands.
 *
 * Pass the screen's own loading flag; the hook measures from mount to the first
 * time it goes false and fires once. Deliberately driven by the screen rather
 * than inferred centrally: only the screen knows which of its several queries
 * is the one the user is waiting for, and a guess would produce a number that
 * looks authoritative and is not.
 */
export function useScreenReady(ready: boolean, screen?: string): void {
  const contextScreen = useUxScreen();
  const key = screen ?? contextScreen;
  const mountedAt = React.useRef<number>(Date.now());
  const fired = React.useRef(false);

  React.useEffect(() => {
    if (!ready || fired.current || !key) return;
    fired.current = true;
    trackTimeToInteractive(key, Date.now() - mountedAt.current);
  }, [ready, key]);
}

/**
 * Emit `empty_state` once while a zero-data placeholder is on screen.
 *
 * Wired into the shared `EmptyState` component, so every screen that uses it is
 * instrumented without touching the screen. `target` names the list that came
 * back empty.
 */
export function useEmptyState(target?: string, screen?: string): void {
  const contextScreen = useUxScreen();
  const key = screen ?? contextScreen;
  const fired = React.useRef(false);

  React.useEffect(() => {
    if (fired.current || !key) return;
    fired.current = true;
    trackEmptyState(key, target);
  }, [key, target]);
}

/**
 * Emit `error_state` when an error placeholder appears.
 *
 * Fires on each transition into the error state rather than once per mount: a
 * screen that fails, is retried, and fails again is two incidents, and a hook
 * that reported one would flatten the retry loops worth finding.
 */
export function useErrorState(isError: boolean, target?: string, screen?: string): void {
  const contextScreen = useUxScreen();
  const key = screen ?? contextScreen;
  const wasError = React.useRef(false);

  React.useEffect(() => {
    if (isError && !wasError.current && key) {
      trackErrorState(key, target);
    }
    wasError.current = isError;
  }, [isError, key, target]);
}

export interface FormTelemetry {
  /** Call when the user submits. The first attempt after a failure is a retry. */
  submitted: () => void;
  /** Call when a save round-trips successfully. */
  saved: () => void;
  /** Call when a save fails. */
  failed: () => void;
  /** Call with the *name* of the rule that rejected input — never the value. */
  invalid: (rule: string) => void;
}

/**
 * Instrument one form.
 *
 * `open` is the form's own visibility, which is what makes abandonment
 * measurable: a form closed without a submit is an abandon, and the dwell
 * time separates "opened by mistake" from "tried and gave up". Those are
 * different problems and the duration is the only thing that tells them apart.
 *
 * A form goes away in one of two ways, and both are abandonments: `open` falls
 * to false while the form stays mounted, or the parent stops rendering it
 * altogether. Five screens use the second shape — they hold the draft in state
 * and mount the sheet only while it exists — so reading the closing edge alone
 * reported those five as *zero* abandonments, which reads as a clean result
 * rather than a missing one (#1739).
 *
 * `target` is the form's stable name (`provider-create`, `virtual-key`), never
 * anything derived from what was typed into it.
 */
export interface FormTelemetryOptions {
  /** overrides the screen key from the enclosing `UxScreenProvider` */
  screen?: string;
  /**
   * whether the draft differs from what it was seeded with. Read at the moment
   * the form goes away, which is what makes the abandon distinction possible: a
   * form closed clean is a misclick, a form closed dirty is somebody who filled
   * it in and gave up (#1731).
   */
  dirty?: boolean;
}

export function useFormTelemetry(
  target: string,
  open: boolean,
  options: FormTelemetryOptions = {},
): FormTelemetry {
  const contextScreen = useUxScreen();
  const key = options.screen ?? contextScreen;
  const openedAt = React.useRef<number>(0);
  const submitted = React.useRef(false);
  const failed = React.useRef(false);
  // read on the edge the form goes away, so it must not be an effect
  // dependency: adding it would re-run the effect — and restart the deferred
  // abandon below — on every keystroke that flips the draft
  const dirty = React.useRef(false);
  dirty.current = options.dirty ?? false;
  // an abandon the cleanup below has deferred, still cancellable. a token
  // rather than a boolean so a stale timer can never silence a later one
  const deferred = React.useRef<{ cancelled: boolean } | null>(null);

  React.useEffect(() => {
    // this effect running at all means the form is still mounted, so whatever
    // the previous cleanup deferred was not a teardown after all
    if (deferred.current) {
      deferred.current.cancelled = true;
      deferred.current = null;
    }

    if (open) {
      // the clock is only started on the *first* open. a re-run with the form
      // still open — a screen key arriving, StrictMode's remount — is not a
      // second opening, and restarting it there would report the dwell of the
      // last render instead of the dwell of the form
      if (!openedAt.current) {
        openedAt.current = Date.now();
        submitted.current = false;
        failed.current = false;
      }
      return () => {
        // the form went away while open. whether this is a real unmount or
        // StrictMode's simulated one is not knowable here — the double-invoke
        // looks exactly like a teardown — so the abandon is *deferred* rather
        // than emitted: a remount re-runs the effect in the same task and
        // cancels it above, a real unmount lets it through. emitting straight
        // from the cleanup would put a spurious abandon on every editor opened
        // in `bun run dev`
        if (!openedAt.current || submitted.current || !key) return;
        const duration = Date.now() - openedAt.current;
        // captured now rather than read in the timer: by the time it fires the
        // form is gone and a later render could have reset the ref
        const wasDirty = dirty.current;
        const token = { cancelled: false };
        deferred.current = token;
        setTimeout(() => {
          if (token.cancelled) return;
          deferred.current = null;
          openedAt.current = 0;
          trackFormAbandon(key, target, duration, wasDirty);
        }, 0);
      };
    }

    // the closing edge, with the form still mounted. a form that was never
    // opened has no dwell to report, and one that was submitted already told
    // its own story
    if (openedAt.current && !submitted.current && key) {
      trackFormAbandon(key, target, Date.now() - openedAt.current, dirty.current);
    }
    openedAt.current = 0;
  }, [open, key, target]);

  const dwell = () => (openedAt.current ? Date.now() - openedAt.current : undefined);

  return React.useMemo<FormTelemetry>(
    () => ({
      submitted: () => {
        submitted.current = true;
        // a submit after a failed one is a retry, not a second first attempt:
        // it is the moment somebody did not understand why the first failed,
        // and two identical form_submit rows hid that behind their timestamps
        const retry = failed.current;
        failed.current = false;
        if (!key) return;
        if (retry) trackRetrySubmit(key, target, dwell());
        else trackFormSubmit(key, target, "ok", dwell());
      },
      saved: () => {
        if (key) trackSaveConfirmed(key, target, dwell());
      },
      failed: () => {
        submitted.current = true;
        failed.current = true;
        if (key) trackFormSubmit(key, target, "error", dwell());
      },
      invalid: (rule: string) => {
        if (key) trackValidationError(key, rule);
      },
    }),
    [key, target],
  );
}

/**
 * The handler a refused control hangs on its wrapper so a denied reach is
 * recorded (#1731).
 *
 * The problem this solves is that a `disabled` button is inert: the HTML spec
 * has the user agent withhold the `click` event from a disabled form control,
 * so the one interaction worth measuring is the one the DOM refuses to report.
 * Every alternative to `disabled` was worse — `aria-disabled` plus a swallowed
 * handler makes a screen reader announce a control that is not one, and
 * removing the control entirely takes away the explanation of why it is
 * missing — so the control stays genuinely disabled and the event is caught
 * beside it instead.
 *
 * `pointerdown` in the **capture** phase on a wrapper is what catches it.
 * Capture runs on every node on the event's path before the target, and a
 * pointer event is dispatched to a disabled control where a mouse or click
 * event is not, so the wrapper sees the reach even though the button never
 * will. The wrapper is `display: contents`, so it is on the DOM path and
 * generates no box of its own — the button keeps its place in the parent's
 * layout exactly as before.
 *
 * A disabled control is not focusable, so there is no keyboard path to miss.
 * Nothing is deduplicated: reaching for the same refused control four times is
 * the signal, not noise.
 */
export function useRefusedClick(
  denied: boolean,
  control: string,
  capability: string | undefined,
  screen?: string,
): { onPointerDownCapture?: React.PointerEventHandler } {
  const contextScreen = useUxScreen();
  const key = screen ?? contextScreen;

  return React.useMemo(() => {
    if (!denied || !capability || !key) return {};
    return {
      onPointerDownCapture: () => trackRefusedClick(key, control, capability),
    };
  }, [denied, capability, control, key]);
}
