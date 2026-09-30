// one colour per model across the cards that draw the same read

// the shared categorical sequence (#1245), the same eight the donut and the
// scatter plot walk, so a model is the same colour wherever it is drawn
const SEQUENCE = Array.from({ length: 8 }, (_, i) => `var(--chart-${i + 1})`);

/**
 * The models ordered by requests, busiest first, the name breaking a tie.
 *
 * The control plane answers by cost, and a card that re-sorts for itself
 * (the bars) and one that keeps the order it was given (the donut) then colour
 * the nth row differently, so one model was two colours in neighbouring cards
 * (#1994). Every card that colours models takes the colour from this order.
 */
export function rankedByRequests<T extends { model: string; requests: number }>(models: T[]): T[] {
  return [...models].sort(
    (a, b) => b.requests - a.requests || (a.model < b.model ? -1 : a.model > b.model ? 1 : 0),
  );
}

/**
 * The colour of the model at `rank` in `rankedByRequests`. Past the eighth the
 * sequence is spent and the last colour repeats rather than wrapping to the
 * first, which would hand the busiest model's colour to one of the quietest.
 */
export function modelColor(rank: number): string {
  return SEQUENCE[Math.min(rank, SEQUENCE.length - 1)];
}
