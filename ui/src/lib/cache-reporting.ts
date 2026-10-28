import type { ProviderCacheVerdict } from "@/lib/api";

/**
 * What the dashboard says about whether providers report prompt-cache metrics.
 *
 * Three states, kept apart on purpose: a provider with no stored verdict has
 * never been probed, which tells nothing about its cache reporting, so it must
 * never read as "does not report".
 */
export type CacheReporting =
  | {
      status: "reports" | "none";
      /** the oldest verdict the claim rests on, so staleness is not hidden by a fresh sibling */
      probedAt: string;
      /** the verdict's model and field spelling, when exactly one provider is meant */
      model?: string;
      spelling?: string | null;
      /** how many of `total` providers the claim covers */
      count: number;
      total: number;
    }
  | { status: "unprobed"; count: number; total: number };

function oldest(verdicts: ProviderCacheVerdict[]): ProviderCacheVerdict {
  return verdicts.reduce((a, b) => (Date.parse(b.probed_at) < Date.parse(a.probed_at) ? b : a));
}

/**
 * The verdict over one or more providers (a route's targets, a group's
 * members). `byProvider` is every stored verdict of the org, keyed by provider
 * id; an id with no entry, or `undefined` (a target whose provider is not in
 * the org's list), is a provider never probed.
 *
 * The rule, the same on Models and Provider Groups:
 *
 * - "does not report" as soon as any provider is known not to, since that
 *   provider's traffic carries no cache usage;
 * - "reports" only when every provider has a verdict and all of them report;
 * - otherwise "not probed yet" (some provider has no verdict and none is known
 *   not to report).
 *
 * `null` means there is nothing to judge: no providers at all, or `byProvider`
 * is `undefined` because the verdicts have not been read.
 */
export function summarizeCacheReporting(
  providerIds: ReadonlyArray<string | undefined>,
  byProvider: ReadonlyMap<string, ProviderCacheVerdict> | undefined,
): CacheReporting | null {
  // the verdicts could not be read (still loading, or the read failed): claim
  // nothing, in particular not "not probed yet"
  if (!byProvider) return null;
  const unique = [...new Set(providerIds)];
  if (unique.length === 0) return null;
  const known = unique.flatMap((id) => {
    const verdict = id === undefined ? undefined : byProvider.get(id);
    return verdict ? [verdict] : [];
  });
  const silent = known.filter((v) => !v.reports);
  const total = unique.length;
  const single = (v: ProviderCacheVerdict) =>
    total === 1 ? { model: v.model, spelling: v.spelling } : {};
  if (silent.length > 0) {
    const at = oldest(silent);
    return {
      status: "none",
      probedAt: at.probed_at,
      count: silent.length,
      total,
      ...single(at),
    };
  }
  if (known.length === total) {
    const at = oldest(known);
    return { status: "reports", probedAt: at.probed_at, count: total, total, ...single(at) };
  }
  return { status: "unprobed", count: known.length, total };
}
