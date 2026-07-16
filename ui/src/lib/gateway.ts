// client for the rolter-gateway data plane (/v1/*), used by the Playground.
//
// the dashboard is served by the control plane, but chat/embeddings/image/audio
// calls hit the gateway on a different port. in dev, vite proxies /gw → :4000
// (see vite.config.ts); in prod the control plane must reverse-proxy /gw/*.
// the gateway authenticates with a virtual key, which the user sets in the
// Playground (persisted to localStorage, never sent to the control plane).

const GW_BASE = "/gw";
const KEY_STORAGE = "rolter.playground.key";

export function getPlaygroundKey(): string {
  try {
    return localStorage.getItem(KEY_STORAGE) ?? "";
  } catch {
    return "";
  }
}

export function setPlaygroundKey(key: string): void {
  try {
    if (key) localStorage.setItem(KEY_STORAGE, key);
    else localStorage.removeItem(KEY_STORAGE);
  } catch {
    // localStorage unavailable — key just won't persist across reloads
  }
}

function authHeaders(json = true): Record<string, string> {
  const headers: Record<string, string> = {};
  const key = getPlaygroundKey();
  if (key) headers.Authorization = `Bearer ${key}`;
  if (json) headers["Content-Type"] = "application/json";
  return headers;
}

// surface the gateway's OpenAI-style `{"error":{"message":...}}` body, else status
async function gwError(res: Response): Promise<Error> {
  try {
    const body = (await res.json()) as { error?: { message?: string } };
    if (body?.error?.message) return new Error(body.error.message);
  } catch {
    // not json
  }
  if (res.status === 401)
    return new Error("unauthorized — set a valid virtual key");
  return new Error(`gateway request failed: ${res.status}`);
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  // string, or OpenAI multimodal content parts (text + image_url)
  content:
    | string
    | Array<
        | { type: "text"; text: string }
        | { type: "image_url"; image_url: { url: string } }
      >;
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
export async function transcribe(
  model: string,
  file: File,
  signal?: AbortSignal,
): Promise<string> {
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
