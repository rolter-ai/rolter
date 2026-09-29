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
    // the breached target is the one the operator has to act on, so it says so
    await expect(canvas.getAllByText("tripped").length).toBeGreaterThan(0);

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
