import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, within } from "storybook/test";

import Dashboard from "./Dashboard";
import { Harness, json, pending, routes, scoped, type FetchStub } from "./story-harness";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import { atMobile, atTablet, expectNoHorizontalOverflow } from "@/lib/story-viewport";

const fmt = formattersFor("en");

const SUMMARY = {
  requests: 132,
  tokens: 1_284_000,
  prompt_tokens: 900_000,
  completion_tokens: 384_000,
  cost_usd: 41.27,
  unpriced_requests: 0,
  unpriced_models: 0,
  errors: 7,
  p50_latency_ms: 210,
  p95_latency_ms: 980,
};

const SERIES = Array.from({ length: 6 }, (_, i) => ({
  bucket: `2026-10-05T0${i}:00:00Z`,
  requests: 10 + i * 3,
  tokens: 4000 + i * 500,
  cost_usd: 1.5 + i,
}));

const BY_MODEL = [
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

const RECENT = [
  {
    ts: "2026-10-05T12:34:56.789Z",
    request_id: "req-1",
    trace_id: "trace-1",
    org_id: "org-1",
    team_id: "team-1",
    project_id: "project-1",
    virtual_key_id: "vk-1",
    model: "gpt-4o",
    provider: "openai",
    target: "openai/gpt-4o",
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
  },
];

const loaded: FetchStub = routes([
  ["/api/v1/analytics/summary", () => ({ data: [SUMMARY] })],
  ["/api/v1/analytics/timeseries", () => ({ data: SERIES })],
  ["/api/v1/analytics/by-model", () => ({ data: BY_MODEL })],
  ["/api/v1/analytics/invocations", () => ({ data: RECENT })],
  ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
]);

const render = (stub: FetchStub) => (
  <Harness fetchStub={stub}>
    <Dashboard />
  </Harness>
);

const meta = {
  title: "Screens/Dashboard",
  component: Dashboard,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Dashboard>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the count is both a stat card and the donut centre
    await expect(await canvas.findAllByText(fmt.number(132))).not.toHaveLength(0);
  },
};

export const Loading: Story = { render: () => render(pending) };

// a deployment that has served nothing yet. the summary is an aggregate with no
// `group by`, so the control plane always answers one row — of zeroes. this stub
// used to answer `data: []`, which is not a quiet day but a query failure, and
// the story sat on the "cannot reach the control plane" screen with no `play`
// to notice
const QUIET = Object.fromEntries(Object.keys(SUMMARY).map((k) => [k, 0]));

export const Empty: Story = {
  render: () =>
    render(
      routes([
        ["/api/v1/analytics/summary", () => ({ data: [QUIET] })],
        ["/api/v1/analytics", () => ({ data: [] })],
        ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
      ]),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.dashboard.statRequests)).toBeVisible();
    // zero traffic is an answer, not a failure: no error panel, and no `NaN%`
    // from dividing the error count by no requests
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvasElement.textContent ?? "").not.toMatch(/NaN|undefined/);
  },
};

/**
 * The same quiet day, answered as an *empty* envelope rather than a row of
 * zeroes. `fetchAnalyticsSummary` used to resolve `r.data[0]`, which is
 * `undefined` here, and react-query v5 rejects a query function that resolves
 * to `undefined` — so this rendered "cannot reach the control plane" (#1608).
 *
 * The sibling of `Empty` rather than a replacement for it: the control plane
 * answers one row today, and this story is what keeps the other shape from
 * becoming an outage screen if a backend change or a proxy ever answers it.
 */
export const EmptySummaryEnvelope: Story = {
  render: () =>
    render(
      routes([
        ["/api/v1/analytics/summary", () => ({ data: [] })],
        ["/api/v1/analytics", () => ({ data: [] })],
        ["/api/v1/currency", () => ({ base: "USD", codes: ["USD"], rates: {} })],
      ]),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(en.pages.dashboard.statRequests)).toBeVisible();
    await expect(await canvas.findByText(fmt.currency(0, "USD"))).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
    await expect(canvasElement.textContent ?? "").not.toMatch(/NaN|undefined/);
  },
};

/**
 * #959 was measured here: at 375px the stat cards were cut mid-value — `132`
 * rendered as `13`, `5.30%` as `5.3` — because four columns were four columns
 * at every width. One card per row, and the value is whole again.
 */
export const Mobile: Story = {
  ...atMobile,
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expectNoHorizontalOverflow();
  },
};

export const Tablet: Story = {
  ...atTablet,
  render: () => render(loaded),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findAllByText(fmt.number(132));
    await expectNoHorizontalOverflow();
  },
};

/**
 * Analytics off is a load state, not an empty one: the `EmptyState` this used
 * to render said "nothing has happened yet" about a control plane that was
 * never asked to record anything (#1236).
 */
export const NoAnalyticsStore: Story = {
  render: () =>
    render(
      scoped(async (input) =>
        String(input).includes("/api/v1/analytics")
          ? json({ error: { message: "no clickhouse_url" } }, 503)
          : json({ base: "USD", codes: ["USD"], rates: {} }),
      ),
    ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      await canvas.findByText(/Analytics are not configured/i),
    ).toBeVisible();
    await expect(canvas.getByText(/CLICKHOUSE_URL/)).toBeVisible();
    await expect(canvas.queryByRole("button", { name: /try again/i })).toBeNull();
  },
};
