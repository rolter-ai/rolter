import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { DeleteIconButton } from "./delete-icon-button";

const meta = {
  title: "Primitives/DeleteIconButton",
  component: DeleteIconButton,
  parameters: { layout: "padded" },
  args: { label: "Delete provider openai" },
} satisfies Meta<typeof DeleteIconButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Pending: Story = { args: { pending: true } };

export const Denied: Story = {
  args: { disabled: true, title: "You need providers:write to delete a provider" },
};

/**
 * The control is an icon with no text, so `label` is the only name it has — and
 * it names the row, not the action, so a list of eight delete buttons does not
 * read as eight identical ones.
 */
export const NamesTheRow: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Delete provider openai" });
    await expect(button).toBeEnabled();
    // the hover title falls back to the name rather than being left empty
    await expect(button).toHaveAttribute("title", "Delete provider openai");
  },
};

/**
 * A delete in flight swaps the bin for a spinner and locks the button.
 *
 * `Providers` and `ProviderGroups` had the same markup as `Models` and neither
 * did this, so the same action looked dead on two screens and alive on the
 * third (#1686). Pressing it twice queued two deletes.
 */
export const PendingBlocksASecondPress: Story = {
  render: function Render() {
    const [presses, setPresses] = React.useState(0);
    return (
      <div className="flex items-center gap-2 text-sm">
        <DeleteIconButton
          label="Delete provider openai"
          pending
          onClick={() => setPresses(presses + 1)}
        />
        <span>{presses} press(es)</span>
      </div>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Delete provider openai" });
    await expect(button).toBeDisabled();
    await userEvent.click(button, { pointerEventsCheck: 0 });
    await expect(canvas.getByText("0 press(es)")).toBeInTheDocument();
  },
};

/** A gated row keeps the control visible and says why it is dead in the title. */
export const DeniedSaysWhy: Story = {
  args: { disabled: true, title: "You need providers:write to delete a provider" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Delete provider openai" });
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute("title", "You need providers:write to delete a provider");
  },
};
