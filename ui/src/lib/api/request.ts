import i18n from "@/lib/i18n";

// bearer token from a real login (see lib/auth.tsx); attached to every request
// so the session-scoped /me/* endpoints authenticate. absent in open-mode /
// email-only sessions, where the control plane needs no auth anyway.
const TOKEN_STORAGE_KEY = "rolter.session.token";

function authHeaders(): Record<string, string> {
  try {
    const token = localStorage.getItem(TOKEN_STORAGE_KEY);
    return token ? { Authorization: `Bearer ${token}` } : {};
  } catch {
    return {};
  }
}

export class ApiError extends Error {
  readonly status: number;
  /**
   * Stable machine-readable reason from the control plane's
   * `{"error": {"code": ...}}` body, when it sent one. Screens branch on this
   * rather than on the message, which is free to be reworded and translated.
   */
  readonly code?: string;
  /**
   * Seconds the caller was told to wait, from the response's `Retry-After`
   * header. Present on a `429` — a lock is a clock, and a screen that can read
   * it does not have to make the user poll to find out (#1079).
   */
  readonly retryAfterSeconds?: number;
  /**
   * The request field a `400 invalid_field` refusal is about, from the body's
   * `{"error": {"field": ...}}`. A form pins the message to that input rather
   * than guessing from the wording (#2804).
   */
  readonly field?: string;

  constructor(
    message: string,
    status: number,
    code?: string,
    retryAfterSeconds?: number,
    field?: string,
  ) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.retryAfterSeconds = retryAfterSeconds;
    this.field = field;
  }
}

/**
 * The control plane is running with no admin token, so it has no local accounts
 * and the per-user `/me/*` endpoints cannot be served to anyone (#942).
 *
 * A plain 401 means "sign in"; this one cannot be fixed that way, so the two
 * must not render alike.
 */
export function isOpenModeNoSession(error: unknown): boolean {
  return error instanceof ApiError && error.code === "open_mode_no_session";
}

/**
 * The control plane has no database, so it has no sessions, and the admin token
 * is the only credential its analytics and health routes accept (#1909). A
 * plain 401 means "sign in again"; here signing in again with an e-mail cannot
 * work, and only entering the token can.
 */
export function isAdminTokenRequired(error: unknown): boolean {
  return error instanceof ApiError && error.code === "admin_token_required";
}

// the shell's "this token is dead" handler, registered by AuthProvider (#1196).
// a module-level callback rather than an import so the api client keeps knowing nothing
// about react, and so a request that 401s in a screen nobody is watching still
// signs the session out once, centrally, instead of leaving every screen to
// reach LoadError with the same dead token still attached
type SessionExpiredHandler = () => void;

let sessionExpiredHandler: SessionExpiredHandler | null = null;

/**
 * Register the handler called when a request made *with a session token* is
 * answered 401. Returns an unsubscribe, so a provider can drop it on unmount.
 */
export function setSessionExpiredHandler(handler: SessionExpiredHandler | null): () => void {
  sessionExpiredHandler = handler;
  return () => {
    if (sessionExpiredHandler === handler) sessionExpiredHandler = null;
  };
}

// endpoints whose 401 is about the credentials in the request, not about the
// session token that happens to be in localStorage: a wrong password and an
// invite token that expired are both answered 401, and neither means the
// current session died. `/auth/mfa/` covers the step-up and enrolment at
// sign-in (#1852): their 401 is about the challenge token in the body
const SESSION_EXEMPT_PATHS = [
  "/api/v1/auth/login",
  "/api/v1/auth/mfa/",
  "/api/v1/invitations/accept/",
];

/// signal a dead session, but only for the failures that actually are one
function noteUnauthorized(url: string, authed: boolean, err: ApiError) {
  if (!authed || err.status !== 401) return;
  // open mode has no accounts at all, so there is no session to have expired
  // and no sign-in that would help (#942)
  if (isOpenModeNoSession(err)) return;
  // a missing or wrong admin token is explained where it is hit, with the
  // screen's own LoadError, rather than by signing out and showing the same
  // prompt again (#1909)
  if (isAdminTokenRequired(err)) return;
  if (SESSION_EXEMPT_PATHS.some((path) => url.includes(path))) return;
  sessionExpiredHandler?.();
}

/**
 * The control plane answered, and the answer was "this endpoint is not
 * mounted here": every CRUD and settings route only exists when it runs with a
 * database (`--database-url`). A config-file-only deployment therefore 404s
 * Users, Keys, Providers and every settings screen, which is a deployment
 * shape, not a bug in the request (#1204).
 *
 * Newer control planes send `code: no_such_endpoint`; older ones only the
 * message, which is matched on its prefix so the screen still explains itself.
 */
export function isEndpointNotMounted(error: unknown): boolean {
  return (
    error instanceof ApiError &&
    error.status === 404 &&
    (error.code === "no_such_endpoint" || error.message.startsWith("no such endpoint"))
  );
}

/**
 * How long the dashboard waits on one control-plane call before giving up, in
 * milliseconds, or `undefined` for no limit (#3147).
 *
 * The control plane answers `504 request_timeout` once a request outlives its
 * own bound (`ROLTER_REQUEST_TIMEOUT_SECS`, 60 s by default), so this waits a
 * little longer than that: the server's coded answer is the one to show, and
 * the client's own abort only covers a connection that never answers at all
 * (a proxy holding it open, a control plane that died mid-request).
 */
export function clientRequestBoundMs(
  config: { requestTimeoutSecs?: number } | undefined = typeof window === "undefined"
    ? undefined
    : window.__ROLTER_CONFIG__,
): number | undefined {
  const secs = config?.requestTimeoutSecs ?? DEFAULT_SERVER_BOUND_SECS;
  if (!Number.isFinite(secs) || secs <= 0) return undefined;
  return secs * 1000 + CLIENT_BOUND_GRACE_MS;
}

/** the control plane's default bound, assumed when the served page names none */
const DEFAULT_SERVER_BOUND_SECS = 60;

/** how much longer than the server the dashboard waits before aborting */
const CLIENT_BOUND_GRACE_MS = 5_000;

/// `fetch`, abandoned with a coded `request_timeout` once the control plane has
/// had longer than its own bound to answer
async function boundedFetch(
  url: string,
  init: RequestInit = {},
  signal?: AbortSignal,
): Promise<Response> {
  const bound = clientRequestBoundMs();
  if (bound === undefined) return fetch(url, signal ? { ...init, signal } : init);
  const controller = new AbortController();
  // the caller's own abort goes through the same controller, and stays
  // attached after the headers arrive so it also stops a body being read
  if (signal?.aborted) controller.abort(signal.reason);
  else signal?.addEventListener("abort", () => controller.abort(signal.reason), { once: true });
  const timer = setTimeout(() => controller.abort(), bound);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } catch (error) {
    // the caller's own abort is theirs to read, not a timeout
    if (controller.signal.aborted && !signal?.aborted) {
      throw new ApiError(i18n.t("errors.api.codes.request_timeout"), 504, "request_timeout");
    }
    throw error;
  } finally {
    clearTimeout(timer);
  }
}

/// extract the control api's `{"error": {"message": ..., "code": ...}}` body
/// when present, falling back to the raw status text
async function apiError(res: Response): Promise<ApiError> {
  // only the delta-seconds form is sent by rolter. the http-date form is legal
  // and unhandled, and an absent header reads as 0 through `Number` — both are
  // dropped rather than rendered, since "retry in NaN seconds" and "retry in 0
  // seconds" are each worse than saying nothing
  const raw = res.headers.get("retry-after");
  const seconds = raw === null ? Number.NaN : Number(raw);
  const retryAfter = Number.isFinite(seconds) && seconds > 0 ? seconds : undefined;
  try {
    const body = (await res.json()) as {
      error?: { message?: string; code?: string; field?: string };
    };
    if (body?.error?.message) {
      return new ApiError(
        body.error.message,
        res.status,
        body.error.code,
        retryAfter,
        typeof body.error.field === "string" ? body.error.field : undefined,
      );
    }
  } catch {
    // not json, fall through
  }
  return new ApiError(
    i18n.t("errors.requestFailed", { status: res.status }),
    res.status,
    undefined,
    retryAfter,
  );
}

/** Options for {@link request}; every call to the control plane is described by one. */
export interface RequestOptions {
  /** absent means GET */
  method?: "POST" | "PUT" | "PATCH" | "DELETE";
  /** JSON-encoded and sent with a `Content-Type`; omit for a body-less request */
  body?: unknown;
  /**
   * attach the stored session token (default true). a request made mid sign-in
   * sends none, so a stale token cannot ride along
   */
  auth?: boolean;
  /** replace the error thrown for a failed response; return nothing to keep `err` */
  mapError?: (res: Response, err: ApiError) => Error | undefined;
  /**
   * abandon the request when this aborts, the body of a streamed answer
   * included; the `AbortError` is thrown as it is, not as a timeout
   */
  signal?: AbortSignal;
}

/**
 * The one place a control-plane call is made: headers, JSON encoding, the
 * request bound and the `apiError` mapping all live here, and the response is
 * handed back untouched for a caller that reads something other than JSON.
 */
export async function requestResponse(
  url: string,
  options: RequestOptions = {},
): Promise<Response> {
  const { method, body, auth = true, mapError, signal } = options;
  const headers = auth ? authHeaders() : {};
  const init: RequestInit =
    method === undefined
      ? { headers }
      : {
          method,
          headers: {
            ...headers,
            ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
          },
          body: body !== undefined ? JSON.stringify(body) : undefined,
        };
  const res = await boundedFetch(url, init, signal);
  if (!res.ok) {
    const err = await apiError(res);
    const mapped = mapError?.(res, err);
    if (mapped) throw mapped;
    // a 401 made with a token signals a dead session, whichever route it came from
    if (auth) noteUnauthorized(url, "Authorization" in headers, err);
    throw err;
  }
  return res;
}

/**
 * How a successful body is read: `json` decodes it, `json-or-empty` also
 * accepts a `204` (a write that answers with nothing), `text` returns it as
 * is — the collector config is `application/yaml`, and parsing it would only
 * be a step towards printing it again.
 */
export type ResponseParse = "json" | "json-or-empty" | "text";

/** {@link requestResponse}, with the body decoded as `parse` says (default `json`) */
export async function request<T>(
  url: string,
  options: RequestOptions & { parse?: ResponseParse } = {},
): Promise<T> {
  const res = await requestResponse(url, options);
  switch (options.parse ?? "json") {
    case "text":
      return (await res.text()) as T;
    case "json-or-empty":
      if (res.status === 204) return undefined as T;
      return (await res.json()) as T;
    default:
      return (await res.json()) as T;
  }
}

export function getJson<T>(url: string): Promise<T> {
  return request<T>(url);
}

/// the same request as `getJson` for an endpoint that answers with a document
/// rather than a record
export function getText(url: string): Promise<string> {
  return request<string>(url, { parse: "text" });
}

export function sendJson<T>(
  method: "POST" | "PUT" | "PATCH" | "DELETE",
  url: string,
  body?: unknown,
): Promise<T> {
  return request<T>(url, { method, body, parse: "json-or-empty" });
}

export interface DataEnvelope<T> {
  data: T[];
}
