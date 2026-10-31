import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Link, useLocation } from "react-router";

import AdaptiveSettings from "./AdaptiveSettings";
import {
  expectForbidden,
  expectSkeleton,
  expectToast,
  Harness as ScreenHarness,
  expectRetryRecovers,
  failingThenRecovering,
  json,
  recording,
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
    // the exploration share is typed as the percent it means, not as 0.05
    await expect(await canvas.findByLabelText("Exploration ratio")).toHaveValue("5");
    // a save shows nothing on this screen, so the live view is one link away
    await expect(
      canvas.getByRole("link", { name: "See live routing on the dashboard" }),
    ).toHaveAttribute("href", "/adaptive-dashboard");
    await expect(canvas.getByText("Blend weights")).toBeVisible();
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
    // the blend has no single culprit, so the message sits on the first weight
    const latency = canvas.getByLabelText("Latency weight");
    await waitFor(() => expect(latency).toHaveAttribute("aria-invalid", "true"));
    await expect(latency).toHaveAccessibleDescription(/At least one weight must be positive/);
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

// the gateway clamps above 50%, so the value is refused rather than quietly
// reinterpreted
export const RejectsAnOutOfRangeExplorationRatio: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const ratio = await canvas.findByLabelText("Exploration ratio");
    await userEvent.clear(ratio);
    await userEvent.type(ratio, "90");
    await waitFor(() => expect(ratio).toHaveAttribute("aria-invalid", "true"));
    await expect(ratio).toHaveAccessibleDescription("Exploration ratio must be between 0 and 50%.");
  },
};

// every failing field is marked at once, each with its own message, and a
// press on Save moves focus to the first one instead of doing nothing (#2651)
export const MarksEveryInvalidField: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cost = await canvas.findByLabelText("Cost weight");
    const samples = canvas.getByLabelText("Warm-up samples");
    await userEvent.clear(cost);
    await userEvent.type(cost, "-1");
    await userEvent.clear(samples);
    await userEvent.type(samples, "1.5");
    await waitFor(() => {
      expect(cost).toHaveAttribute("aria-invalid", "true");
      expect(samples).toHaveAttribute("aria-invalid", "true");
    });
    await expect(cost).toHaveAccessibleDescription(/Each weight must be between 0 and/);
    await expect(samples).toHaveAccessibleDescription(
      /Warm-up samples must be a whole number between 0 and/,
    );
    await expect(canvas.getByLabelText("Latency weight")).not.toHaveAttribute("aria-invalid");
    await expect(canvas.getByText("2 fields need attention")).toBeVisible();
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(cost).toHaveFocus();
  },
};

// a 400 that names a field lands on that field, with focus, not in a toast
export const ServerRejectionLandsOnTheField: Story = {
  render: () => {
    const stub: FetchStub = async (_input, init) => {
      if (init?.method === "PUT") {
        return json({ error: { message: "exploration_ratio must be between 0 and 0.5" } }, 400);
      }
      return json(BASE);
    };
    return <Harness fetchStub={stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const ratio = await canvas.findByLabelText("Exploration ratio");
    await userEvent.clear(ratio);
    await userEvent.type(ratio, "10");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ratio).toHaveAttribute("aria-invalid", "true"));
    await expect(ratio).toHaveAccessibleDescription("exploration_ratio must be between 0 and 0.5");
    await waitFor(() => expect(ratio).toHaveFocus());
  },
};

// interaction: a valid edit round-trips and confirms
let rec = recording(async () => json(BASE));

export const SavesChanges: Story = {
  render: () => {
    rec = recording(async (_input, init) =>
      init?.method === "PUT" ? json({ ...BASE, ...JSON.parse(String(init.body)) }) : json(BASE),
    );
    return <Harness fetchStub={rec.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cost = await canvas.findByLabelText("Cost weight");
    await userEvent.clear(cost);
    await userEvent.type(cost, "2");
    const ratio = canvas.getByLabelText("Exploration ratio");
    await userEvent.clear(ratio);
    await userEvent.type(ratio, "7");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expectToast(canvasElement, /adaptive routing settings updated/i);
    // the percent typed travels as the fraction the API takes
    const body = await rec.expectSentBody<{ exploration_ratio: number }>("PUT", "adaptive-routing");
    await expect(body.exploration_ratio).toBeCloseTo(0.07, 6);
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
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
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

// the control plane answers 5xx: the retry is on offer, reaches the network,
// and the screen comes back once the control plane does (#2220)
const adaptiveSettingsDown = failingThenRecovering("/api/v1/adaptive-routing-policy", async () =>
  json(BASE),
);
export const LoadFailsAndRetries: Story = {
  render: () => {
    adaptiveSettingsDown.reset();
    return <Harness fetchStub={adaptiveSettingsDown.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expectRetryRecovers(
      canvasElement,
      adaptiveSettingsDown,
      /failed to return adaptive routing settings/,
      async () => await expect(await canvas.findByLabelText("Exploration ratio")).toHaveValue("5"),
    );
  },
};

// the shared settings footer (#2214): a switch waits for Save and says so, a
// pristine screen has nothing to save, and a save says when and how far it got
function Elsewhere() {
  const { pathname } = useLocation();
  return (
    <>
      <Link to="/elsewhere">Go elsewhere</Link>
      <output data-testid="pathname">{pathname}</output>
    </>
  );
}

const gateway = (id: string, converged: boolean) => ({
  id,
  role: "gateway",
  build_version: "1.0.0",
  config_version: converged ? 8 : 7,
  desired_state: "active",
  state_changed_at: "2026-07-31T12:00:00Z",
  first_seen_at: "2026-07-31T12:00:00Z",
  last_seen_at: "2026-07-31T12:00:00Z",
  live: true,
  converged,
});

const savingTo = (nodes: unknown[]) =>
  recording(async (input, init) => {
    if (String(input).includes("/cluster/nodes")) return json(nodes);
    return init?.method === "PUT"
      ? json({ ...BASE, ...JSON.parse(String(init.body)) })
      : json(BASE);
  });

export const KillSwitchWaitsForSave: Story = {
  render: () => <Harness fetchStub={async () => json(BASE)} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const kill = await canvas.findByRole("switch");
    const save = canvas.getByRole("button", { name: "Save changes" });
    // untouched: nothing to save, nothing to discard, and the screen says when it was last saved
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(canvas.getByRole("button", { name: "Discard changes" })).toBeDisabled();
    await expect(canvas.getByText(/^Last saved /)).toBeVisible();
    await expect(canvas.queryByText("Not saved")).toBeNull();
    // flipped: the switch moved, the gateways have not heard of it, and the screen says so
    await userEvent.click(kill);
    await expect(kill).toHaveAttribute("aria-checked", "false");
    await expect(canvas.getByText("Not saved")).toBeVisible();
    await expect(canvas.getByText(/^1 change not saved yet/)).toHaveAttribute("role", "status");
    await expect(save).not.toHaveAttribute("aria-disabled");
    // discarded: back to what the gateways run
    await userEvent.click(canvas.getByRole("button", { name: "Discard changes" }));
    await expect(kill).toHaveAttribute("aria-checked", "true");
    await expect(canvas.queryByText("Not saved")).toBeNull();
    await expect(save).toHaveAttribute("aria-disabled", "true");
  },
};

export const SavedLineSaysHowManyGatewaysRunIt: Story = {
  render: () => {
    rec = savingTo([gateway("gw-a", true), gateway("gw-b", false)]);
    return <Harness fetchStub={rec.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch"));
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(await canvas.findByText(/^Saved at /)).toBeVisible();
    await expect(canvas.queryByText("Not saved")).toBeNull();
    // read from the config versions the Cluster screen reads
    await waitFor(() => expect(canvas.getByText(/Applied to 1 of 2 live gateways/)).toBeVisible());
  },
};

export const EnterInAFieldSaves: Story = {
  render: () => {
    rec = savingTo([]);
    return <Harness fetchStub={rec.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cost = await canvas.findByLabelText("Cost weight");
    await userEvent.clear(cost);
    await userEvent.type(cost, "3{Enter}");
    const body = await rec.expectSentBody<{ cost_weight: number }>("PUT", "adaptive-routing");
    await expect(body.cost_weight).toBe(3);
  },
};

export const EnterInAWrongFieldFocusesItInstead: Story = {
  render: () => {
    rec = savingTo([]);
    return <Harness fetchStub={rec.stub} />;
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const cost = await canvas.findByLabelText("Cost weight");
    await userEvent.clear(cost);
    await userEvent.type(cost, "-1{Enter}");
    await waitFor(() => expect(cost).toHaveAttribute("aria-invalid", "true"));
    await waitFor(() => expect(cost).toHaveFocus());
    rec.expectNotSent("PUT", "adaptive-routing");
  },
};

export const LeavingWithAnUnsavedSwitchAsksFirst: Story = {
  render: () => (
    <ScreenHarness fetchStub={async () => json(BASE)}>
      <AdaptiveSettings />
      <Elsewhere />
    </ScreenHarness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(await canvas.findByRole("switch"));
    await userEvent.click(canvas.getByRole("link", { name: "Go elsewhere" }));
    const prompt = within(await within(document.body).findByRole("dialog"));
    await waitFor(() => expect(prompt.getByText("Discard unsaved changes?")).toBeVisible());
    await userEvent.click(prompt.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(within(document.body).queryByRole("dialog")).toBeNull());
    await expect(canvas.getByTestId("pathname")).toHaveTextContent("/");
  },
};
