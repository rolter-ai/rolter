// the colours of a traffic split's segments: a route card's target bars and
// the route sheet's variant preview, so the nth share is the same colour in both
export const SHARE_BARS = [1, 2, 3, 4, 5, 6].map((n) => `var(--chart-${n})`);

/** the colour of the `i`th share, wrapping past the sixth */
export function shareBarColor(i: number): string {
  return SHARE_BARS[i % SHARE_BARS.length];
}
