import { useQuery } from "@tanstack/react-query";

import { fetchCurrencySettings } from "@/lib/api";

/**
 * The deployment's settlement currency code.
 *
 * Spend and budgets are denominated in `currency.base` (see `CurrencyConfig` in
 * rolter-core) — the `_usd` suffixes on `cost_usd` and `limit_usd` are historic
 * names, not a guarantee of dollars. Screens used to print a literal `$` in
 * front of those numbers, which mislabels every non-USD deployment (#1182).
 *
 * The query key is shared with the model sheet's currency chooser, so the
 * settings are fetched once per session, and a failure falls back to USD rather
 * than rendering an amount with no unit at all.
 */
export function useCurrencyCode(): string {
  return useBaseCurrency() || "USD";
}

/**
 * The settlement currency as the control plane states it, or `undefined`
 * while the settings are being read or could not be. For a value that must
 * match the server's own exactly, such as an export's file name (#3498), where
 * the USD fallback above would be a guess.
 */
export function useBaseCurrency(): string | undefined {
  const settings = useQuery({
    queryKey: ["currency-settings"],
    queryFn: fetchCurrencySettings,
    retry: false,
    staleTime: 5 * 60_000,
  });
  return settings.data?.base || undefined;
}
