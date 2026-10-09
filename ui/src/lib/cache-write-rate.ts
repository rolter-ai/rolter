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
