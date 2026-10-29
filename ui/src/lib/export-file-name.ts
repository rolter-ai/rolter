import type { WindowBounds } from "@/lib/time-window";

/**
 * The name the control plane gives an analytics export,
 * `<what>_<from>_to_<to>_<CURRENCY>.csv`, built in the dashboard from the same
 * parts (#3498): the date of each bound as it is sent, `7-days-ago` and `now`
 * for a bound left out, and the deployment's base currency, the one the file's
 * figures are in.
 *
 * A browser that streams an export to a file asks where to save before the
 * request leaves, since the dialog needs the click, so it cannot wait for the
 * server's `Content-Disposition`. Suggesting this name there means both ways
 * of saving offer one name. It mirrors `file_name` and `window_label` in
 * `crates/rolter-control/src/analytics_export.rs` character for character,
 * including what they drop.
 */
export function exportFileName(what: string, bounds: Partial<WindowBounds>, currency: string) {
  const code = [...currency]
    .filter((c) => /[A-Za-z0-9]/.test(c))
    .join("")
    .toUpperCase();
  return `${what}_${windowLabel(bounds.since, "7-days-ago")}_to_${windowLabel(bounds.until, "now")}_${code}.csv`;
}

/** a bound's date, its first ten characters kept to digits and `-`, or `unset` */
function windowLabel(bound: string | undefined, unset: string): string {
  const trimmed = bound?.trim();
  if (!trimmed) return unset;
  const date = [...trimmed]
    .slice(0, 10)
    .filter((c) => /[0-9-]/.test(c))
    .join("");
  return date || unset;
}
