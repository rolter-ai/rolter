import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor } from "storybook/test";

import type { AdaptiveRoutingPolicyDto, AdaptiveRoutingTelemetryDto } from "@/lib/api";
import AdaptiveDashboard from "./AdaptiveDashboard";
import {
  expectForbidden,
  expectSkeleton,
  Harness as ScreenHarness,
  json,
  type FetchStub,
  type StoryRole,
} from "./story-harness";

const TELEMETRY: AdaptiveRoutingTelemetryDto = {
  generated_at: "2026-08-02T01:15:00Z",
  fresh_window_secs: 60,
  routes: [
    {
      model: "gpt-4o",
      engaged: true,
      nodes: [
        {
          node_id: "gateway-eu-1",
          engaged: true,
          observed: 1_284,
          decisions: { blend: 1_056, exploration: 64, fallback: 164 },
          policy: {
            enabled: true,
            latency_weight: 0.55,
            cost_weight: 0.25,
            load_weight: 0.2,
            exploration_ratio: 0.05,
            min_samples: 100,
          },
          targets: [
            {
              target: 0,
              provider: "openai-primary",
              upstream_model: "gpt-4o-2024-11-20",
              score: 0.814,
              latency_score: 0.91,
              cost_score: 0.42,
              load_score: 0.72,
              latency_ms: 238.4,
              cost_per_mtok: 7.5,
              in_flight: 4,
              samples: 842,
              last_sample_age_ms: 1_200,
            },
            {
              target: 1,
              provider: "azure-west",
              upstream_model: "gpt-4o",
              score: 0.667,
              latency_score: 0.62,
              cost_score: 0.73,
              load_score: 0.7,
              latency_ms: 311.2,
              cost_per_mtok: 6.8,
              in_flight: 2,
              samples: 442,
              last_sample_age_ms: 850,
            },
          ],
          reported_at: "2026-08-02T01:14:52Z",
        },
        {
          node_id: "gateway-us-2",
          engaged: false,
          observed: 42,
          decisions: { blend: 0, exploration: 2, fallback: 40 },
          policy: {
            enabled: true,
            latency_weight: 0.55,
            cost_weight: 0.25,
            load_weight: 0.2,
            exploration_ratio: 0.05,
            min_samples: 100,
          },
          targets: [
            {
              target: 0,
              provider: "openai-us",
              upstream_model: "gpt-4o",
              score: 0.5,
              latency_ms: 0,
              cost_per_mtok: 7.5,
              in_flight: 0,
              samples: 42,
              last_sample_age_ms: 2_400,
            },
          ],
          reported_at: "2026-08-02T01:14:48Z",
        },
      ],
    },
    {
      model: "claude-sonnet",
      engaged: false,
      nodes: [
        {
          node_id: "gateway-eu-1",
          engaged: false,
          observed: 18,
          decisions: { blend: 0, exploration: 0, fallback: 18 },
          policy: {
            enabled: false,
            latency_weight: 1,
            cost_weight: 0,
            load_weight: 0,
            exploration_ratio: 0.02,
            min_samples: 50,
          },
          targets: [],
          reported_at: "2026-08-02T01:14:52Z",
        },
      ],
    },
  ],
};

/**
 * The screen under the shared fetch-stub harness, with a role to render as.
 *
 * `role` is what a story needs to mount a `CapabilityProvider` at all: with no
 * provider above it `can()` answers "unknown", the `superadminOnly` wrapper
 * never blocks, and a story can only reach the 403 by stubbing one — which
 * tests the screen's own error path rather than the gate (#1606).
 */
function Harness({ fetchStub, role }: { fetchStub: FetchStub; role?: StoryRole }) {
  return (
    <ScreenHarness fetchStub={fetchStub} role={role}>
    <AdaptiveDashboard />
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/AdaptiveDashboard",
  component: AdaptiveDashboard,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof AdaptiveDashboard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(TELEMETRY)} />,
  play: async ({ canvas }) => {
    await waitFor(() => expect(canvas.getByText("BLEND ACTIVE")).toBeVisible());
    await expect(canvas.getAllByText("DISABLED")).toHaveLength(2);
    await expect(canvas.getByRole("cell", { name: "0.814" })).toBeVisible();

    // Additional gateways stay compact until the operator asks for their
    // node-specific signals. Native details/summary preserves keyboard access.
    await userEvent.click(canvas.getByText("gateway-us-2"));
    await expect(canvas.getByRole("rowheader", { name: "openai-us / gpt-4o" })).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

/**
 * The deployment-wide kill switch, as the dashboard reads it (#1648).
 *
 * The screen only reaches for this to explain an empty list, so every story
 * below drives it: an empty screen that blames the gateway while the switch is
 * off is the bug, and it is invisible unless both answers are stubbed.
 */
const POLICY: AdaptiveRoutingPolicyDto = {
  enabled: true,
  latency_weight: 0.55,
  cost_weight: 0.25,
  load_weight: 0.2,
  exploration_ratio: 0.05,
  min_samples: 100,
  updated_at: "2026-08-02T01:00:00Z",
  affected_routes: ["deepseek-r1"],
};

/** Answer the two queries the screen makes, and nothing else. */
function stub(
  telemetry: AdaptiveRoutingTelemetryDto,
  policy: Partial<AdaptiveRoutingPolicyDto> = {},
): FetchStub {
  return async (input) =>
    String(input).includes("adaptive-routing-policy")
      ? json({ ...POLICY, ...policy } satisfies AdaptiveRoutingPolicyDto)
      : json(telemetry);
}

const NOTHING: AdaptiveRoutingTelemetryDto = { ...TELEMETRY, routes: [] };

/**
 * Nothing reporting while the switch is *on* — the gateway really is the place
 * to look, and the copy says what makes a report invisible here.
 */
export const Empty: Story = {
  render: () => <Harness fetchStub={stub(NOTHING, { enabled: true })} />,
  play: async ({ canvas }) => {
    await waitFor(() =>
      expect(canvas.getByText("No fresh adaptive routing telemetry")).toBeVisible(),
    );
    // a gateway with no node id is dropped by the control plane, which looked
    // exactly like a gateway that never reported (#1644)
    await expect(canvas.getByText(/without a node id is dropped/)).toBeVisible();
    await expect(canvas.getByRole("link", { name: "Review routing rules" })).toHaveAttribute(
      "href",
      "/routing-rules",
    );
  },
};

/**
 * The switch is off, which is the shipped default: the screen names the switch
 * and the routes it is holding back instead of sending the operator to debug a
 * gateway that is behaving correctly (#1648).
 */
export const SwitchedOff: Story = {
  render: () => <Harness fetchStub={stub(NOTHING, { enabled: false })} />,
  play: async ({ canvas }) => {
    await waitFor(() =>
      expect(canvas.getByText("Adaptive routing is switched off")).toBeVisible(),
    );
    // the gateway is not at fault here, so it is not named
    await expect(canvas.queryByText("No fresh adaptive routing telemetry")).toBeNull();
    await expect(canvas.getByText("1 route is held back:")).toBeVisible();
    await expect(canvas.getByText("deepseek-r1")).toBeVisible();
    // and the CTA goes where the switch is, not to the routing rules
    await expect(
      canvas.getByRole("link", { name: "Open adaptive routing settings" }),
    ).toHaveAttribute("href", "/adaptive-settings");
  },
};

/** The switch is off and no route asks for the strategy — nothing to list. */
export const SwitchedOffWithNoAdaptiveRoutes: Story = {
  render: () => (
    <Harness fetchStub={stub(NOTHING, { enabled: false, affected_routes: [] })} />
  ),
  play: async ({ canvas }) => {
    await waitFor(() =>
      expect(canvas.getByText("Adaptive routing is switched off")).toBeVisible(),
    );
    await expect(canvas.queryByText(/held back:/)).toBeNull();
  },
};

/**
 * The policy read failed. It is never what this screen waits on, so the empty
 * state falls back to the generic copy rather than accusing a switch it could
 * not read of being off.
 */
export const SwitchStateUnknown: Story = {
  render: () => (
    <Harness
      fetchStub={async (input) =>
        String(input).includes("adaptive-routing-policy")
          ? json({ error: { message: "forbidden" } }, 403)
          : json(NOTHING)
      }
    />
  ),
  play: async ({ canvas }) => {
    await waitFor(() =>
      expect(canvas.getByText("No fresh adaptive routing telemetry")).toBeVisible(),
    );
    await expect(canvas.queryByText("Adaptive routing is switched off")).toBeNull();
    // and no error panel over a screen whose own query answered fine
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

export const Forbidden: Story = {
  render: () => (
    <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />
  ),
  play: async ({ canvas }) => {
    await waitFor(() =>
      expect(canvas.getByRole("alert")).toHaveTextContent(
        /You do not have access to adaptive routing telemetry/,
      ),
    );
  },
};

export const RetryAfterFailure: Story = {
  render: () => {
    let attempts = 0;
    return (
      <Harness
        fetchStub={async () => {
          attempts += 1;
          return attempts === 1
            ? json({ error: { message: "temporarily unavailable" } }, 503)
            : json(TELEMETRY);
        }}
      />
    );
  },
  play: async ({ canvas }) => {
    await userEvent.click(await canvas.findByRole("button", { name: "Try again" }));
    await waitFor(() => expect(canvas.getByText("BLEND ACTIVE")).toBeVisible());
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// `the adaptive-routing telemetry a superadmin alone reads` is a deployment-scoped resource, so `superadminOnly` never mounts
// the screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story below cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(TELEMETRY)} role="admin" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(TELEMETRY)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};
