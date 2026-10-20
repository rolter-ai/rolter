/**
 * What a price save says about the cache-write rates (#2876, #2902).
 *
 * The control plane keeps the stored rate when `PUT /api/v1/model-prices` leaves
 * `cache_write_per_mtok` or `cache_write_1h_per_mtok` out, clears it on `null`
 * and sets it on a string. The other rates are replaced by every save, so these
 * two are told apart from them: a form that opened on a price and never touched
 * an input sends nothing for it, and cannot reset a rate set from the other
 * screen, a config import or the API in the meantime.
 *
 * `typed` is the input as it stands and `seeded` the value it opened with ("" for
 * a price with no rate, or no price yet). The result spreads into the body.
 */
function ratePatch<K extends string>(
  key: K,
  typed: string,
  seeded: string,
): { [P in K]?: string | null } {
  const next = typed.trim();
  if (next === seeded.trim()) return {};
  return { [key]: next === "" ? null : next } as { [P in K]?: string | null };
}

/** the rate for tokens written to the 5 minute cache, and to any cache that reports no lifetime */
export function cacheWritePatch(
  typed: string,
  seeded: string,
): { cache_write_per_mtok?: string | null } {
  return ratePatch("cache_write_per_mtok", typed, seeded);
}

/**
 * The rate for tokens written to Anthropic's 1 hour cache. Cleared, it falls
 * back to the 5 minute rate, and from there to the input rate.
 */
export function cacheWrite1hPatch(
  typed: string,
  seeded: string,
): { cache_write_1h_per_mtok?: string | null } {
  return ratePatch("cache_write_1h_per_mtok", typed, seeded);
}

/**
 * The rates for requests billed by something other than tokens (#2937). The
 * control plane keeps each one when a save leaves it out, clears it on `null`
 * and sets it on a string, exactly as it does the cache-write rates above.
 */
export function imagePatch(typed: string, seeded: string): { image_per_unit?: string | null } {
  return ratePatch("image_per_unit", typed, seeded);
}

export function audioInputPatch(
  typed: string,
  seeded: string,
): { audio_input_per_minute?: string | null } {
  return ratePatch("audio_input_per_minute", typed, seeded);
}

export function speechPatch(typed: string, seeded: string): { speech_per_mchar?: string | null } {
  return ratePatch("speech_per_mchar", typed, seeded);
}

/**
 * The rates for the audio tokens of a realtime or audio model (#1882), kept,
 * cleared and set like the rest. Empty prices those tokens at the matching
 * text rate.
 */
export function audioInputTokenPatch(
  typed: string,
  seeded: string,
): { audio_input_per_mtok?: string | null } {
  return ratePatch("audio_input_per_mtok", typed, seeded);
}

export function audioOutputTokenPatch(
  typed: string,
  seeded: string,
): { audio_output_per_mtok?: string | null } {
  return ratePatch("audio_output_per_mtok", typed, seeded);
}

export function cachedAudioPatch(
  typed: string,
  seeded: string,
): { cached_audio_input_per_mtok?: string | null } {
  return ratePatch("cached_audio_input_per_mtok", typed, seeded);
}

/**
 * Every rate a price save keeps when it leaves it out, as a form holds them:
 * the input as it stands, or as it opened. Keyed by the form's own names, so a
 * draft's price object can be passed as it is.
 */
export interface KeptRates {
  cacheWrite: string;
  cacheWrite1h: string;
  image: string;
  audioInput: string;
  speech: string;
  audioInputTokens: string;
  audioOutputTokens: string;
  cachedAudio: string;
}

/**
 * What a save says about every kept rate: only those whose input changed are
 * named, so a rate left alone stays as stored. The result spreads into the body.
 */
export function keptRatePatches(typed: KeptRates, seeded: KeptRates) {
  return {
    ...cacheWritePatch(typed.cacheWrite, seeded.cacheWrite),
    ...cacheWrite1hPatch(typed.cacheWrite1h, seeded.cacheWrite1h),
    ...imagePatch(typed.image, seeded.image),
    ...audioInputPatch(typed.audioInput, seeded.audioInput),
    ...speechPatch(typed.speech, seeded.speech),
    ...audioInputTokenPatch(typed.audioInputTokens, seeded.audioInputTokens),
    ...audioOutputTokenPatch(typed.audioOutputTokens, seeded.audioOutputTokens),
    ...cachedAudioPatch(typed.cachedAudio, seeded.cachedAudio),
  };
}
