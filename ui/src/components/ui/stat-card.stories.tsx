import type { Meta, StoryObj } from "@storybook/react";

import { StatCard } from "./stat-card";

const meta = {
  title: "Display/StatCard",
  component: StatCard,
  parameters: { layout: "padded" },
  args: { label: "Requests / min", value: "1,284" },
} satisfies Meta<typeof StatCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Grid: Story = {
  render: () => (
    <div className="grid max-w-3xl grid-cols-3 gap-3">
      <StatCard label="Requests / min" value="1,284" delta="+12%" trend="up" />
      <StatCard label="p95 latency" value="342" unit="ms" delta="-8%" trend="down" />
      <StatCard label="Error rate" value="0.4" unit="%" delta="0%" trend="flat" />
    </div>
  ),
};
