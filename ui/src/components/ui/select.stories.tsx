import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Field } from "./field";
import { Select } from "./select";

const meta = {
  title: "Primitives/Select",
  component: Select,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Select>;

export default meta;
type Story = StoryObj<typeof meta>;

const STRATEGIES = ["round_robin", "random", "power_of_two", "cache_aware", "weighted"];

export const Default: Story = {
  render: () => (
    <Select aria-label="Strategy" defaultValue="round_robin">
      {STRATEGIES.map((s) => (
        <option key={s} value={s}>
          {s}
        </option>
      ))}
    </Select>
  ),
};

function ControlledSelect() {
  const [value, setValue] = React.useState("round_robin");
  return (
    <Field label="Strategy" htmlFor="strategy" hint={`current: ${value}`}>
      <Select
        id="strategy"
        value={value}
        onChange={(e) => setValue(e.target.value)}
      >
        {STRATEGIES.map((s) => (
          <option key={s} value={s}>
            {s}
          </option>
        ))}
      </Select>
    </Field>
  );
}

export const WithField: Story = { render: () => <ControlledSelect /> };

// interaction: choosing an option updates the controlled value
export const SelectsOption: Story = {
  render: () => <ControlledSelect />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const select = canvas.getByLabelText("Strategy");
    await userEvent.selectOptions(select, "cache_aware");
    await expect(select).toHaveValue("cache_aware");
    await expect(canvas.getByText("current: cache_aware")).toBeInTheDocument();
  },
};
