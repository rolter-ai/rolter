import type { TFunction } from "i18next";

/**
 * The dispositions a gateway records for a request parameter it did not
 * forward as sent (#1586): the same names the `x-rolter-param-adjustments`
 * header and the request log carry.
 */
export const PARAM_DISPOSITIONS = ["dropped", "clamped", "translated"] as const;

export type ParamDisposition = (typeof PARAM_DISPOSITIONS)[number];

/**
 * One `<disposition>:<param>` entry of a log row's `param_adjustments`, split.
 * The disposition never holds a `:`, so the first one splits it; an entry with
 * none is read as a bare parameter name with no disposition.
 */
export function parseAdjustment(entry: string): { disposition: string; param: string } {
  const at = entry.indexOf(":");
  return at < 0
    ? { disposition: "", param: entry }
    : { disposition: entry.slice(0, at), param: entry.slice(at + 1) };
}

/**
 * A disposition in words. A name this build has no copy for is printed as the
 * control plane sent it, so one a newer gateway adds is still read.
 */
export function dispositionLabel(t: TFunction, disposition: string): string {
  const known = PARAM_DISPOSITIONS.find((name) => name === disposition);
  return known ? t(`analytics.paramAdjustments.${known}`) : disposition;
}
