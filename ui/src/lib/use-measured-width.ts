import * as React from "react";

/** The width drawn before the frame has been measured, and wherever it cannot be (no layout, no ResizeObserver). */
export const FALLBACK_WIDTH = 640;

/**
 * The pixel width of the element `ref` is attached to, kept current by a
 * `ResizeObserver`. The SVG charts draw at this width, so a unit of their
 * viewBox is a pixel at every width and an 11px label stays 11px (#1994, #3450).
 *
 * `mounted` is whatever decides whether the frame is in the document; the
 * observer is attached again when it changes. Before the first measurement, and
 * wherever layout is unavailable, the width is `FALLBACK_WIDTH`.
 */
export function useMeasuredWidth<T extends HTMLElement>(mounted: boolean = true) {
  const ref = React.useRef<T>(null);
  const [measured, setMeasured] = React.useState(0);
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const read = () => setMeasured(el.clientWidth);
    read();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(read);
    observer.observe(el);
    return () => observer.disconnect();
  }, [mounted]);
  return { ref, width: measured > 0 ? measured : FALLBACK_WIDTH };
}
