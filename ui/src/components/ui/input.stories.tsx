import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Field } from "./field";
import { Input } from "./input";

const meta = {
  title: "Primitives/Input",
  component: Input,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Input>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { placeholder: "gpt-4o", "aria-label": "Model" },
};

export const Disabled: Story = {
  args: { value: "locked", disabled: true, "aria-label": "Model" },
};

// a Field-wrapped input that validates non-empty on change, to exercise the
// label + error affordance
function ValidatedField() {
  const [value, setValue] = React.useState("gpt-4o");
  const error = value.trim() === "" ? "Model name is required" : undefined;
  return (
    <Field label="Model name" htmlFor="model" error={error} hint="the public model id">
      <Input
        id="model"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="gpt-4o"
      />
    </Field>
  );
}

export const WithField: Story = { render: () => <ValidatedField /> };

// interaction: typing updates the value; clearing it surfaces the error and
// hides the hint
export const TypingAndValidation: Story = {
  render: () => <ValidatedField />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByLabelText("Model name");
    await expect(input).toHaveValue("gpt-4o");
    await expect(canvas.getByText("the public model id")).toBeInTheDocument();

    await userEvent.clear(input);
    await expect(input).toHaveValue("");
    await expect(canvas.getByText("Model name is required")).toBeInTheDocument();
    await expect(canvas.queryByText("the public model id")).toBeNull();

    await userEvent.type(input, "claude-sonnet");
    await expect(input).toHaveValue("claude-sonnet");
    await expect(canvas.queryByText("Model name is required")).toBeNull();
  },
};
