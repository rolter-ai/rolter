import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, fn, userEvent, within } from "storybook/test";

import { Button } from "./button";
import { SaveBar } from "./save-bar";

const meta = {
  title: "Forms/SaveBar",
  component: SaveBar,
  parameters: { layout: "padded" },
  args: { invalid: 0, onSave: fn() },
} satisfies Meta<typeof SaveBar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Clean: Story = {
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(args.onSave).toHaveBeenCalledTimes(1);
    await expect(canvas.queryByRole("status")).toBeNull();
  },
};

/** Invalid: the count is announced and Save stays pressable (aria-disabled, not disabled). */
export const FieldsNeedAttention: Story = {
  args: { invalid: 2 },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("status")).toHaveTextContent("2 fields need attention");
    const save = canvas.getByRole("button", { name: "Save changes" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(save).toBeEnabled();
    await expect(save).toHaveAccessibleDescription("2 fields need attention");
    await userEvent.click(save);
    await expect(args.onSave).toHaveBeenCalledTimes(1);
  },
};

export const Pending: Story = {
  args: { pending: true },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByRole("button", { name: "Saving…" })).toBeDisabled();
  },
};

/** Nothing to save: Save reads as unavailable and a press is ignored, but it can still be pressed (#3449). */
export const NothingToSave: Story = {
  args: { disabled: true, status: "3 fields changed" },
  render: (args) => (
    <SaveBar {...args}>
      <Button variant="outline" disabled>
        Discard
      </Button>
    </SaveBar>
  ),
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    const save = canvas.getByRole("button", { name: "Save changes" });
    await expect(save).toHaveAttribute("aria-disabled", "true");
    await expect(save).toBeEnabled();
    await userEvent.click(save);
    await expect(args.onSave).not.toHaveBeenCalled();
    await expect(canvas.getByRole("status")).toHaveTextContent("3 fields changed");
  },
};

/**
 * The press that blurs a field commits it, which makes the form dirty before
 * the click lands on Save (#3449): a pristine Save that swallowed the press
 * would need a second click.
 */
export const PressThatCommitsAFieldSubmits: Story = {
  render: (args) => {
    const [value, setValue] = React.useState("");
    const [draft, setDraft] = React.useState("");
    return (
      <>
        <input
          aria-label="Name"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => setValue(draft)}
        />
        <SaveBar {...args} disabled={value === ""} />
      </>
    );
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    await userEvent.type(canvas.getByLabelText("Name"), "x");
    await userEvent.click(canvas.getByRole("button", { name: "Save changes" }));
    await expect(args.onSave).toHaveBeenCalledTimes(1);
  },
};
