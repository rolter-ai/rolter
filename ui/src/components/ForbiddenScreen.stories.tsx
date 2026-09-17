import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import { ForbiddenScreen, superadminOnly } from "./ForbiddenScreen";
import { CapabilityProvider } from "@/lib/can";
import { Harness, expectForbidden, json, pending, recording, scoped } from "@/pages/story-harness";

const meta = {
  title: "Screens/ForbiddenScreen",
  component: ForbiddenScreen,
  parameters: { layout: "fullscreen" },
} satisfies Meta<typeof ForbiddenScreen>;

export default meta;
type Story = StoryObj<typeof meta>;

/** the screen on its own: the same refusal a real 403 renders */
export const Default: Story = {
  args: { resource: "feature flags" },
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("alert")).toHaveTextContent(/feature flags/);
  },
};

// a settings screen that would give away what it holds if it mounted at all
function DeploymentSecrets() {
  return <p>runtime policy: 3 rules</p>;
}

const Gated = superadminOnly(DeploymentSecrets, "errors.resources.featureFlags");

/**
 * an org admin is not a superadmin, so the wrapper refuses before the screen
 * mounts — the body never renders and no request goes out to be refused
 */
export const NotASuperadmin: Story = {
  args: { resource: "" },
  render: () => (
    <Harness role="admin" fetchStub={pending}>
      <Gated />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectForbidden(canvasElement);
    const canvas = within(canvasElement);
    await expect(canvas.queryByText(/runtime policy/)).toBeNull();
  },
};

/** a superadmin passes the gate and gets the screen itself */
export const Superadmin: Story = {
  args: { resource: "" },
  render: () => (
    <Harness role="superadmin" fetchStub={pending}>
      <Gated />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText(/runtime policy/)).toBeVisible());
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

// the gate's question, left hanging or failed. `Harness role=` cannot say
// either: it answers `/api/v1/rbac/effective` itself, ahead of the story's
// stub, so the provider is mounted by hand and the stub keeps the last word
const unanswered = recording(
  scoped(async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/rbac/effective") return new Promise<Response>(() => {});
    return json({});
  }),
);

/**
 * the effective-permissions answer never arrives: only an explicit "not a
 * superadmin" blocks, so the screen still mounts and still gets its 403 the
 * old way rather than being hidden on a guess
 */
export const GateUnanswered: Story = {
  args: { resource: "" },
  render: () => (
    <Harness fetchStub={unanswered.stub}>
      <CapabilityProvider>
        <Gated />
      </CapabilityProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the question really is out and really is unanswered — without this the
    // story would pass just as well with no gate in play at all
    await unanswered.expectSent("GET", "/api/v1/rbac/effective");
    await expect(canvas.getByText(/runtime policy/)).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};

const failed = recording(
  scoped(async (input) => {
    const path = new URL(String(input), "http://localhost").pathname;
    if (path === "/api/v1/rbac/effective") return json({ error: "boom" }, 500);
    return json({});
  }),
);

/** a failed answer is not a "no" either: an rbac outage must not lock the settings away */
export const GateFailedToAnswer: Story = {
  args: { resource: "" },
  render: () => (
    <Harness fetchStub={failed.stub}>
      <CapabilityProvider>
        <Gated />
      </CapabilityProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await failed.expectSent("GET", "/api/v1/rbac/effective");
    // give the 500 time to land before saying the screen survived it
    await new Promise((resolve) => setTimeout(resolve, 100));
    await expect(canvas.getByText(/runtime policy/)).toBeVisible();
    await expect(canvas.queryByRole("alert")).toBeNull();
  },
};
