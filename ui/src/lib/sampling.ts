// the request-log sample rate read the way an operator reasons about it:
// "about 1 in 4 requests" rather than 0.25 (#2088)

/** A share of requests, read as `numerator in denominator`. */
export interface SampleShare {
  numerator: number;
  denominator: number;
}

/**
 * The closest "n in d" reading of a sample rate strictly between 0 and 1.
 *
 * Below 10 % it is one in however many (0.1 % is 1 in 1000). Above 90 % it is
 * all but one in however many (95 % is 19 in 20). In between it is the nearest
 * fraction with a denominator of ten or less, the smallest denominator winning a
 * tie, so 50 % reads 1 in 2 rather than 5 in 10. At 0, at 1 and outside that
 * range there is no share to state, and the answer is `null`.
 */
export function sampleShare(rate: number): SampleShare | null {
  if (!Number.isFinite(rate) || rate <= 0 || rate >= 1) return null;
  if (rate < 0.1) return { numerator: 1, denominator: Math.round(1 / rate) };
  if (rate > 0.9) {
    const denominator = Math.round(1 / (1 - rate));
    return { numerator: denominator - 1, denominator };
  }
  // d = 10 always yields a numerator in 1..9 for a rate in [0.1, 0.9], so the
  // loop never leaves this seed in place
  let best: SampleShare = { numerator: 1, denominator: 10 };
  let bestError = Infinity;
  for (let denominator = 2; denominator <= 10; denominator++) {
    const numerator = Math.round(rate * denominator);
    if (numerator < 1 || numerator >= denominator) continue;
    const error = Math.abs(numerator / denominator - rate);
    if (error < bestError - 1e-9) {
      best = { numerator, denominator };
      bestError = error;
    }
  }
  return best;
}
