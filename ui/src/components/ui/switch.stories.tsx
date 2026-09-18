import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Switch } from "./switch";

const meta = {
  title: "Primitives/Switch",
  component: Switch,
  parameters: { layout: "padded" },
  // the render-only stories drive their own state via a wrapper; these satisfy
  // the required-prop type without being used
  args: { checked: false, "aria-label": "Feature toggle" },
} satisfies Meta<typeof Switch>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  initial = false,
  disabled = false,
}: {
  initial?: boolean;
  disabled?: boolean;
}) {
  const [checked, setChecked] = React.useState(initial);
  // a wrapping <label> names an input, never a role="switch" button, so the
  // caption beside the track is wired up explicitly — the pattern every call
  // site in the dashboard follows (#1181)
  const labelId = React.useId();
  return (
    <label className="flex items-center gap-2 text-sm">
      <Switch
        checked={checked}
        onCheckedChange={setChecked}
        disabled={disabled}
        aria-labelledby={labelId}
      />
      <span id={labelId}>{checked ? "Enabled" : "Disabled"}</span>
    </label>
  );
}

export const Off: Story = { render: () => <Controlled /> };
export const On: Story = { render: () => <Controlled initial /> };
export const DisabledOn: Story = { render: () => <Controlled initial disabled /> };

// a switch always carries a name: the primitive's type requires aria-label or
// aria-labelledby, and axe fails the story if the rendered button has neither
export const IsNamed: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("switch", { name: "Disabled" })).toBeInTheDocument();
  },
};

// interaction: toggling flips aria-checked and the label text
export const TogglesOnClick: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const sw = canvas.getByRole("switch");
    await expect(sw).toHaveAttribute("aria-checked", "false");
    await userEvent.click(sw);
    await expect(sw).toHaveAttribute("aria-checked", "true");
    await expect(canvas.getByText("Enabled")).toBeInTheDocument();
    await userEvent.click(sw);
    await expect(sw).toHaveAttribute("aria-checked", "false");
  },
};
