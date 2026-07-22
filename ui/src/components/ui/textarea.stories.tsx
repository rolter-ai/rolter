import type { Meta, StoryObj } from "@storybook/react";

import { Field } from "./field";
import { Textarea } from "./textarea";

const meta = {
  title: "Primitives/Textarea",
  component: Textarea,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Textarea>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: { placeholder: "You are a helpful assistant…", rows: 4, "aria-label": "System prompt" },
};

export const WithField: Story = {
  render: () => (
    <Field label="System prompt" htmlFor="sys" hint="injected before every message">
      <Textarea id="sys" rows={4} defaultValue="You are a helpful assistant." />
    </Field>
  ),
};
