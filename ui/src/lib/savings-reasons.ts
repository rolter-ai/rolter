/**
 * The reasons the savings advisor leaves a request out of the re-pricing under,
 * as the control plane names them (`EXCLUSION_REASONS` in
 * `crates/rolter-control/src/savings.rs`), in the order the screen lists them.
 *
 * Each is worded in every catalog as `pages.savings.reasons.<reason>`.
 * `ui/scripts/savings-reasons.test.ts` holds this list to the control plane's
 * published one, so a reason added there fails the build until it is listed
 * here and worded in every catalog (#3423).
 */
export const EXCLUDED_REASONS = [
  "unpriced",
  "failed",
  "cache_hit",
  "usage_unknown",
  "uses_tools",
  "image_input",
  "audio_input",
  "structured_output",
  "multi_turn",
  "not_a_model_call",
  "no_token_usage",
  "prompt_too_large",
  "output_too_large",
  "route_not_configured",
  "no_cheaper_candidate",
] as const;
