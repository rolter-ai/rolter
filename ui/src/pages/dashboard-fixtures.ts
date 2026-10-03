// One day of traffic the Dashboard's stories and the assembled shell's stub
// both answer with, so the two reference renders cannot drift apart.
//
// The figures add up: the by-model rows sum to the summary's requests, errors,
// tokens and cost, and the hourly series sums to the same requests, tokens and
// cost. A render of the screen therefore never shows a 132-request tile over a
// chart that holds 105, or over one that holds nothing (#1994).
//
// Not a `.stories.tsx` file: it is a fixture, like `story-harness.tsx`.

export const SUMMARY = {
  requests: 132,
  tokens: 1_284_000,
  prompt_tokens: 900_000,
  completion_tokens: 384_000,
  cost_usd: 41.27,
  unpriced_requests: 0,
  unpriced_models: 0,
  errors: 7,
  avg_latency_ms: 214.6,
  p50_latency_ms: 210,
  p95_latency_ms: 980,
};

const HOURLY_REQUESTS = [14, 18, 21, 24, 26, 29];
const HOURLY_TOKENS = [150_000, 190_000, 210_000, 235_000, 240_000, 259_000];
const HOURLY_COST = [4.27, 5.9, 6.9, 7.3, 8.1, 8.8];

export const SERIES = HOURLY_REQUESTS.map((requests, i) => ({
  bucket: `2026-10-05T0${i}:00:00Z`,
  requests,
  tokens: HOURLY_TOKENS[i],
  cost_usd: HOURLY_COST[i],
}));

// in cost order, which is how the control plane answers: the model with the
// most requests is not the one that cost most in every deployment, and the
// cards that colour by model must not depend on it
export const BY_MODEL = [
  {
    model: "gpt-4o",
    requests: 84,
    tokens: 800_000,
    cost_usd: 30.1,
    unpriced_requests: 0,
    errors: 4,
    p50_latency_ms: 190,
    p95_latency_ms: 820,
  },
  {
    model: "claude-sonnet-4",
    requests: 48,
    tokens: 484_000,
    cost_usd: 11.17,
    unpriced_requests: 0,
    errors: 3,
    p50_latency_ms: 240,
    p95_latency_ms: 1100,
  },
];

/** one row of the recent-requests log, for `model`, `minutesAgo` before now */
export function recentRow(
  requestId: string,
  model: string,
  minutesAgo: number,
  overrides: Record<string, unknown> = {},
) {
  return {
    ts: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
    request_id: requestId,
    trace_id: `trace-${requestId}`,
    org_id: "org-1",
    team_id: "team-1",
    project_id: "project-1",
    virtual_key_id: "vk-1",
    model,
    provider: "openai",
    target: `openai/${model}`,
    variant: "",
    status: 200,
    stream: 0,
    cache_hit: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    prompt_tokens: 8000,
    completion_tokens: 4345,
    total_tokens: 12345,
    cost_usd: 0.0123,
    latency_ms: 842,
    ttft_ms: 120,
    error: "",
    ...overrides,
  };
}

// now, so the row is today's whatever day the story runs on and its time is
// shown without a date
export const RECENT = [recentRow("req-1", "gpt-4o", 0)];
