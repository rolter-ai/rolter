import type { Meta, StoryObj } from "@storybook/react";

import { Donut } from "./donut";

const meta = {
  title: "Charts/Donut",
  component: Donut,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Donut>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    size: 160,
    segments: [
      { label: "OpenAI", value: 62 },
      { label: "Anthropic", value: 28 },
      { label: "Ollama", value: 10 },
    ],
  },
};

export const Single: Story = {
  args: { size: 160, segments: [{ label: "OpenAI", value: 100 }] },
};
