import { getJson, sendJson } from "./request";

export interface ModelPriceRow {
  id: string;
  model: string;
  /// decimal, returned as text
  input_per_mtok: string;
  output_per_mtok: string;
  cached_input_per_mtok?: string | null;
  /**
   * The rate for tokens written to the provider's prompt cache (#2876); `null`
   * prices them at the input rate
   */
  cache_write_per_mtok: string | null;
  /**
   * The rate for the part of those writes that went to Anthropic's 1 hour
   * cache (#2891); `null` prices it at `cache_write_per_mtok`, and from there
   * at the input rate
   */
  cache_write_1h_per_mtok: string | null;
  /** the price of one generated image (#2937); `null` charges nothing for images */
  image_per_unit: string | null;
  /** the price of one minute of input audio, for transcriptions (#2937) */
  audio_input_per_minute: string | null;
  /** the price per million characters of text-to-speech input (#2937) */
  speech_per_mchar: string | null;
  /**
   * The price per million audio input tokens (#1882), for a realtime or audio
   * model that bills audio apart from text; `null` charges them at the input
   * rate. Optional because a control plane that predates it does not send it.
   */
  audio_input_per_mtok?: string | null;
  /** the price per million audio output tokens (#1882); `null` means the output rate */
  audio_output_per_mtok?: string | null;
  /** the price per million cached audio input tokens (#1882) */
  cached_audio_input_per_mtok?: string | null;
  currency: string;
  created_at: string;
}

export interface UpsertModelPriceInput {
  model: string;
  input_per_mtok: string;
  output_per_mtok: string;
  cached_input_per_mtok?: string;
  /**
   * Unlike the rates beside it, this one is not replaced by a save that leaves
   * it out: absent keeps the stored rate, `null` clears it back to the input
   * rate and a string sets it (#2876). Build it with `cacheWritePatch()`.
   */
  cache_write_per_mtok?: string | null;
  /**
   * Kept, cleared and set the same way as `cache_write_per_mtok`; `null` falls
   * back to that rate (#2902). Build it with `cacheWrite1hPatch()`.
   */
  cache_write_1h_per_mtok?: string | null;
  /**
   * Kept, cleared and set the same way as `cache_write_per_mtok` (#2937).
   * Build them with `imagePatch()`, `audioInputPatch()` and `speechPatch()`.
   */
  image_per_unit?: string | null;
  audio_input_per_minute?: string | null;
  speech_per_mchar?: string | null;
  /**
   * Kept, cleared and set the same way (#1882). Build them with
   * `audioInputTokenPatch()`, `audioOutputTokenPatch()` and `cachedAudioPatch()`.
   */
  audio_input_per_mtok?: string | null;
  audio_output_per_mtok?: string | null;
  cached_audio_input_per_mtok?: string | null;
  currency?: string;
}

export function fetchModelPrices(): Promise<ModelPriceRow[]> {
  return getJson<ModelPriceRow[]>("/api/v1/model-prices");
}

/**
 * The deployment's settlement currency and the codes it can price in.
 *
 * `codes` is the base currency plus every code in the operator's rate table —
 * exactly the set the control plane accepts on a price write. The dashboard
 * used to hardcode seven ISO-4217 codes, which made a configured RUB
 * unselectable and offered a JPY the API would reject (#965).
 */
export interface CurrencySettings {
  base: string;
  codes: string[];
  /** units of `base` per unit of the keyed currency; base is implicitly 1.0 */
  rates: Record<string, number>;
}

export function fetchCurrencySettings(): Promise<CurrencySettings> {
  return getJson<CurrencySettings>("/api/v1/currency");
}

/**
 * Whether `code` can be converted into the settlement currency.
 *
 * A price already stored in a code the rate table no longer carries must still
 * display — dropping it would hide real pricing — but it cannot be converted,
 * so spend that includes it is understated. Callers surface that rather than
 * silently treating the number as base currency.
 */
export function isConvertible(settings: CurrencySettings | undefined, code: string): boolean {
  if (!settings) return true;
  const normalized = code.trim().toUpperCase();
  return settings.codes.some((c) => c.toUpperCase() === normalized);
}

export function upsertModelPrice(input: UpsertModelPriceInput): Promise<ModelPriceRow> {
  return sendJson<ModelPriceRow>("PUT", "/api/v1/model-prices", input);
}

export function deleteModelPrice(model: string): Promise<void> {
  return sendJson<void>("DELETE", `/api/v1/model-prices/${encodeURIComponent(model)}`);
}

// --- local-account auth (crates/rolter-control/src/auth.rs, ROL-32) ---
