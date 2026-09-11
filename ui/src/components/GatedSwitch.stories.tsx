import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";
import * as React from "react";

import { GatedSwitch } from "./GatedSwitch";
import { Harness, routes } from "@/pages/story-harness";

// The row toggle that knows whether the caller may flip it (#1258).
//
// Same three answers as `GatedButton` — allowed, refused, not known yet — and
// the same rule that only an explicit "no" disables. The difference is what a
// live one costs: a refused switch moves, sends the update, and snaps back when
// the 403 lands, which reads as the deployment losing the write rather than
// never accepting it.

const stub = routes([]);

const meta = {
  title: "Controls/GatedSwitch",
  component: GatedSwitch,
  parameters: { layout: "centered" },
  // every story supplies its own `render`; these are the args the docs page
  // introspects, and `gate` is required so they cannot be left off
  args: { gate: "provider:update", checked: true, "aria-label": "Enable openai-prod" },
} satisfies Meta<typeof GatedSwitch>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Allowed: Story = {
  render: () => (
    <Harness fetchStub={stub} role="admin">
      <GatedSwitch gate="provider:update" checked aria-label="Enable openai-prod" />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const toggle = within(canvasElement).getByRole("switch");
    await waitFor(() => expect(toggle).toBeEnabled());
  },
};

export const Refused: Story = {
  render: () => (
    <Harness fetchStub={stub} role="viewer">
      <GatedSwitch gate="provider:update" checked aria-label="Enable openai-prod" />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const toggle = within(canvasElement).getByRole("switch");
    await waitFor(() => expect(toggle).toBeDisabled());
    // the track carries no text, so the title is the only place the reason fits
    await expect(toggle).toHaveAttribute("title", "Requires the Admin role");
  },
};

export const Unknown: Story = {
  render: () => (
    // no role, so no answer: a control plane a version behind leaves the
    // dashboard here, and a toggle that disables itself over that would look
    // like the deployment went read-only
    <Harness fetchStub={stub}>
      <GatedSwitch gate="provider:update" checked aria-label="Enable openai-prod" />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByRole("switch")).toBeEnabled();
  },
};

// the gate never overrides a disable the screen already had for its own reason
export const AlreadyDisabled: Story = {
  render: () => (
    <Harness fetchStub={stub} role="admin">
      <GatedSwitch gate="provider:update" checked disabled aria-label="Enable openai-prod" />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByRole("switch")).toBeDisabled();
  },
};

// a refused toggle does not move: the row must not flip and then snap back
export const RefusedDoesNotFlip: Story = {
  render: function Render() {
    const [on, setOn] = React.useState(true);
    return (
      <Harness fetchStub={stub} role="viewer">
        <div className="flex flex-col items-center gap-2">
          <GatedSwitch
            gate="provider:update"
            checked={on}
            onCheckedChange={setOn}
            aria-label="Enable openai-prod"
          />
          <span data-testid="state">{on ? "on" : "off"}</span>
        </div>
      </Harness>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const toggle = canvas.getByRole("switch");
    await waitFor(() => expect(toggle).toBeDisabled());
    await userEvent.click(toggle, { pointerEventsCheck: 0 });
    await expect(canvas.getByTestId("state")).toHaveTextContent("on");
  },
};
