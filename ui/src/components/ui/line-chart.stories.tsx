import type { Meta, StoryObj } from "@storybook/react";

import { LineChart } from "./line-chart";

const meta = {
  title: "Charts/LineChart",
  component: LineChart,
  parameters: { layout: "padded" },
} satisfies Meta<typeof LineChart>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    height: 200,
    labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00"],
    series: [
      { name: "p50", values: [120, 132, 128, 140, 135, 150] },
      { name: "p95", values: [280, 320, 300, 360, 342, 380] },
    ],
    formatValue: (v: number) => `${v} ms`,
  },
};
