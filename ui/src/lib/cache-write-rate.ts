/**
 * What a price save says about the cache-write rate (#2876).
 *
 * The control plane keeps the stored rate when `PUT /api/v1/model-prices` leaves
 * `cache_write_per_mtok` out, clears it on `null` and sets it on a string. The
 * other rates are replaced by every save, so this one is told apart from them:
 * a form that opened on a price and never touched the input sends nothing, and
 * cannot reset a rate set from the other screen, a config import or the API in
 * the meantime.
 *
 * `typed` is the input as it stands and `seeded` the value it opened with ("" for
 * a price with no rate, or no price yet). The result spreads into the body.
 */
export function cacheWritePatch(
  typed: string,
  seeded: string,
): { cache_write_per_mtok?: string | null } {
  const next = typed.trim();
  if (next === seeded.trim()) return {};
  return { cache_write_per_mtok: next === "" ? null : next };
}
