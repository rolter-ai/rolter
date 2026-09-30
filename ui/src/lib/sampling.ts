// the request-log sample rate read the way an operator reasons about it:
// "about 1 in 4 requests" rather than 0.25 (#2088), and a percentage typed into
// a form read back as the rate the control plane stores (#2104)

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

/** What a sampling percentage typed into a form comes to. */
export type SamplingInput =
  | { ok: true; rate: number }
  /**
   * `invalid` is anything that is not a number, blank included; `range` is a
   * number outside 0 to 100. The form words them differently: one asks for a
   * number, the other says what the bounds are.
   */
  | { ok: false; problem: "invalid" | "range" };

// a decimal the way a number input reports one: a sign, digits with an optional
// point, an optional exponent. `Number()` alone also reads "0x10" as 16 and ""
// as 0, and a form must not turn either into a rate
const DECIMAL = /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i;

/**
 * A sampling percentage (0 to 100) read as the 0 to 1 rate the control plane
 * takes.
 *
 * Zero is a rate: it sends nothing, which is a legitimate way to park a
 * connector. Nothing is coerced, so a blank field, text and 150 are refused
 * instead of becoming 100 %, the rate that ships every request and the worst
 * direction for a value to fail in.
 */
export function parseSamplingPercent(text: string): SamplingInput {
  const typed = text.trim();
  if (!DECIMAL.test(typed)) return { ok: false, problem: "invalid" };
  const percent = Number(typed);
  if (!Number.isFinite(percent)) return { ok: false, problem: "invalid" };
  if (percent < 0 || percent > 100) return { ok: false, problem: "range" };
  // `-0` would serialise as 0 anyway, but a rate that compares unequal to 0
  // is a surprise waiting in whatever reads it next
  return { ok: true, rate: percent === 0 ? 0 : percent / 100 };
}
