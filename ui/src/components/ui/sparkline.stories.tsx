import type { Meta, StoryObj } from "@storybook/react";

import { Sparkline } from "./sparkline";

const meta = {
  title: "Charts/Sparkline",
  component: Sparkline,
  parameters: { layout: "padded" },
  args: { values: [] },
} satisfies Meta<typeof Sparkline>;

export default meta;
type Story = StoryObj<typeof meta>;

const SERIES = [4, 8, 5, 9, 7, 12, 10, 14, 11, 15, 13, 18];

export const Default: Story = { args: { values: SERIES, width: 160, height: 40 } };

export const InlineWithStat: Story = {
  render: () => (
    <div className="flex items-center gap-3">
      <span className="font-mono text-2xl">1,284</span>
      <Sparkline values={SERIES} width={120} height={32} />
    </div>
  ),
};
