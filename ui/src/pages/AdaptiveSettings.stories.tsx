import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import AdaptiveSettings from "./AdaptiveSettings";
import {
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  json,
  Toasted,
  type FetchStub,
  type StoryRole,
} from "./story-harness";
import type { AdaptiveRoutingPolicyDto } from "@/lib/api";

const BASE: AdaptiveRoutingPolicyDto = {
  enabled: true,
  latency_weight: 1,
  cost_weight: 0.5,
  load_weight: 0.25,
  exploration_ratio: 0.05,
  min_samples: 50,
  updated_at: "2026-07-31T12:00:00Z",
  affected_routes: ["gpt-4o", "claude-sonnet"],
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
      <Toasted>
        <AdaptiveSettings />
      </Toasted>
    </ScreenHarness>
  );
}

const meta = {
  title: "Screens/AdaptiveSettings",
  component: AdaptiveSettings,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof AdaptiveSettings>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Loaded: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the blast radius of the kill switch is visible before it is flipped
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
    await expect(canvas.getByText("claude-sonnet")).toBeVisible();
  },
};

export const Loading: Story = {
  render: () => <Harness fetchStub={() => new Promise<Response>(() => {})} />,
  play: async ({ canvasElement }) => {
    await expectSkeleton(canvasElement);
  },
};

// a deployment where nothing is on the adaptive strategy yet
export const NoAffectedRoutes: Story = {
  render: () => <Harness fetchStub={async () => json({ ...BASE, affected_routes: [] })} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() =>
      expect(canvas.getByText("No route currently uses the adaptive strategy.")).toBeVisible(),
    );
  },
};

// a non-superadmin principal gets 403
export const Forbidden: Story = {
  render: () => <Harness fetchStub={async () => json({ error: { message: "forbidden" } }, 403)} />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

// an all-zero blend does not stop adaptive routing, it turns the strategy into
// a random balancer — the kill switch is the honest way to stop it
export const RejectsAnAllZeroBlend: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const label of ["Latency weight", "Cost weight", "Load weight"]) {
      const field = await canvas.findByLabelText(label);
      await userEvent.clear(field);
      await userEvent.type(field, "0");
    }
    await waitFor(() =>
      expect(canvas.getByText(/At least one weight must be positive/)).toBeVisible(),
    );
    await expect(canvas.getByRole("button", { name: "Save Changes" })).toBeDisabled();
  },
};

// the gateway clamps above 0.5, so the value is refused rather than quietly
// reinterpreted
export const RejectsAnOutOfRangeExplorationRatio: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const ratio = await canvas.findByLabelText("Exploration ratio");
    await userEvent.clear(ratio);
    await userEvent.type(ratio, "0.9");
    await waitFor(() =>
      expect(canvas.getByText("Exploration ratio must be between 0 and 0.5.")).toBeVisible(),
    );
  },
};

// interaction: a valid edit round-trips and confirms
export const SavesChanges: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ ...BASE, ...JSON.parse(String(init.body)) });
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cost = await canvas.findByLabelText("Cost weight");
    await userEvent.clear(cost);
    await userEvent.type(cost, "2");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /adaptive routing settings updated/i);
    await expect(canvas.getByLabelText("Cost weight")).toHaveValue("2");
  },
};

/**
 * The save is refused (#1607).
 *
 * `SavesChanges` covers the answer; this covers the other one. The refusal
 * reaches the toast queue, and the form keeps the value that was typed rather
 * than snapping back to the setting the server last confirmed — a settings
 * screen that reverts on a rejected save loses the edit without saying so.
 */
export const SaveRejectedByTheServer: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json(
          { error: { message: "the blend must leave at least one weight non-zero" } },
          422,
        );
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const field = await canvas.findByLabelText("Cost weight");
    await userEvent.clear(field);
    await userEvent.type(field, "2");
    await userEvent.click(canvas.getByRole("button", { name: "Save Changes" }));
    await expectToast(canvasElement, /non-zero/, "error");
    await waitFor(() => expect(canvas.getByLabelText("Cost weight")).toHaveValue("2"));
  },
};

// What a non-superadmin gets, which is the screen refused before it asks
// (#1606).
//
// `the adaptive-routing policy` is a deployment-scoped resource, so `superadminOnly` never mounts
// the screen for an org role however high. The stub answers the screen's own
// request with a perfectly good payload on purpose: if the wrapper is dropped
// the screen renders that payload and this story fails, which the `Forbidden`
// story below cannot do — it stubs the 403 itself, so it passes either way.
export const RefusedToAnAdmin: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} role="admin" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};

export const RefusedToAViewer: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} role="viewer" />,
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
  },
};
