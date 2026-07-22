import type { Meta, StoryObj } from "@storybook/react";

import { BarChart } from "./bar-chart";

const meta = {
  title: "Charts/BarChart",
  component: BarChart,
  parameters: { layout: "padded" },
} satisfies Meta<typeof BarChart>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    height: 180,
    data: [420, 380, 512, 640, 588, 700, 660],
    labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    unit: "req",
  },
};

export const TopN: Story = {
  args: {
    height: 180,
    data: [900, 640, 512, 420, 380, 210, 120, 90],
    labels: ["gpt-4o", "claude", "llama", "mistral", "gemini", "groq", "xai", "ollama"],
    topN: 5,
  },
};
