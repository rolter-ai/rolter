import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import Health from "./Health";
import {
  Harness,
  ORG,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
} from "./story-harness";
import type { BreakerStateDto, BreakerTargetDto, MttrRow, TimelineRow, UptimeRow } from "@/lib/api";
import { resolveColorToken } from "@/lib/story-tokens";
import { phoneFits } from "@/lib/story-viewport";

// two grains of the same provider. `openai-dead` is watched by probes (a
// provider-grain row) *and* by real traffic through two models; before #1257
// that arrived as three peer cards with contradictory counts.
const UPTIME: UptimeRow[] = [
  {
    provider: "openai-dead",
    target_id: "openai-dead",
    grain: "provider",
    sources: ["probe", "status_page"],
    events: 200,
    ok: 10,
    errors: 190,
    timeouts: 0,
    uptime: 0.05,
    failure_rate: 0.95,
    error_budget_burn: 95,
    sla_breached: 1,
    last_event: "2026-08-06T10:00:00Z",
  },
  {
    provider: "openai-dead",
    target_id: "gpt-4o",
    grain: "target",
    sources: ["passive"],
    events: 20,
    ok: 10,
    errors: 10,
    timeouts: 0,
    uptime: 0.5,
    failure_rate: 0.5,
    error_budget_burn: 50,
    sla_breached: 1,
    last_event: "2026-08-06T09:59:00Z",
  },
  {
    provider: "openai-dead",
    target_id: "gpt-4o-mini",
    grain: "target",
    sources: ["passive"],
    events: 40,
    ok: 39,
    errors: 1,
    timeouts: 0,
    uptime: 0.975,
    failure_rate: 0.025,
    error_budget_burn: 2.5,
    sla_breached: 1,
    last_event: "2026-08-06T09:57:00Z",
  },
  // a provider with no probes at all: only passive rows, so the card rolls
  // them up itself and says the headline is derived
  {
    provider: "anthropic",
    target_id: "sonnet@eu",
    grain: "target",
    sources: ["passive"],
    events: 980,
    ok: 902,
    errors: 71,
    timeouts: 7,
    uptime: 0.9204,
    failure_rate: 0.0796,
    error_budget_burn: 7.96,
    sla_breached: 1,
    last_event: "2026-08-06T09:58:00Z",
  },
];

const MTTR: MttrRow[] = [
  {
    provider: "openai-dead",
    target_id: "openai-dead",
    grain: "provider",
    mttr_seconds: 3600,
    incidents: 1,
  },
  { provider: "openai-dead", target_id: "gpt-4o", grain: "target", mttr_seconds: 42, incidents: 2 },
  {
    provider: "anthropic",
    target_id: "sonnet@eu",
    grain: "target",
    mttr_seconds: 913,
    incidents: 5,
  },
];

const TIMELINE: TimelineRow[] = Array.from({ length: 12 }, (_, i) => ({
  bucket: `2026-08-06T${String(i).padStart(2, "0")}:00:00Z`,
  provider: "openai-dead",
  target_id: "openai-dead",
  grain: "provider" as const,
  events: 300,
  ok: i === 7 ? 280 : 300,
  errors: i === 7 ? 20 : 0,
  timeouts: 0,
}));

const loaded = routes([
  ["/health/uptime", () => ({ data: UPTIME })],
  ["/health/mttr", () => ({ data: MTTR })],
  ["/health/timeline", () => ({ data: TIMELINE })],
]);

const meta = {
  title: "Screens/Health",
  component: Health,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof Health>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("sonnet@eu")).toBeVisible());
    // #2113: a breach is named as the SLA verdict it is, never as a breaker
    // state — nothing on this screen reads the gateway's breakers
    await expect(canvas.getAllByText("below SLA").length).toBeGreaterThan(0);
    await expect(canvas.queryByText(/tripped|closed/i)).toBeNull();

    // #1257: one card per provider, with its targets nested inside it — not
    // one card per (provider, target_id) pair
    await expect(canvas.getAllByTestId(/^health-card-/)).toHaveLength(2);
    const dead = canvas.getByTestId("health-card-openai-dead");
    await expect(within(dead).getByText("openai-dead")).toBeVisible();
    await expect(within(dead).getByText("gpt-4o")).toBeVisible();
    await expect(within(dead).getByText("gpt-4o-mini")).toBeVisible();
    await expect(within(dead).getByText("2 targets")).toBeVisible();
    // the headline is the provider-grain row, and it names what fed it
    await expect(within(dead).getByText("probed · probes, status page")).toBeVisible();
    await expect(within(dead).getByText("uptime · 200 events")).toBeVisible();

    // #2116: the strip names its range, its legend and its values as text
    const strip = within(dead).getAllByTestId("health-timeline")[0];
    await expect(strip).toHaveAccessibleName(
      /Health timeline, .* to .*: 20 failures in 3,600 events/,
    );
    await expect(within(dead).getByText("No failures")).toBeVisible();
    await expect(within(strip).getAllByRole("listitem")).toHaveLength(12);
    await expect(within(strip).getAllByText(/20 error/)).toHaveLength(1);
    // a start and an end label, formatted rather than the raw ISO string
    await expect(within(dead).queryByText(/T\d\d:00:00Z/)).toBeNull();

    // the failing bucket takes --status-danger, is taller than a clean one,
    // and the clean ones keep the success fill
    const bars = Array.from(strip.querySelectorAll<HTMLElement>(":scope > div[aria-hidden]"));
    const down = bars.filter((b) => b.dataset.down);
    await expect(down).toHaveLength(1);
    await expect(getComputedStyle(down[0]).backgroundColor).toBe(
      resolveColorToken("--status-danger"),
    );
    await expect(getComputedStyle(bars[0]).backgroundColor).not.toBe(
      resolveColorToken("--status-danger"),
    );
    await expect(parseFloat(down[0].style.height)).toBeGreaterThan(
      parseFloat(bars[0].style.height),
    );

    // #2122: a target's figures name themselves, in words rather than by position
    const gpt = within(dead).getByTestId("health-target-gpt-4o");
    await expect(within(gpt).getByText("MTTR 42s")).toBeVisible();
    await expect(within(gpt).getByText("uptime", { exact: false })).toBeInTheDocument();

    // a target with no timeline rows says so, never "no events" beside counts
    const mini = canvas.getByTestId("health-target-gpt-4o-mini");
    await expect(within(mini).getByText("No timeline.")).toBeVisible();
    await expect(canvas.queryByText(/No events in window/)).toBeNull();
  },
};

// one timeout among thousands must not look like an outage: height follows the share
export const FailureShare: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/health/uptime", () => ({ data: [uptimeRow("share", "share", 20000, 11, ["probe"])] })],
        ["/health/mttr", () => ({ data: [] })],
        [
          "/health/timeline",
          () => ({
            data: [...strip("share", "share", { 3: 1, 8: 5000 }, 10000).slice(0, 12)],
          }),
        ],
      ])}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const timeline = await waitFor(() => canvas.getByTestId("health-timeline"));
    const bars = Array.from(timeline.querySelectorAll<HTMLElement>(":scope > div[aria-hidden]"));
    const h = (i: number) => parseFloat(bars[i].style.height);
    await expect(h(3)).toBeLessThan(h(8));
    await expect(h(8)).toBe(100);
  },
};

// a provider observed only by traffic has no provider-grain row, so the card
// rolls its targets up itself and labels the headline as derived
export const DerivedHeadline: Story = {
  render: () => (
    <Harness fetchStub={loaded}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const card = await waitFor(() => canvas.getByTestId("health-card-anthropic"));
    await expect(within(card).getByText("rolled up from targets")).toBeVisible();
    await expect(within(card).getByText("uptime · 980 events")).toBeVisible();
    await expect(within(card).getByText("1 target")).toBeVisible();
  },
};

// #2113: three SLA states, one per card. the timeline arrives in ClickHouse's
// own wire format — a zone-less `2026-08-06 10:00:00` — and its newest two
// hours (10:00 and 11:00) are what "recently" means here
const hour = (h: number) => `2026-08-06 ${String(h).padStart(2, "0")}:00:00`;

function strip(
  provider: string,
  target: string,
  failures: Record<number, number>,
  events = 100,
): TimelineRow[] {
  return Array.from({ length: 12 }, (_, h) => ({
    bucket: hour(h),
    provider,
    target_id: target,
    grain: provider === target ? ("provider" as const) : ("target" as const),
    events,
    ok: events - (failures[h] ?? 0),
    errors: failures[h] ?? 0,
    timeouts: 0,
  }));
}

function uptimeRow(
  provider: string,
  target: string,
  events: number,
  failures: number,
  sources: string[],
): UptimeRow {
  const failureRate = failures / events;
  return {
    provider,
    target_id: target,
    grain: provider === target ? "provider" : "target",
    sources,
    events,
    ok: events - failures,
    errors: failures,
    timeouts: 0,
    uptime: 1 - failureRate,
    failure_rate: failureRate,
    error_budget_burn: failureRate / 0.01,
    sla_breached: failureRate > 0.01 ? 1 : 0,
    last_event: "2026-08-06 11:59:30.000",
  };
}

const SLA_UPTIME: UptimeRow[] = [
  // 60 failures in 1200 probes: over the 1% the SLA allows, even though the
  // last two hours were clean — the window's verdict stands
  uptimeRow("groq", "groq", 1200, 60, ["probe"]),
  // 6 failures in 1200 is inside the SLA, but all six landed in the last two
  // hours: 3% there, three times what the SLA allows
  uptimeRow("vllm-pool", "vllm-pool", 1200, 6, ["probe"]),
  // one bad hour early in the window, then clean: inside the SLA, not at risk
  uptimeRow("mistral", "mistral", 1200, 5, ["probe"]),
  // no probes, so the card is rolled up from its targets and takes the worst
  uptimeRow("anthropic", "sonnet@eu", 1200, 4, ["passive"]),
  uptimeRow("anthropic", "haiku@eu", 1200, 0, ["passive"]),
];

const SLA_TIMELINE: TimelineRow[] = [
  ...strip("groq", "groq", { 2: 30, 3: 30 }),
  ...strip("vllm-pool", "vllm-pool", { 10: 3, 11: 3 }),
  ...strip("mistral", "mistral", { 3: 5 }),
  ...strip("anthropic", "sonnet@eu", { 11: 4 }),
  ...strip("anthropic", "haiku@eu", {}),
];

export const SlaStates: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/health/uptime", () => ({ data: SLA_UPTIME })],
        ["/health/mttr", () => ({ data: [] })],
        ["/health/timeline", () => ({ data: SLA_TIMELINE })],
      ])}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByTestId(/^health-card-/)).toHaveLength(4));

    // each pill names the state and takes the text half of its status pair
    const expectPill = async (provider: string, label: string, token: `--${string}`) => {
      const pill = within(canvas.getByTestId(`health-card-${provider}`)).getByTestId(
        "health-sla-state",
      );
      await expect(pill).toHaveTextContent(label);
      await expect(getComputedStyle(pill).color).toBe(resolveColorToken(token));
    };
    await expectPill("groq", "below SLA", "--status-danger-text");
    await expectPill("vllm-pool", "SLA at risk", "--status-warning-text");
    await expectPill("mistral", "within SLA", "--status-success-text");
    await expectPill("anthropic", "SLA at risk", "--status-warning-text");

    // an at-risk provider still holds the SLA over the window, so its uptime
    // figure is not painted as a breach
    const atRisk = canvas.getByTestId("health-card-vllm-pool");
    await expect(getComputedStyle(within(atRisk).getByText("99.50%")).color).not.toBe(
      resolveColorToken("--status-danger-text"),
    );

    // a target row's dot carries its state as text too
    const sonnet = canvas.getByTestId("health-target-sonnet@eu");
    await expect(within(sonnet).getByText("SLA at risk")).toBeInTheDocument();
    const haiku = canvas.getByTestId("health-target-haiku@eu");
    await expect(within(haiku).getByText("within SLA")).toBeInTheDocument();

    // the rollups answered with no incident, which is zero and not unknown
    const mistral = within(canvas.getByTestId("health-card-mistral"));
    await expect(mistral.getByText("Incidents").nextElementSibling).toHaveTextContent("0");

    // a breach is not painted on every element of the card: the pill and the
    // border carry it, so the headline figure stays neutral (#2122)
    const groq = canvas.getByTestId("health-card-groq");
    await expect(getComputedStyle(within(groq).getByText("95.00%")).color).not.toBe(
      resolveColorToken("--status-danger-text"),
    );

    // the line above the grid says what "at risk" means
    await expect(canvas.getByText(/SLA at risk means/)).toBeVisible();
  },
};

// a 7-day uptime says nothing about whether a provider is still reporting:
// the card and each target say when the last event arrived, a stale one takes
// the warning tone and different words, and a provider that never reported
// shows no uptime at all rather than a perfect one (#2118)
const agoIso = (ms: number) => new Date(Date.now() - ms).toISOString();
const STALE_UPTIME: UptimeRow[] = [
  { ...uptimeRow("fresh", "fresh", 100, 0, ["probe"]), last_event: agoIso(2 * 60 * 1000) },
  { ...uptimeRow("gone", "gone", 100, 0, ["probe"]), last_event: agoIso(3 * 60 * 60 * 1000) },
  {
    ...uptimeRow("silent", "gpt-4o", 0, 0, ["passive"]),
    grain: "target",
    uptime: 1,
    failure_rate: 0,
    error_budget_burn: 0,
    last_event: "",
  },
];

export const StaleProvider: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/health/uptime", () => ({ data: STALE_UPTIME })],
        ["/health/mttr", () => ({ data: [] })],
        ["/health/timeline", () => ({ data: [] })],
      ])}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByTestId(/^health-card-/)).toHaveLength(3));
    // the page says when it last asked, and the header refresh re-reads it
    await expect(canvas.getByTestId("health-updated")).toHaveTextContent(/^Updated /);

    const fresh = within(canvas.getByTestId("health-card-fresh"));
    await expect(fresh.getByTestId("health-last-event")).toHaveTextContent(/^Last event 2 min/);
    await expect(fresh.getByTestId("health-last-event")).not.toHaveAttribute("data-stale");

    const gone = within(canvas.getByTestId("health-card-gone"));
    await expect(gone.getByTestId("health-last-event")).toHaveTextContent(/^Stale: last event 3 h/);
    await expect(gone.getByTestId("health-last-event")).toHaveAttribute("data-stale", "true");
    await expect(getComputedStyle(gone.getByTestId("health-last-event")).color).toBe(
      resolveColorToken("--status-warning-text"),
    );

    // zero events: no uptime figure, and no claim about a last event
    const silent = within(canvas.getByTestId("health-card-silent"));
    await expect(silent.queryByText("100.00%")).toBeNull();
    await expect(silent.getAllByText("—").length).toBeGreaterThan(0);
    await expect(silent.getAllByText("No events yet").length).toBeGreaterThan(0);
  },
};

// the timeline read fails while uptime succeeds: the grid still renders under
// the error, and must not claim "no events" beside non-zero counts
export const TimelineFailed: Story = {
  render: () => (
    <Harness
      fetchStub={routes([
        ["/health/uptime", () => ({ data: UPTIME })],
        ["/health/mttr", () => ({ data: MTTR })],
        ["/health/timeline", () => json({ error: { message: "boom" } }, 500)],
      ])}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByTestId("health-card-openai-dead")).toBeVisible());
    await expect(canvas.queryByText(/No events in window/)).toBeNull();
    await expect(canvas.getAllByText("No timeline.").length).toBeGreaterThan(0);
  },
};

// three queries, none of them settled: the card grid stands in for itself so
// the layout does not jump when the rollups land
export const Loading: Story = {
  render: () => (
    <Harness fetchStub={pending}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a deployment that has served no traffic has no rollups to compute — that is
// not a failure, and the CTA points at the one thing that produces an event
export const Empty: Story = {
  render: () => (
    <Harness fetchStub={routes([["/health/", () => ({ data: [] })]])}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectEmptyState(canvasElement, /No health events recorded yet/);
  },
};

export const Error_: Story = {
  name: "Error",
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "boom" } }, 500))}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /failed to return health rollups/i);
  },
};

// a read past the request timeout is a busy store, not a dead one (#2713)
export const QueryTimeout: Story = {
  render: () => (
    <Harness
      fetchStub={scoped(async () =>
        json(
          { error: { message: "analytics query timed out", code: "analytics_query_timeout" } },
          504,
        ),
      )}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /health rollups took too long/i);
    const canvas = within(canvasElement);
    await expect(canvas.queryByText(/failed to return/i)).toBeNull();
    await expect(canvas.getAllByRole("button", { name: /try again/i }).length).toBeGreaterThan(0);
  },
};

// retrying a 403 cannot work, so LoadError withholds the button and says who can
export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={scoped(async () => json({ error: { message: "forbidden" } }, 403))}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectLoadError(canvasElement, /You do not have access to health rollups/);
  },
};

// the same screen at a phone's width in both languages: Russian runs a third
// longer than English and overflowed twice as many screens (#2004)
const healthFits = phoneFits({
  render: () => (
    <Harness fetchStub={loaded}>
      <Health />
    </Harness>
  ),
  ready: async (canvas) => {
    await waitFor(() => expect(canvas.getByText("sonnet@eu")).toBeVisible());
    // a target's name had been squeezed to "gp…" beside four fixed-width figures
    for (const name of ["gpt-4o", "gpt-4o-mini"]) {
      const el = canvas.getByText(name);
      await expect(el.scrollWidth).toBeLessThanOrEqual(el.clientWidth);
    }
  },
});
export const MobileFits: Story = healthFits("mobile", "en");
export const MobileFitsInRussian: Story = healthFits("mobile", "ru");
export const SmallPhone: Story = healthFits("small", "en");

// ------------------------------------------------ live circuit breakers (#2254)

const breakerTarget = (patch: Partial<BreakerTargetDto>): BreakerTargetDto => ({
  model: "gpt-4o",
  target_index: 0,
  provider: "openai-dead",
  upstream_model: "gpt-4o",
  state: "open",
  nodes_open: 2,
  nodes_half_open: 0,
  nodes: [],
  ...patch,
});

const breakerState = (targets: BreakerTargetDto[], gateways_live = 3): BreakerStateDto => ({
  generated_at: "2026-09-29T08:00:00Z",
  fresh_window_secs: 40,
  gateways_live,
  targets,
});

const withBreakers = (answerBreakers: () => Response | Promise<Response>) =>
  scoped(async (input, init) => {
    if (String(input).includes("/breaker-state")) return answerBreakers();
    return loaded(input, init);
  });

/**
 * The live breakers sit apart from the 7-day SLA verdict and say what they are:
 * each gateway's in-memory state, reported about every 10 seconds. A provider
 * with an open or probing breaker is named with its affected models, and its
 * card carries the same badge; a provider whose breakers are closed is not.
 */
export const BreakersOpenAndProbing: Story = {
  render: () => (
    <Harness
      fetchStub={withBreakers(() =>
        json(
          breakerState([
            breakerTarget({ target_index: 0 }),
            breakerTarget({
              model: "gpt-4o-mini",
              target_index: 1,
              state: "half_open",
              nodes_open: 0,
              nodes_half_open: 1,
              upstream_model: "gpt-4o-mini",
            }),
            breakerTarget({
              model: "claude",
              provider: "anthropic",
              state: "closed",
              nodes_open: 0,
            }),
          ]),
        ),
      )}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const panel = await canvas.findByTestId("health-breakers");
    await expect(panel).toHaveTextContent(/3 gateways, reported about every 10 seconds/);
    await expect(panel).toHaveTextContent(/separate from the 7-day SLA verdict/);
    const dead = within(panel).getByTestId("health-breaker-openai-dead");
    await expect(dead).toHaveTextContent("gpt-4o (target 1) via gpt-4o");
    await expect(dead).toHaveTextContent("gpt-4o-mini (target 2)");
    await expect(within(dead).getAllByText("Breaker open on 2 gateways").length).toBeGreaterThan(0);
    await expect(within(dead).getByText("Probing on 1 gateway")).toBeVisible();
    // a closed breaker lists nothing
    await expect(within(panel).queryByTestId("health-breaker-anthropic")).toBeNull();
    // the provider's card carries the badge too
    const card = canvas.getByTestId("health-card-openai-dead");
    await expect(within(card).getByText("Breaker open on 2 gateways")).toBeVisible();
    await expect(
      within(canvas.getByTestId("health-card-anthropic")).queryByTestId("breaker-badge"),
    ).toBeNull();
  },
};

/** a superadmin's answer names each gateway */
export const BreakersPerGateway: Story = {
  render: () => (
    <Harness
      fetchStub={withBreakers(() =>
        json(
          breakerState([
            breakerTarget({
              nodes: [
                {
                  node_id: "gw-1",
                  state: "open",
                  consecutive_failures: 5,
                  retry_after_secs: 20,
                  reported_at: "2026-09-29T08:00:00Z",
                },
                {
                  node_id: "gw-2",
                  state: "open",
                  consecutive_failures: 6,
                  reported_at: "2026-09-29T08:00:00Z",
                },
                {
                  node_id: "gw-3",
                  state: "closed",
                  consecutive_failures: 0,
                  reported_at: "2026-09-29T08:00:00Z",
                },
              ],
            }),
          ]),
        ),
      )}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const panel = await within(canvasElement).findByTestId("health-breakers");
    await expect(panel).toHaveTextContent(/gw-1 · open · 5 consecutive failures · retries in 20s/);
    await expect(panel).toHaveTextContent(/gw-2 · open · 6 consecutive failures/);
    await expect(panel).not.toHaveTextContent(/gw-3/);
  },
};

/** every live gateway reports no tripped breaker */
export const BreakersNoneOpen: Story = {
  render: () => (
    <Harness fetchStub={withBreakers(() => json(breakerState([])))}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const panel = await within(canvasElement).findByTestId("health-breakers");
    await expect(panel).toHaveTextContent(/No breaker is open or probing on any live gateway/);
    await expect(within(canvasElement).queryByTestId("breaker-badge")).toBeNull();
  },
};

/** nothing polls the gateways: the panel says it can say nothing, not that all is well */
export const BreakersNoGatewayReporting: Story = {
  render: () => (
    <Harness fetchStub={withBreakers(() => json(breakerState([], 0)))}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const panel = await within(canvasElement).findByTestId("health-breakers");
    await expect(panel).toHaveTextContent(/No gateway is reporting its breakers/);
    await expect(panel).not.toHaveTextContent(/No breaker is open/);
  },
};

/** a control plane without a store: no panel, no error, the screen as it was */
export const BreakerStateUnavailable: Story = {
  render: () => (
    <Harness fetchStub={withBreakers(() => json({ error: { message: "no store" } }, 404))}>
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText("sonnet@eu")).toBeVisible());
    await expect(canvas.queryByTestId("health-breakers")).toBeNull();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

const ACME = { id: "org-2", name: "Acme", slug: "acme", created_at: "2026-01-01T00:00:00Z" };

const twinRow = (org: string, uptime: number, breached: boolean): UptimeRow => ({
  org_id: org,
  provider: "openai",
  target_id: "openai",
  grain: "provider",
  sources: ["probe"],
  events: 1000,
  ok: Math.round(uptime * 1000),
  errors: 1000 - Math.round(uptime * 1000),
  timeouts: 0,
  uptime,
  failure_rate: 1 - uptime,
  error_budget_burn: (1 - uptime) / 0.01,
  sla_breached: breached ? 1 : 0,
  last_event: "2026-08-06T10:00:00Z",
});

/**
 * #3441, #3460: two organizations each have a provider called `openai`. They
 * are two cards, each with its own organization's uptime and its own breakers,
 * and each says whose it is; one organization's open breaker never lands on the
 * other's card.
 */
export const TwoOrgsShareAProviderName: Story = {
  render: () => (
    <Harness
      fetchStub={async (input, init) => {
        const path = new URL(String(input), "http://localhost").pathname;
        if (path === "/api/v1/orgs") return json([ORG, ACME]);
        return scoped(
          routes([
            [
              "/health/uptime",
              () => ({ data: [twinRow(ORG.id, 0.5, true), twinRow(ACME.id, 0.9995, false)] }),
            ],
            ["/health/mttr", () => ({ data: [] })],
            ["/health/timeline", () => ({ data: [] })],
            [
              "/breaker-state",
              () => breakerState([breakerTarget({ org_id: ORG.id, provider: "openai" })]),
            ],
          ]),
        )(input, init);
      }}
    >
      <Health />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getAllByTestId("health-card-openai")).toHaveLength(2));
    const card = (org: string) =>
      canvas.getAllByTestId("health-card-openai").find((c) => c.dataset.org === org)!;
    await waitFor(() => expect(within(card(ORG.id)).getByText("Org: Rolter")).toBeVisible());
    await expect(within(card(ACME.id)).getByText("Org: Acme")).toBeVisible();
    // each card its own figures
    await expect(within(card(ORG.id)).getByText("50.00%")).toBeVisible();
    await expect(within(card(ACME.id)).queryByText("50.00%")).toBeNull();
    // the open breaker is on Rolter's routes, so only Rolter's card shows it
    await waitFor(() =>
      expect(within(card(ORG.id)).getByText("Breaker open on 2 gateways")).toBeVisible(),
    );
    await expect(within(card(ACME.id)).queryByTestId("breaker-badge")).toBeNull();
  },
};
