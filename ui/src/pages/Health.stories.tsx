import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import Health from "./Health";
import {
  Harness,
  expectEmptyState,
  expectLoadError,
  expectSkeleton,
  json,
  pending,
  routes,
  scoped,
} from "./story-harness";
import type { MttrRow, TimelineRow, UptimeRow } from "@/lib/api";
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
    await expect(within(dead).getByText("probed · probe, status_page")).toBeVisible();
    await expect(within(dead).getByText("uptime · 200 events")).toBeVisible();
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

    // the line above the grid says what "at risk" means
    await expect(canvas.getByText(/SLA at risk means/)).toBeVisible();
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
