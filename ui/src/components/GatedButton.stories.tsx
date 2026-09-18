import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";
import * as React from "react";

import { GatedButton } from "./GatedButton";
import { expectAllowed, Harness, routes } from "@/pages/story-harness";

// The one control that knows whether the caller may press it (#1183).
//
// Three states, and only one of them disables: allowed, refused, and "not known
// yet". The third renders enabled on purpose — a button that starts disabled
// and enables itself a request later reads as broken, and an unanswerable
// capability query must not empty the dashboard.

const stub = routes([]);

const meta = {
  title: "Controls/GatedButton",
  component: GatedButton,
  parameters: { layout: "centered" },
  // every story supplies its own `render`; these are the args the docs page
  // introspects, and `gate` is required so they cannot be left off
  args: { gate: "provider:create", children: "Add provider" },
} satisfies Meta<typeof GatedButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Allowed: Story = {
  render: () => (
    <Harness fetchStub={stub} role="admin">
      <GatedButton gate="provider:create">Add provider</GatedButton>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // not a bare `toBeEnabled`: enabled is where this button starts, so that
    // assertion is true before the gate has said anything (#1707)
    await expectAllowed(canvasElement, "Add provider");
    await expect(within(canvasElement).getByRole("button")).not.toHaveAttribute("title");
  },
};

export const Refused: Story = {
  render: () => (
    <Harness fetchStub={stub} role="viewer">
      <GatedButton gate="provider:create">Add provider</GatedButton>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button");
    await waitFor(() => expect(button).toBeDisabled());
    // the reason, not just the refusal
    await expect(button).toHaveAttribute("title", "Requires the Admin role");
  },
};

export const RefusedToEveryone: Story = {
  render: () => (
    <Harness fetchStub={stub} role="admin">
      <GatedButton gate="feature_flags:update">Save changes</GatedButton>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button");
    await waitFor(() => expect(button).toBeDisabled());
    // deployment-wide policy is nobody's role at a scope — naming one would
    // describe a floor nobody can stand on
    await expect(button).toHaveAttribute("title", "Requires a superadmin");
  },
};

export const Unknown: Story = {
  render: () => (
    // no role, so no provider and no answer: exactly what a control plane a
    // version behind, or one that never answers, leaves the dashboard with
    <Harness fetchStub={stub}>
      <GatedButton gate="provider:create">Add provider</GatedButton>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button");
    await expect(button).toBeEnabled();
  },
};

// the gate never overrides a disable the screen already had for its own reason
export const AlreadyDisabled: Story = {
  render: () => (
    <Harness fetchStub={stub} role="admin">
      <GatedButton gate="provider:create" disabled>
        Add provider
      </GatedButton>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    // story-wait-allow: disabled by its own prop from the first paint, so there
    // is no gate answer to wait for - that it stays disabled is the point
    await expect(within(canvasElement).getByRole("button")).toBeDisabled();
  },
};

// a refused button still swallows the click: `disabled` is real, and the
// pointer-events override exists only so the tooltip can be read
export const RefusedSwallowsTheClick: Story = {
  render: function Render() {
    const [clicks, setClicks] = React.useState(0);
    return (
      <Harness fetchStub={stub} role="viewer">
        <div className="flex flex-col items-center gap-2">
          <GatedButton gate="provider:create" onClick={() => setClicks((n) => n + 1)}>
            Add provider
          </GatedButton>
          <span data-testid="clicks">{clicks}</span>
        </div>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button");
    await waitFor(() => expect(button).toBeDisabled());
    await userEvent.click(button, { pointerEventsCheck: 0 });
    await expect(canvas.getByTestId("clicks")).toHaveTextContent("0");
  },
};
