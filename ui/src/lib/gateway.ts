import i18n from "@/lib/i18n";

// client for the rolter-gateway data plane (/v1/*), used by the Playground.
//
// the dashboard is served by the control plane, but chat/embeddings/image/audio
// calls hit the gateway on a different port. in dev, vite proxies /gw → :4000
// (see vite.config.ts); in prod the control plane must reverse-proxy /gw/*.
// the gateway authenticates with a virtual key: the Playground mints a
// short-lived one per sitting and keeps it here, in memory.

const GW_BASE = "/gw";

/**
 * The key the Playground is currently sending, and when it stops working.
 *
 * `expiresAt` is set for a key rolter minted; a key the operator pasted by
 * hand carries no expiry here, because the dashboard did not choose one and
 * guessing at it would be worse than saying nothing.
 */
export interface PlaygroundKeyState {
  key: string;
  expiresAt: string | null;
  /** rolter minted this key for this sitting, rather than a person pasting it */
  minted: boolean;
}

const NO_KEY: PlaygroundKeyState = { key: "", expiresAt: null, minted: false };

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
 * the screen — and so a renewed key can never keep the previous expiry.
 */
export function setPlaygroundKey(
  key: string,
  options: { expiresAt?: string | null; minted?: boolean } = {},
): void {
  state = key
    ? { key, expiresAt: options.expiresAt ?? null, minted: options.minted ?? false }
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

function authHeaders(json = true): Record<string, string> {
  const headers: Record<string, string> = {};
  const key = getPlaygroundKey();
  if (key) headers.Authorization = `Bearer ${key}`;
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

/** The backoff a freshly minted key is given to reach the gateway. */
export interface KeyPropagationTiming {
  /** total time spent waiting between attempts before giving up */
  budgetMs: number;
  /** the wait before the first retry; each later one doubles it */
  firstDelayMs: number;
  /** the longest single wait, so the last attempts still land close together */
  maxDelayMs: number;
}

// the gateway learns about a new key from its snapshot poll, every 5 s by
// default (`ROLTER_SNAPSHOT_POLL_SECS`). one interval is the worst case when
// the key lands just after a poll; the second is room for a poll that was slow
// to answer
const KEY_PROPAGATION: KeyPropagationTiming = {
  budgetMs: 10_000,
  firstDelayMs: 250,
  maxDelayMs: 2_000,
};

let keyPropagation: KeyPropagationTiming = KEY_PROPAGATION;

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
  if (!(error instanceof GatewayError) || error.status !== 401) return false;
  let waited = 0;
  for (let n = 0; n <= failures; n += 1) waited += keyPropagationDelay(n);
  return waited <= keyPropagation.budgetMs;
}

/**
 * Shorten (or restore, with `null`) the propagation backoff, so a story can
 * reach the "never accepted" fallback without waiting out ten real seconds.
 */
export function setKeyPropagationForTests(timing: Partial<KeyPropagationTiming> | null): void {
  keyPropagation = timing ? { ...KEY_PROPAGATION, ...timing } : KEY_PROPAGATION;
}

/** One addressable id from the gateway's own catalogue. */
export interface GatewayModel {
  id: string;
  /** the route, provider or provider group this id belongs to */
  owned_by?: string;
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
  const res = await fetch(`${GW_BASE}/v1/models`, {
    headers: authHeaders(false),
    signal,
  });
  if (!res.ok) throw await gwError(res);
  const body = (await res.json()) as { data?: GatewayModel[] };
  return body.data ?? [];
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
}

export async function embed(
  model: string,
  input: string[],
  signal?: AbortSignal,
): Promise<number[][]> {
  const res = await fetch(`${GW_BASE}/v1/embeddings`, {
    method: "POST",
    headers: authHeaders(),
    body: JSON.stringify({ model, input }),
    signal,
  });
  if (!res.ok) throw await gwError(res);
  const body = (await res.json()) as { data?: { embedding: number[] }[] };
  return (body.data ?? []).map((d) => d.embedding);
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
}

// text → speech: returns an object URL for an <audio> element
export async function synthesizeSpeech(
  model: string,
  input: string,
  voice: string,
  signal?: AbortSignal,
): Promise<string> {
  const res = await fetch(`${GW_BASE}/v1/audio/speech`, {
    method: "POST",
    headers: authHeaders(),
    body: JSON.stringify({ model, input, voice }),
    signal,
  });
  if (!res.ok) throw await gwError(res);
  const blob = await res.blob();
  return URL.createObjectURL(blob);
}

// speech → text: multipart upload of an audio file
export async function transcribe(model: string, file: File, signal?: AbortSignal): Promise<string> {
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
}

// realtime WebSocket URL (same-origin via the /gw proxy, ws upgrade enabled).
// the virtual key rides as a query param since browsers can't set WS headers.
export function realtimeUrl(model: string): string {
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const key = getPlaygroundKey();
  const params = new URLSearchParams({ model });
  if (key) params.set("api_key", key);
  return `${proto}//${location.host}${GW_BASE}/v1/realtime?${params.toString()}`;
}

/**
 * The gateway's OpenAI-compatible base URL, as a client outside the browser
 * would have to write it (#1585).
 *
 * The dashboard itself talks to `/gw` relative to its own origin, which is
 * useless in a snippet somebody pastes into a terminal — so this resolves it
 * against the current origin. A deployment that serves the gateway on its own
 * host still has to say so; this is the address the dashboard can prove works,
 * not a guess at the operator's ingress.
 */
export function gatewayBaseUrl(): string {
  return new URL(GW_BASE, location.origin).toString().replace(/\/$/, "");
}
