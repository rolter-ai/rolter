import * as React from "react";

// the two viewport widths the shell changes shape at (#959, #1203). they are
// tailwind's `md` and `lg`, quoted here because the change is not purely a
// class swap: the rail becomes a modal drawer and the detail panels become
// sheets, and both of those are decisions javascript has to make.
//
// `.98` rather than the breakpoint itself so a viewport of exactly 768px is
// `md` for both the media query and the tailwind class, not neither.
export const BELOW_MD = "(max-width: 767.98px)";
export const BELOW_LG = "(max-width: 1023.98px)";

/**
 * Whether `query` currently matches.
 *
 * Starts `false` and settles after mount rather than reading `matchMedia`
 * in the initializer: the first paint is then the same everywhere, including
 * in a test renderer with no `matchMedia` at all.
 */
export function useMediaQuery(query: string): boolean {
  const subscribe = React.useCallback(
    (onChange: () => void) => {
      if (typeof window === "undefined" || !window.matchMedia) return () => {};
      const mql = window.matchMedia(query);
      mql.addEventListener("change", onChange);
      return () => mql.removeEventListener("change", onChange);
    },
    [query],
  );
  const get = React.useCallback(() => {
    if (typeof window === "undefined" || !window.matchMedia) return false;
    return window.matchMedia(query).matches;
  }, [query]);
  return React.useSyncExternalStore(subscribe, get, () => false);
}
