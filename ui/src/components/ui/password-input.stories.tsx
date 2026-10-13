import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import { Field } from "./field";
import { PasswordInput } from "./password-input";

const meta = {
  title: "Forms/PasswordInput",
  component: PasswordInput,
  parameters: { layout: "padded" },
  args: { name: "password", autoComplete: "current-password" },
  render: (args) => (
    <div className="max-w-sm">
      <Field label="Password" error={args["aria-invalid"] ? "Too short." : undefined}>
        <PasswordInput {...args} />
      </Field>
    </div>
  ),
} satisfies Meta<typeof PasswordInput>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Hidden by default; the toggle reveals what was typed and says which way it flips. */
export const TogglesVisibility: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByLabelText("Password");
    await userEvent.type(input, "correct-horse");
    await expect(input).toHaveAttribute("type", "password");
    await userEvent.click(canvas.getByRole("button", { name: "Show password" }));
    await expect(input).toHaveAttribute("type", "text");
    await expect(input).toHaveValue("correct-horse");
    await userEvent.click(canvas.getByRole("button", { name: "Hide password" }));
    await expect(input).toHaveAttribute("type", "password");
  },
};

/** `Field` reaches the input through the wrapper: label, error and invalid state land on it. */
export const FieldDescribesIt: Story = {
  args: { "aria-invalid": true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const input = canvas.getByLabelText("Password");
    await expect(input).toHaveAttribute("aria-invalid", "true");
    await expect(input).toHaveAccessibleDescription("Too short.");
  },
};
