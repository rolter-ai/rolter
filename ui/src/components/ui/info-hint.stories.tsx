import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, within } from "storybook/test";

import { InfoHint } from "./info-hint";

const NOTE =
  "Requests are queued per provider once every worker is busy; past the capacity the gateway answers 429 rather than growing without bound.";

const meta = {
  title: "Primitives/InfoHint",
  component: InfoHint,
  parameters: { layout: "padded" },
  args: { text: NOTE },
} satisfies Meta<typeof InfoHint>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Closed is the resting state: the note costs nothing until it is asked for. */
export const Default: Story = {
  render: (args) => (
    <span className="inline-flex items-center gap-1.5 text-sm">
      Queue capacity
      <InfoHint {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument();
    await expect(canvas.getByRole("button")).toHaveAttribute("aria-expanded", "false");
  },
};

/** The trigger takes an explicit name where the surrounding label has one. */
export const NamedForItsField: Story = {
  args: { label: "About queue capacity" },
  render: (args) => (
    <span className="inline-flex items-center gap-1.5 text-sm">
      Queue capacity
      <InfoHint {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("button", { name: "About queue capacity" })).toBeVisible();
  },
};

/** Pointing at it opens the note and wires it up as the trigger's description. */
export const OpensOnHover: Story = {
  render: (args) => (
    <span className="inline-flex items-center gap-1.5 text-sm">
      Queue capacity
      <InfoHint {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button");
    await userEvent.hover(trigger);
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(/answers 429/);
    await expect(trigger).toHaveAttribute("aria-expanded", "true");
    await expect(trigger).toHaveAccessibleDescription(/answers 429/);
  },
};

/**
 * A press toggles, which on a touch device is the only way in and out of the
 * note. With a pointer the hover has already opened it, so the click that
 * follows shuts it again — asserted here so that stays a decision rather than
 * a surprise.
 */
export const ClickTogglesItShut: Story = {
  render: (args) => (
    <span className="inline-flex items-center gap-1.5 text-sm">
      Queue capacity
      <InfoHint {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const trigger = canvas.getByRole("button");
    await userEvent.click(trigger);
    await expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument();
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
  },
};

/**
 * Keyboard parity with the pointer: focus reveals the note and blur hides it
 * again, so the hint is not a mouse-only affordance.
 */
export const OpensOnFocus: Story = {
  render: (args) => (
    <span className="inline-flex items-center gap-1.5 text-sm">
      Queue capacity
      <InfoHint {...args} />
    </span>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.tab();
    await expect(canvas.getByRole("tooltip")).toBeVisible();
    await userEvent.tab();
    await expect(canvas.queryByRole("tooltip")).not.toBeInTheDocument();
  },
};
