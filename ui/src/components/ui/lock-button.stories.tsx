import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { LockButton } from "./lock-button";

const LOCKED = "Locked — clients can't override. Click to unlock.";
const UNLOCKED = "Unlocked — clients can override. Click to lock.";

const meta = {
  title: "Forms/LockButton",
  component: LockButton,
  parameters: { layout: "padded" },
  args: { locked: true, onToggle: () => {} },
} satisfies Meta<typeof LockButton>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({ initial = false, disabled = false }: { initial?: boolean; disabled?: boolean }) {
  const [locked, setLocked] = React.useState(initial);
  return (
    <div className="flex items-center gap-2 text-sm">
      <LockButton locked={locked} onToggle={() => setLocked((v) => !v)} disabled={disabled} />
      <span>temperature</span>
    </div>
  );
}

export const Unlocked: Story = { render: () => <Controlled /> };
export const Locked: Story = { render: () => <Controlled initial /> };
export const Disabled: Story = { render: () => <Controlled initial disabled /> };

/**
 * The padlock is an icon, so its name has to carry both halves: what the state
 * is now and what pressing it does. `aria-pressed` is what makes it a toggle
 * rather than a button that looks stateful.
 */
export const NamesStateAndAction: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // bites if the label stops following `locked`, or loses the "click to…"
    // half that says what a press will do
    const button = canvas.getByRole("button", { name: UNLOCKED });
    await expect(button).toHaveAttribute("aria-pressed", "false");
    await expect(button).toHaveAttribute("title", UNLOCKED);
  },
};

/** Pressing it flips both the state and the name it reports. */
export const TogglesOnClick: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: UNLOCKED }));
    const button = canvas.getByRole("button", { name: LOCKED });
    await expect(button).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(button);
    await expect(canvas.getByRole("button", { name: UNLOCKED })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  },
};

/** A view-only sheet renders the padlock inert rather than hiding the state. */
export const DisabledIgnoresClicks: Story = {
  render: () => <Controlled initial disabled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: LOCKED });
    await expect(button).toBeDisabled();
    await userEvent.click(button, { pointerEventsCheck: 0 });
    await expect(canvas.getByRole("button", { name: LOCKED })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  },
};
