/**
 * Dashboard UX event emitters (#805).
 *
 * Browser spans (see `telemetry.ts`) say how long a screen took and whether it
 * threw. They do not say whether it was *usable*. This is the client half of
 * that stream: a queue in front of `POST /api/v1/ui-events`, plus the emitters
 * the dashboard calls.
 *
 * Three rules hold this module together, and each one exists because the
 * obvious implementation gets it wrong:
 *
 * 1. **Structural only.** Every field is a key, an enum, a duration or an id.
 *    `sanitizeKey` is not a formatting nicety — it is the last thing standing
 *    between a caller who passes `error.message` and a free-text column. A
 *    value that is not a plausible key is dropped here rather than sent, so
 *    the mistake costs one missing event instead of a leak.
 * 2. **Telemetry never degrades the product.** Every failure path is swallowed.
 *    A dashboard whose save button breaks because analytics 500'd is worse than
 *    a dashboard with no analytics.
 * 3. **Failure is terminal, not repeating.** A deployment without the endpoint
 *    (older control plane, reverse proxy that drops it, session that cannot
 *    authenticate) must cost one request, not one per flush forever.
 */
import { UI_EVENTS_MAX_BATCH, sendUiEvents, type UiEvent } from "@/lib/api";
import { currentTraceId } from "@/lib/telemetry";

/** Mirrors the server's `MAX_KEY_LEN`; a longer key is a 400, so drop it here. */
const MAX_KEY_LEN = 96;

/**
 * How long a partial batch waits before going out. Long enough that a burst of
 * interactions is one request, short enough that a tab closed by the process
 * manager rather than the user loses little.
 */
const FLUSH_INTERVAL_MS = 5_000;

/** Queue ceiling. Beyond this the oldest events are dropped rather than grown
 * without bound — an offline tab left open for a day must not accumulate. */
const MAX_QUEUE = 500;

const SESSION_STORAGE_KEY = "rolter.ux.session";

interface UxContext {
  orgId?: string;
  teamId?: string;
  projectId?: string;
}

let queue: UiEvent[] = [];
let timer: ReturnType<typeof setTimeout> | undefined;
let context: UxContext = {};
let sessionId = "";
let listenersBound = false;
/** Set after a response that says this endpoint will never work for us. */
let disabled = false;
let inFlight: Promise<void> = Promise.resolve();

/**
 * A stable id per tab. `sessionStorage` rather than `localStorage`: two tabs are
 * two sessions, and a session that outlived the tab would make dwell times and
 * navigation paths meaningless.
 */
function getSessionId(): string {
  if (sessionId) return sessionId;
  try {
    const stored = sessionStorage.getItem(SESSION_STORAGE_KEY);
    if (stored) {
      sessionId = stored;
      return sessionId;
    }
  } catch {
    // private mode or a blocked storage partition — fall through to a
    // per-load id, which still groups everything within this page load
  }
  sessionId = randomId();
  try {
    sessionStorage.setItem(SESSION_STORAGE_KEY, sessionId);
  } catch {
    // as above; the id is still useful for the life of this document
  }
  return sessionId;
}

/** Monotonic tail for the last-resort id, so uniqueness survives without WebCrypto. */
let idCounter = 0;

/**
 * A unique id for an event or a session.
 *
 * The session id is the one that matters: it groups a person's whole visit, so
 * a guessable one would let a caller file events into somebody else's session
 * and skew their funnel. `Math.random` is not good enough for that, and neither
 * is it good enough for CodeQL, which flags it here — correctly.
 *
 * `crypto.randomUUID` is the nice answer but requires a secure context, and a
 * dashboard served over plain http on a private network is a supported
 * deployment. `crypto.getRandomValues` carries no such restriction, so it is
 * the one that actually holds across the deployments rolter runs in.
 */
function randomId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    // not a secure context — fall through
  }
  try {
    const bytes = new Uint8Array(16);
    crypto.getRandomValues(bytes);
    return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  } catch {
    // no WebCrypto at all. ids still have to be distinct or every event
    // collides, so fall back to something merely unique rather than something
    // weakly random — a counter is honest about not being unpredictable
    idCounter += 1;
    return `seq-${Date.now().toString(36)}-${idCounter.toString(36)}`;
  }
}

/**
 * Reduce a caller-supplied string to something a `LowCardinality` column can
 * hold, or reject it.
 *
 * Rejecting rather than truncating is deliberate. Truncating a URL or an error
 * message produces a *plausible-looking* key that is really a fragment of free
 * text, which is exactly the value this stream must not carry; a truncated
 * secret is still a secret. Anything that does not already look like a stable
 * key is not one.
 */
export function sanitizeKey(value: string | undefined): string {
  if (!value) return "";
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > MAX_KEY_LEN) return "";
  // a key is a slug: letters, digits, and the separators the screen keys and
  // form names already use. a space, a quote, a slash or a colon means we were
  // handed a sentence, a path or a URL rather than a key
  if (!/^[a-zA-Z0-9._:-]+$/.test(trimmed)) return "";
  return trimmed;
}

/** Set the grouping labels carried on every subsequent event. */
export function setUxContext(next: UxContext): void {
  context = {
    orgId: sanitizeKey(next.orgId),
    teamId: sanitizeKey(next.teamId),
    projectId: sanitizeKey(next.projectId),
  };
}

function appVersion(): string {
  try {
    return sanitizeKey(__APP_VERSION__);
  } catch {
    return "";
  }
}

export interface TrackOptions {
  target?: string;
  fromScreen?: string;
  outcome?: UiEvent["outcome"];
  durationMs?: number;
}

/**
 * Queue one event. Silently does nothing when the screen key is unusable or
 * the endpoint has already refused us.
 */
export function track(
  screen: string,
  action: UiEvent["action"],
  options: TrackOptions = {},
): void {
  if (disabled) return;
  const screenKey = sanitizeKey(screen);
  // without a screen the event answers no question worth asking, and the
  // server requires it — dropping here keeps a bad key from failing the batch
  // it happens to share a request with
  if (!screenKey) return;

  const event: UiEvent = {
    event_id: randomId(),
    // stamped here, not in flush(): the queue holds events for up to
    // FLUSH_INTERVAL_MS and far longer while the tab is offline, so flush time
    // collapsed a whole session onto one instant (#1224)
    ts: new Date().toISOString(),
    screen: screenKey,
    action,
    session_id: getSessionId(),
  };

  const target = sanitizeKey(options.target);
  if (target) event.target = target;
  const fromScreen = sanitizeKey(options.fromScreen);
  if (fromScreen) event.from_screen = fromScreen;
  if (options.outcome) event.outcome = options.outcome;
  if (typeof options.durationMs === "number" && Number.isFinite(options.durationMs)) {
    // the column is UInt32; a negative or absurd duration is a bug upstream,
    // and clamping keeps it from becoming a 400 for the whole batch
    event.duration_ms = Math.min(Math.max(Math.round(options.durationMs), 0), 4_294_967_295);
  }

  const traceId = currentTraceId();
  if (traceId) event.trace_id = traceId;
  if (context.orgId) event.org_id = context.orgId;
  if (context.teamId) event.team_id = context.teamId;
  if (context.projectId) event.project_id = context.projectId;
  const version = appVersion();
  if (version) event.app_version = version;

  queue.push(event);
  if (queue.length > MAX_QUEUE) {
    // drop oldest: a tab that has been offline for hours is more interesting
    // for what just happened than for what happened this morning
    queue = queue.slice(-MAX_QUEUE);
  }

  bindListeners();
  if (queue.length >= UI_EVENTS_MAX_BATCH) {
    void flush();
  } else {
    scheduleFlush();
  }
}

function scheduleFlush(): void {
  if (timer !== undefined) return;
  timer = setTimeout(() => {
    timer = undefined;
    void flush();
  }, FLUSH_INTERVAL_MS);
}

/**
 * Send everything queued.
 *
 * `keepalive` rather than `navigator.sendBeacon`: the endpoint authenticates
 * the caller, and a beacon cannot carry an `Authorization` header. `keepalive`
 * survives the document being torn down and still goes through the normal
 * fetch path, so the session token is attached exactly as it is everywhere else.
 */
export async function flush(): Promise<void> {
  if (timer !== undefined) {
    clearTimeout(timer);
    timer = undefined;
  }
  if (disabled || queue.length === 0) return;

  const batch = queue.slice(0, UI_EVENTS_MAX_BATCH);
  queue = queue.slice(batch.length);

  // serialize flushes so a hide-triggered flush cannot interleave with the
  // timer's and reorder a session's events
  inFlight = inFlight.then(async () => {
    try {
      await sendUiEvents(batch);
    } catch (error) {
      const status = (error as { status?: number } | undefined)?.status;
      if (status === 401 || status === 403 || status === 404 || status === 405) {
        // this deployment will not accept these events — no session, no such
        // route on an older control plane, or a proxy that drops it. retrying
        // costs a request per flush for the life of the tab and never succeeds
        disabled = true;
        queue = [];
        return;
      }
      // anything else is plausibly transient, but re-queueing risks an
      // unbounded retry loop against a failing server. UX telemetry is not
      // worth that; the batch is dropped
    }
  });
  return inFlight;
}

function bindListeners(): void {
  if (listenersBound || typeof document === "undefined") return;
  listenersBound = true;
  // `visibilitychange` to hidden is the only reliable "the user is leaving"
  // signal on mobile — `beforeunload`/`unload` do not fire when a tab is
  // backgrounded and then discarded, which is how most sessions actually end
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") void flush();
  });
  if (typeof window !== "undefined") {
    // desktop back/forward and tab close, where visibilitychange may not fire
    window.addEventListener("pagehide", () => void flush());
  }
}

/** Test seam: drop the queue, context, session and circuit-breaker state. */
export function resetUxForTests(): void {
  queue = [];
  if (timer !== undefined) clearTimeout(timer);
  timer = undefined;
  context = {};
  sessionId = "";
  disabled = false;
  inFlight = Promise.resolve();
}

/** Test seam: the events queued but not yet sent. */
export function pendingUxEvents(): readonly UiEvent[] {
  return queue;
}

// --- emitters -------------------------------------------------------------
//
// Named wrappers rather than raw `track` calls at the call sites: the action
// enum is shared with a ClickHouse `Enum8`, and a typo at a call site should be
// a type error rather than a silently dropped row.

/** The user arrived on a screen. */
export const trackScreenView = (screen: string, fromScreen?: string) =>
  track(screen, "screen_view", { fromScreen });

/** The screen finished loading its primary data and became usable. */
export const trackTimeToInteractive = (screen: string, durationMs: number) =>
  track(screen, "time_to_interactive", { durationMs });

/** The user moved from one screen to another. */
export const trackNavigate = (screen: string, fromScreen: string) =>
  track(screen, "navigate", { fromScreen });

/** The user left via browser back rather than by acting on the screen. */
export const trackBackOut = (screen: string, fromScreen: string, durationMs?: number) =>
  track(screen, "back_out", { fromScreen, durationMs });

/** A form was submitted. `target` names the form, never its contents. */
export const trackFormSubmit = (
  screen: string,
  target: string,
  outcome: UiEvent["outcome"] = "ok",
  durationMs?: number,
) => track(screen, "form_submit", { target, outcome, durationMs });

/** A form was opened and closed without submitting. */
export const trackFormAbandon = (screen: string, target: string, durationMs?: number) =>
  track(screen, "form_abandon", { target, outcome: "cancelled", durationMs });

/** A validation rule rejected input. `rule` is the rule's name, not the value. */
export const trackValidationError = (screen: string, rule: string) =>
  track(screen, "validation_error", { target: rule, outcome: "error" });

/** A zero-data placeholder was shown. */
export const trackEmptyState = (screen: string, target?: string) =>
  track(screen, "empty_state", { target });

/** An error placeholder was shown. `target` names the region, not the message. */
export const trackErrorState = (screen: string, target?: string) =>
  track(screen, "error_state", { target, outcome: "error" });

/** A save round-tripped and the user saw the confirmation. */
export const trackSaveConfirmed = (screen: string, target: string, durationMs?: number) =>
  track(screen, "save_confirmed", { target, outcome: "ok", durationMs });
