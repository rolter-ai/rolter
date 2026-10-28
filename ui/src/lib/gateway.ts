import i18n from "@/lib/i18n";

// client for the rolter-gateway data plane (/v1/*), used by the Playground,
// plus the readiness probe behind the screen header's gateway pill (#1973).
//
// the dashboard is served by the control plane, but chat/embeddings/image/audio
// calls hit the gateway on a different port. in dev, vite proxies /gw → :4000
// (see vite.config.ts); in prod the control plane must reverse-proxy /gw/*.
// the gateway authenticates with a virtual key: the Playground mints a
// short-lived one per sitting and keeps it here, in memory. the control plane
// guards /gw/* with the dashboard session, so every call carries two
// credentials: the session as `Authorization` and the key as
// `x-rolter-gateway-key`, which the proxy forwards as the gateway's own
// `Authorization: Bearer <key>` (#2486).

const GW_BASE = "/gw";

/**
 * The built-in model the gateway answers with no upstream or secrets. A
 * configured route of the same name shadows it, so a picker must not assume it
 * is on offer.
 */
export const FAKE_MODEL = "fake-llm";

/**
 * The key the Playground is currently sending, and when it stops working.
 *
 * `expiresAt` is set for a key rolter minted; a key the operator pasted by
 * hand carries no expiry here, because the dashboard did not choose one and
 * guessing at it would be worse than saying nothing.
 *
 * `models` is the reach the control plane wrote onto a key it minted, so the
 * screen can say when that reach is the built-in `fake-llm` alone (#2300). It
 * is empty for a pasted key, whose reach the dashboard never saw.
 */
export interface PlaygroundKeyState {
  key: string;
  expiresAt: string | null;
  /** rolter minted this key for this sitting, rather than a person pasting it */
  minted: boolean;
  /** when rolter minted it, by this tab's clock; `null` for a pasted key */
  mintedAt: number | null;
  models: readonly string[];
}

const NO_KEY: PlaygroundKeyState = {
  key: "",
  expiresAt: null,
  minted: false,
  mintedAt: null,
  models: [],
};

// deliberately a module variable and not `localStorage` (#944): a gateway
// credential written to browser storage outlives the sitting that needed it and
// stays there until somebody clears it. this one dies with the tab, and the
// Playground asks for a fresh one instead of keeping it.
let state: PlaygroundKeyState = NO_KEY;

const listeners = new Set<() => void>();

export function getPlaygroundKeyState(): PlaygroundKeyState {
  return state;
}

export function getPlaygroundKey(): string {
  return state.key;
}

/**
 * Set the key every gateway call below authenticates with.
 *
 * Passing an empty key clears it. The whole state object is replaced rather
 * than mutated so `useSyncExternalStore` sees a new reference and re-renders
 * the screen — and so a renewed key can never keep the previous expiry or
 * reach.
 */
export function setPlaygroundKey(
  key: string,
  options: { expiresAt?: string | null; minted?: boolean; models?: readonly string[] } = {},
): void {
  state = key
    ? {
        key,
        expiresAt: options.expiresAt ?? null,
        minted: options.minted ?? false,
        mintedAt: options.minted ? Date.now() : null,
        models: options.models ?? [],
      }
    : NO_KEY;
  for (const listener of listeners) listener();
}

/** Subscribe to key changes; returns the unsubscribe `useSyncExternalStore` wants. */
export function subscribePlaygroundKey(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// gateway calls the Playground has out right now. swapping the key under one of
// them would not change the request, but it would change what the screen says
// about the key that request is using, so a re-mint waits for this to reach 0
let inFlight = 0;
const inFlightListeners = new Set<() => void>();

export function getGatewayCallsInFlight(): number {
  return inFlight;
}

export function subscribeGatewayCalls(listener: () => void): () => void {
  inFlightListeners.add(listener);
  return () => {
    inFlightListeners.delete(listener);
  };
}

function notifyCalls(): void {
  for (const listener of inFlightListeners) listener();
}

/**
 * Count a call that outlives a single awaited promise — a realtime socket,
 * from the moment it is opened until it closes. Returns the release; calling
 * it more than once counts once.
 */
export function trackGatewaySession(): () => void {
  inFlight += 1;
  notifyCalls();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    inFlight -= 1;
    notifyCalls();
  };
}

async function tracked<T>(call: () => Promise<T>): Promise<T> {
  const release = trackGatewaySession();
  try {
    return await call();
  } finally {
    release();
  }
}

// the dashboard session token api/request.ts attaches to every control-plane call. the
// /gw proxy takes the same one; open mode has none, and then none is sent
const SESSION_STORAGE_KEY = "rolter.session.token";

function sessionToken(): string {
  try {
    return localStorage.getItem(SESSION_STORAGE_KEY) ?? "";
  } catch {
    return "";
  }
}

function authHeaders(json = true): Record<string, string> {
  const headers: Record<string, string> = {};
  const session = sessionToken();
  if (session) headers.Authorization = `Bearer ${session}`;
  const key = getPlaygroundKey();
  if (key) headers["x-rolter-gateway-key"] = key;
  if (json) headers["Content-Type"] = "application/json";
  return headers;
}

/**
 * A gateway answer that was not a success, carrying the status it came with.
 *
 * The message is what the operator reads; the status is what the caller
 * branches on, since "the key is not live yet" and "the gateway is down" call
 * for different next steps (#1853).
 */
export class GatewayError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = "GatewayError";
    this.status = status;
  }
}

// surface the gateway's OpenAI-style `{"error":{"message":...}}` body, else status
async function gwError(res: Response): Promise<GatewayError> {
  try {
    const body = (await res.json()) as { error?: { message?: string } };
    if (body?.error?.message) return new GatewayError(body.error.message, res.status);
  } catch {
    // not json
  }
  if (res.status === 401) {
    return new GatewayError(i18n.t("errors.gateway.unauthorized"), res.status);
  }
  return new GatewayError(
    i18n.t("errors.gateway.requestFailed", { status: res.status }),
    res.status,
  );
}

/**
 * Whether a gateway failure is the gateway refusing the key it was sent.
 *
 * The gateway answers a key it does not hold, a revoked one and an expired one
 * alike with `401` (`authenticate` in `crates/rolter-gateway/src/handlers.rs`),
 * so the status is the whole signal. Any other failure says something about
 * the gateway rather than the key, and must not be read as the key being bad.
 */
export function isKeyRefusal(error: unknown): boolean {
  return error instanceof GatewayError && error.status === 401;
}

/** The backoff a freshly minted key is given to reach the gateway. */
export interface KeyPropagationTiming {
  /** total time spent waiting between attempts before giving up */
  budgetMs: number;
  /** the wait before the first retry; each later one doubles it */
  firstDelayMs: number;
  /** the longest single wait, so the last attempts still land close together */
  maxDelayMs: number;
}

/** the gateway's own default for `ROLTER_SNAPSHOT_POLL_SECS` */
const DEFAULT_POLL_SECS = 5;

/**
 * The backoff for a deployment whose gateways poll every `pollSecs` (#1916).
 *
 * The gateway learns about a new key from its snapshot poll. One interval is
 * the worst case when the key lands just after a poll; the second is room for
 * a poll that was slow to answer. A fixed 10 s, sized for the 5 s default,
 * gave a deployment with a longer interval the #1853 race back on every load.
 */
export function keyPropagationFor(pollSecs: number | undefined): KeyPropagationTiming {
  const secs =
    pollSecs !== undefined && Number.isFinite(pollSecs) && pollSecs > 0
      ? pollSecs
      : DEFAULT_POLL_SECS;
  return { budgetMs: Math.max(10_000, 2 * secs * 1000), firstDelayMs: 250, maxDelayMs: 2_000 };
}

const runtimePollSecs = () =>
  typeof window === "undefined" ? undefined : window.__ROLTER_CONFIG__?.snapshotPollSecs;

let keyPropagation: KeyPropagationTiming = keyPropagationFor(runtimePollSecs());

/**
 * How long to wait before asking the gateway again, after `failures + 1`
 * failed attempts. `failures` counts from zero, the way TanStack Query's
 * `retryDelay` receives it.
 */
export function keyPropagationDelay(failures: number): number {
  return Math.min(keyPropagation.firstDelayMs * 2 ** failures, keyPropagation.maxDelayMs);
}

/**
 * Whether a gateway call made with a key rolter has just minted is worth
 * making again (#1853).
 *
 * The mint answers as soon as the row is written, but the gateway only learns
 * about the key on its next snapshot poll, so the first call made with it
 * answers `401` for a few seconds. Only a `401` is waited out: any other
 * failure says something about the gateway rather than about the key, and
 * retrying it would only delay the fallback the screen shows for it. The
 * total wait is bounded by the budget, so a key the gateway never accepts
 * still ends in the fallback rather than in a spinner.
 */
export function awaitingMintedKey(failures: number, error: unknown): boolean {
  if (!isKeyRefusal(error)) return false;
  let waited = 0;
  for (let n = 0; n <= failures; n += 1) waited += keyPropagationDelay(n);
  return waited <= keyPropagation.budgetMs;
}

/**
 * Shorten (or restore, with `null`) the propagation backoff, so a story can
 * reach the "never accepted" fallback without waiting out ten real seconds.
 */
export function setKeyPropagationForTests(timing: Partial<KeyPropagationTiming> | null): void {
  const base = keyPropagationFor(runtimePollSecs());
  keyPropagation = timing ? { ...base, ...timing } : base;
}

/**
 * How long to wait before sending a refused chat message once more, or `null`
 * when the refusal stands (#1916).
 *
 * The model list answering is no proof the key is live everywhere: behind a
 * load balancer the first message can reach a replica that has not polled
 * since the mint, and get a `401`. Within the propagation window of a key
 * rolter minted, that refusal is waited out once — for one more poll, or what
 * is left of the window — rather than shown. A pasted key, or one minted
 * longer ago than the window, is refused for a reason waiting will not fix.
 */
export function mintedKeyRetryDelay(
  key: Pick<PlaygroundKeyState, "minted" | "mintedAt">,
  now: number = Date.now(),
): number | null {
  if (!key.minted || key.mintedAt === null) return null;
  const left = keyPropagation.budgetMs - (now - key.mintedAt);
  if (left <= 0) return null;
  const poll = keyPropagation.budgetMs / 2;
  return Math.max(keyPropagation.firstDelayMs, Math.min(poll, left));
}

/** One addressable id from the gateway's own catalogue. */
export interface GatewayModel {
  id: string;
  /** the route, provider or provider group this id belongs to */
  owned_by?: string;
  rolter_capabilities?: { decisions: boolean; chat: boolean };
}

/**
 * What the **gateway** will actually serve, which is not the same set as the
 * control plane's route list.
 *
 * `GET /api/v1/models` on the control plane returns configured routes. The
 * gateway additionally serves `provider-slug/model` pins and `group-slug/model`
 * provider-group addresses (ADR-0017 addendum). Listing routes alone means a
 * provider group can be configured, be live, and be impossible to select in the
 * Playground — the one screen whose job is to send it a request.
 *
 * Needs a virtual key, because it is a gateway call like any other; callers
 * fall back to the control-plane list when there is no key to use. A refusal
 * throws a {@link GatewayError}, so the caller can tell a key the gateway has
 * not picked up yet from one it never will.
 */
export async function fetchGatewayModels(signal?: AbortSignal): Promise<GatewayModel[]> {
  // counted like any other call, so a re-mint cannot swap the key mid-probe
  return tracked(async () => {
    const res = await fetch(`${GW_BASE}/v1/models`, {
      headers: authHeaders(false),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const body = (await res.json()) as { data?: GatewayModel[] };
    return body.data ?? [];
  });
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  // string, or OpenAI multimodal content parts (text + image_url)
  content:
    | string
    | Array<{ type: "text"; text: string } | { type: "image_url"; image_url: { url: string } }>;
}

export async function chatCompletion(
  model: string,
  messages: ChatMessage[],
  signal?: AbortSignal,
): Promise<string> {
  return tracked(async () => {
    const res = await fetch(`${GW_BASE}/v1/chat/completions`, {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ model, messages, stream: false }),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const body = (await res.json()) as {
      choices?: { message?: { content?: string } }[];
    };
    return body.choices?.[0]?.message?.content ?? "";
  });
}

/** The token counts the gateway reports for a reply, when it reports them. */
export interface ChatUsage {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
}

/** What a finished chat stream leaves behind besides the text it delivered. */
export interface ChatStreamResult {
  text: string;
  /** from the final chunk (`stream_options.include_usage`); null when the gateway sent none */
  usage: ChatUsage | null;
  /** milliseconds from sending the request to the end of the stream */
  elapsedMs: number;
}

/** One `data:` payload of the chat stream, as far as the dashboard reads it. */
interface ChatChunk {
  choices?: { delta?: { content?: string | null }; finish_reason?: string | null }[];
  usage?: ChatUsage | null;
  error?: { message?: string };
}

/**
 * Ask for a chat completion as a server-sent event stream and hand the text to
 * `onText` as it arrives (#3096).
 *
 * `onText` receives the whole reply so far, not the new piece, so a caller that
 * renders it needs no accumulator of its own. The call counts as in flight from
 * the request until the stream ends or is aborted, not only until the headers
 * arrive, because the key is in use for all of it (#2758). Aborting `signal`
 * rejects with the abort error; what had arrived is already with `onText`.
 *
 * A stream that ends without its `[DONE]` or a finish reason was cut short and
 * rejects with a {@link GatewayError}, as does an `error` object the gateway
 * sends in place of a chunk. A gateway that answers with plain JSON, whatever
 * `stream` said, is read as the whole reply in one piece.
 */
export async function streamChatCompletion(
  model: string,
  messages: ChatMessage[],
  onText: (text: string) => void,
  signal?: AbortSignal,
): Promise<ChatStreamResult> {
  return tracked(async () => {
    const started = performance.now();
    const res = await fetch(`${GW_BASE}/v1/chat/completions`, {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({
        model,
        messages,
        stream: true,
        stream_options: { include_usage: true },
      }),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const elapsed = () => Math.round(performance.now() - started);

    if (!res.body || (res.headers.get("content-type") ?? "").includes("application/json")) {
      const body = (await res.json()) as {
        choices?: { message?: { content?: string } }[];
        usage?: ChatUsage | null;
      };
      const text = body.choices?.[0]?.message?.content ?? "";
      onText(text);
      return { text, usage: body.usage ?? null, elapsedMs: elapsed() };
    }

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    let text = "";
    let usage: ChatUsage | null = null;
    let finished = false;

    // one event is the lines up to a blank line; only its `data:` lines matter
    const take = (event: string) => {
      const data = event
        .split(/\r?\n/)
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trimStart())
        .join("\n");
      if (!data) return;
      if (data === "[DONE]") {
        finished = true;
        return;
      }
      let chunk: ChatChunk;
      try {
        chunk = JSON.parse(data) as ChatChunk;
      } catch {
        return;
      }
      if (chunk.error) {
        throw new GatewayError(
          chunk.error.message ?? i18n.t("errors.gateway.streamEnded"),
          res.status,
        );
      }
      if (chunk.usage) usage = chunk.usage;
      const choice = chunk.choices?.[0];
      if (choice?.finish_reason) finished = true;
      const piece = choice?.delta?.content;
      if (piece) {
        text += piece;
        onText(text);
      }
    };

    for (;;) {
      const { done, value } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      const events = buffer.split(/\r?\n\r?\n/);
      buffer = done ? "" : (events.pop() ?? "");
      for (const event of events) take(event);
      if (done) break;
    }
    if (!finished) throw new GatewayError(i18n.t("errors.gateway.streamEnded"), res.status);
    return { text, usage, elapsedMs: elapsed() };
  });
}

export interface DecisionRequest {
  model: string;
  state: string | Record<string, unknown> | unknown[];
  questions: Record<string, { type: string; instructions: string; criteria?: unknown }>;
  /** OpenJev's single shared image: base64 or an image data URI. */
  image_data?: string;
}

export interface DecisionAnswer {
  type?: string;
  choice?: string;
  score?: number;
  noul?: number;
  confidence?: number;
  probabilities?: Record<string, number>;
  legend?: Record<string, string>;
  error?: unknown;
}

export interface DecisionResponse {
  model?: string;
  answers: Record<string, DecisionAnswer>;
  usage?: { input_tokens?: number; output_tokens?: number };
}

/** Typed decisions use the same session/key split and in-flight tracking as chat. */
export async function decide(
  request: DecisionRequest,
  signal?: AbortSignal,
): Promise<DecisionResponse> {
  return tracked(async () => {
    const res = await fetch(`${GW_BASE}/v1/systemone`, {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify(request),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    return (await res.json()) as DecisionResponse;
  });
}

export async function embed(
  model: string,
  input: string[],
  signal?: AbortSignal,
): Promise<number[][]> {
  return tracked(async () => {
    const res = await fetch(`${GW_BASE}/v1/embeddings`, {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ model, input }),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const body = (await res.json()) as { data?: { embedding: number[] }[] };
    return (body.data ?? []).map((d) => d.embedding);
  });
}

export interface GeneratedImage {
  // a data: URL ready to drop into <img src>
  url: string;
}

export async function generateImages(
  model: string,
  prompt: string,
  n: number,
  size: string,
  signal?: AbortSignal,
): Promise<GeneratedImage[]> {
  return tracked(async () => {
    const res = await fetch(`${GW_BASE}/v1/images/generations`, {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ model, prompt, n, size }),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const body = (await res.json()) as {
      data?: { url?: string; b64_json?: string }[];
    };
    return (body.data ?? []).map((d) => ({
      url: d.b64_json ? `data:image/png;base64,${d.b64_json}` : (d.url ?? ""),
    }));
  });
}

// text → speech: returns an object URL for an <audio> element
export async function synthesizeSpeech(
  model: string,
  input: string,
  voice: string,
  signal?: AbortSignal,
): Promise<string> {
  return tracked(async () => {
    const res = await fetch(`${GW_BASE}/v1/audio/speech`, {
      method: "POST",
      headers: authHeaders(),
      body: JSON.stringify({ model, input, voice }),
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const blob = await res.blob();
    return URL.createObjectURL(blob);
  });
}

// speech → text: multipart upload of an audio file
export async function transcribe(model: string, file: File, signal?: AbortSignal): Promise<string> {
  return tracked(async () => {
    const form = new FormData();
    form.append("model", model);
    form.append("file", file);
    const res = await fetch(`${GW_BASE}/v1/audio/transcriptions`, {
      method: "POST",
      headers: authHeaders(false),
      body: form,
      signal,
    });
    if (!res.ok) throw await gwError(res);
    const body = (await res.json()) as { text?: string };
    return body.text ?? "";
  });
}

// realtime WebSocket URL (same-origin via the /gw proxy, ws upgrade enabled).
// browsers can't set WS headers, so both credentials ride as query params: the
// virtual key as `api_key`, the dashboard session as `rolter_session`, which
// the proxy strips before it reaches the gateway (#2486).
export function realtimeUrl(model: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const key = getPlaygroundKey();
  const session = sessionToken();
  const params = new URLSearchParams({ model });
  if (key) params.set("api_key", key);
  if (session) params.set("rolter_session", session);
  return `${proto}//${location.host}${GW_BASE}/v1/realtime?${params.toString()}`;
}

/**
 * Where a client outside the browser reaches the gateway.
 */
export interface GatewayBase {
  /** the gateway's root, with no trailing slash and no `/v1` — a snippet appends that */
  url: string;
}

/**
 * The address every snippet, example and placeholder in the dashboard hands
 * to an external client (#2218), or `null` when none is saved.
 *
 * It is the public base URL saved on Client Settings and nothing else. The
 * dashboard's `/gw` proxy is not a fallback any more (#2486): it requires a
 * dashboard session, which a client outside the browser does not have, so a
 * snippet built on it would be a 401. `null` means "no address to show", and
 * the caller prompts for one instead of inventing it.
 *
 * A saved value is trimmed of trailing slashes and of one trailing `/v1`,
 * since the OpenAI SDKs document their base URL with the version on it and an
 * operator pasting one would otherwise get `/v1/v1/chat/completions`.
 *
 * Pure, so both branches are unit-testable; screens read it through
 * `useGatewayBase()`, which supplies the saved value.
 */
export function gatewayBase(publicBaseUrl?: string | null): GatewayBase | null {
  const saved = (publicBaseUrl ?? "").trim().replace(/\/+$/, "").replace(/\/v1$/, "");
  return saved ? { url: saved } : null;
}

/**
 * What one readiness probe of the gateway established (#1973).
 *
 * - `ready`: the gateway answered its `/readyz` with `200 ok`
 * - `draining`: it answered `503 draining`, so it finishes the requests in
 *   flight and takes no new ones
 * - `unreachable`: the control plane's `/gw` proxy answered `502` in its own
 *   JSON error shape, which it does only when the request to the gateway
 *   itself failed
 * - `awaitingSnapshot`: it answered `503 awaiting_snapshot`. It is managed by a
 *   control plane and has applied no config snapshot yet, so it refuses every
 *   key (#3348)
 * - `snapshotFormatUnsupported`: it answered `503 snapshot_format_unsupported`.
 *   It has never applied a snapshot and the control plane serves none in a
 *   format it reads, so it refuses every key until the two builds agree
 *   (ADR-0038, #3350)
 */
export type GatewayReadiness =
  "ready" | "draining" | "unreachable" | "awaitingSnapshot" | "snapshotFormatUnsupported";

// the bodies a gateway's `/readyz` answers `503` with, and what each one means
const NOT_READY: Record<string, GatewayReadiness> = {
  draining: "draining",
  awaiting_snapshot: "awaitingSnapshot",
  snapshot_format_unsupported: "snapshotFormatUnsupported",
};

/**
 * A probe answer that says nothing about the gateway: an unexpected status, a
 * page from something in front of the control plane, a network failure or no
 * answer within the timeout. `status` is `0` when there was no response.
 */
export class GatewayProbeError extends Error {
  readonly status: number;

  constructor(status: number) {
    super(`gateway readiness probe: ${status}`);
    this.name = "GatewayProbeError";
    this.status = status;
  }
}

/**
 * Read a `/gw/readyz` answer, or `null` when it is not one the gateway or the
 * `/gw` proxy gives.
 *
 * Strict on purpose. A 200 has to carry the gateway's own `ok`, because an SPA
 * fallback in front of the control plane answers any path with 200 and
 * `index.html`. A 401 is the proxy refusing the dashboard session, which says
 * nothing about the gateway, so it is unknown too. A 502 has to carry the proxy's `{"error":{"message":…}}`,
 * because an ingress that cannot reach the *control plane* answers 502 too,
 * and calling that "gateway unreachable" would send the operator to the wrong
 * process. Anything else is unknown rather than guessed at.
 */
export function readinessFrom(status: number, body: string): GatewayReadiness | null {
  if (status === 200) return body.trim() === "ok" ? "ready" : null;
  if (status === 503) {
    const answer = body.trim();
    return Object.prototype.hasOwnProperty.call(NOT_READY, answer) ? NOT_READY[answer] : null;
  }
  if (status === 502) {
    try {
      const parsed = JSON.parse(body) as { error?: { message?: unknown } };
      return typeof parsed?.error?.message === "string" ? "unreachable" : null;
    } catch {
      return null;
    }
  }
  return null;
}

// the /gw proxy sets no timeout on its request to the gateway, so a gateway
// host that drops packets holds the probe open until the OS gives up on the
// connection (#2014). the pill stops waiting well before its next poll
const READINESS_TIMEOUT_MS = 10_000;

/**
 * Ask the gateway, through the control plane's `/gw` proxy, whether it is
 * taking traffic (#1973).
 *
 * The `/gw` proxy requires a dashboard session (#2486), so the session goes as
 * `Authorization: Bearer`; open mode has none and sends none. The gateway's
 * `/readyz` takes no key, so no Playground key is sent, and the proxy never
 * forwards the session. Throws {@link GatewayProbeError} for any answer
 * {@link readinessFrom} does not recognise.
 */
export async function fetchGatewayReadiness(signal?: AbortSignal): Promise<GatewayReadiness> {
  const controller = new AbortController();
  const abort = () => controller.abort();
  const timer = setTimeout(abort, READINESS_TIMEOUT_MS);
  signal?.addEventListener("abort", abort);
  try {
    let res: Response;
    try {
      const session = sessionToken();
      res = await fetch(`${GW_BASE}/readyz`, {
        cache: "no-store",
        headers: session ? { Authorization: `Bearer ${session}` } : {},
        signal: controller.signal,
      });
    } catch {
      throw new GatewayProbeError(0);
    }
    const readiness = readinessFrom(res.status, await res.text());
    if (!readiness) throw new GatewayProbeError(res.status);
    return readiness;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
  }
}
