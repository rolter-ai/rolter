import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Checkbox } from "./checkbox";

const meta = {
  title: "Forms/Checkbox",
  component: Checkbox,
  parameters: { layout: "padded" },
  args: { "aria-label": "Option" },
} satisfies Meta<typeof Checkbox>;

export default meta;
type Story = StoryObj<typeof meta>;

function Labelled({
  initial = false,
  disabled = false,
  indeterminate = false,
}: {
  initial?: boolean;
  disabled?: boolean;
  indeterminate?: boolean;
}) {
  const [checked, setChecked] = React.useState(initial);
  return (
    <label className="flex items-center gap-2 text-sm">
      <Checkbox
        checked={checked}
        disabled={disabled}
        indeterminate={indeterminate}
        onChange={(e) => setChecked(e.target.checked)}
      />
      Allow the team
    </label>
  );
}

export const Unchecked: Story = { render: () => <Labelled /> };
export const Checked: Story = { render: () => <Labelled initial /> };
export const Disabled: Story = { render: () => <Labelled initial disabled /> };
export const Indeterminate: Story = {
  render: () => <Labelled indeterminate />,
  play: async ({ canvasElement }) => {
    const box = within(canvasElement).getByRole("checkbox", { name: "Allow the team" });
    await expect(box).toBePartiallyChecked();
  },
};

// the wrapping label names it and a click on the text toggles it; Space toggles
// it from the keyboard, and the box takes focus on the way
export const LabelAndKeyboard: Story = {
  render: () => <Labelled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const box = canvas.getByRole("checkbox", { name: "Allow the team" });
    await expect(box).not.toBeChecked();
    await userEvent.click(canvas.getByText("Allow the team"));
    await expect(box).toBeChecked();
    await userEvent.tab({ shift: true });
    await userEvent.tab();
    await expect(box).toHaveFocus();
    await userEvent.keyboard(" ");
    await expect(box).not.toBeChecked();
  },
};

// a disabled box ignores the pointer and the keyboard
export const DisabledStaysPut: Story = {
  render: () => <Labelled initial disabled />,
  play: async ({ canvasElement }) => {
    const box = within(canvasElement).getByRole("checkbox", { name: "Allow the team" });
    await userEvent.click(box);
    await expect(box).toBeChecked();
    await expect(box).toBeDisabled();
  },
};
