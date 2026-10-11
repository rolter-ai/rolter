import type { KvEventsConfig, LmCacheConfig } from "@/lib/api";

/**
 * The cache-telemetry sources of a provider as the sheet edits them (#2996).
 *
 * Every number is the text of its input, so a blank one can mean "the default"
 * and a half-typed one is not rewritten under the cursor. `kv`/`lm` are the
 * switches: off means the provider has no such source.
 */
export interface TelemetryDraft {
  kv: boolean;
  kvEndpoint: string;
  kvTopic: string;
  kvMaxBlocks: string;
  kvStaleSecs: string;
  lm: boolean;
  lmEndpoint: string;
  lmRefreshSecs: string;
  lmStaleSecs: string;
}

export type TelemetryField = Exclude<keyof TelemetryDraft, "kv" | "lm">;

export function blankTelemetry(): TelemetryDraft {
  return {
    kv: false,
    kvEndpoint: "",
    kvTopic: "",
    kvMaxBlocks: "",
    kvStaleSecs: "",
    lm: false,
    lmEndpoint: "",
    lmRefreshSecs: "",
    lmStaleSecs: "",
  };
}

export function telemetryFrom(
  kv: KvEventsConfig | null | undefined,
  lm: LmCacheConfig | null | undefined,
): TelemetryDraft {
  return {
    kv: !!kv,
    kvEndpoint: kv?.endpoint ?? "",
    kvTopic: kv?.topic ?? "",
    kvMaxBlocks: kv ? String(kv.max_blocks) : "",
    kvStaleSecs: kv ? String(kv.stale_secs) : "",
    lm: !!lm,
    lmEndpoint: lm?.endpoint ?? "",
    lmRefreshSecs: lm ? String(lm.refresh_secs) : "",
    lmStaleSecs: lm ? String(lm.stale_secs) : "",
  };
}

// a blank or unreadable number is left out, so the control plane applies its
// default; a readable one that is out of range is sent and refused by name
function count(text: string): number | undefined {
  const trimmed = text.trim();
  if (!trimmed) return undefined;
  const n = Number(trimmed);
  return Number.isFinite(n) ? n : undefined;
}

export function kvPayload(d: TelemetryDraft): Partial<KvEventsConfig> & { endpoint: string } {
  return {
    endpoint: d.kvEndpoint.trim(),
    ...(d.kvTopic.trim() ? { topic: d.kvTopic.trim() } : {}),
    ...(count(d.kvMaxBlocks) !== undefined ? { max_blocks: count(d.kvMaxBlocks) } : {}),
    ...(count(d.kvStaleSecs) !== undefined ? { stale_secs: count(d.kvStaleSecs) } : {}),
  };
}

export function lmPayload(d: TelemetryDraft): Partial<LmCacheConfig> & { endpoint: string } {
  return {
    endpoint: d.lmEndpoint.trim(),
    ...(count(d.lmRefreshSecs) !== undefined ? { refresh_secs: count(d.lmRefreshSecs) } : {}),
    ...(count(d.lmStaleSecs) !== undefined ? { stale_secs: count(d.lmStaleSecs) } : {}),
  };
}

const KV_KEYS: Record<string, TelemetryField> = {
  endpoint: "kvEndpoint",
  topic: "kvTopic",
  max_blocks: "kvMaxBlocks",
  stale_secs: "kvStaleSecs",
};
const LM_KEYS: Record<string, TelemetryField> = {
  endpoint: "lmEndpoint",
  refresh_secs: "lmRefreshSecs",
  stale_secs: "lmStaleSecs",
};

/**
 * The field a refusal is about, from the message the control plane sent.
 *
 * A 400 names its field as `kv_events.endpoint` or `lmcache.endpoint`, and a
 * range refusal lists the keys instead (`kv_events max_blocks and stale_secs
 * must be greater than zero`), where the first one listed is flagged. Anything
 * that names neither source is not about these fields and returns `null`.
 */
export function telemetryFieldOf(message: string): TelemetryField | null {
  const source = /\b(kv_events|lmcache)\b/.exec(message)?.[1];
  if (!source) return null;
  const keys = source === "kv_events" ? KV_KEYS : LM_KEYS;
  const dotted = new RegExp(`\\b${source}\\.(\\w+)`).exec(message)?.[1];
  if (dotted && keys[dotted]) return keys[dotted];
  const listed = Object.keys(keys)
    .map((k) => ({ k, at: message.search(new RegExp(`\\b${k}\\b`)) }))
    .filter((x) => x.at >= 0)
    .sort((a, b) => a.at - b.at)[0];
  return keys[listed?.k ?? "endpoint"];
}
