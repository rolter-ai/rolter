import type { Meta, StoryObj } from "@storybook/react";

import { Skeleton } from "./skeleton";

const meta = {
  title: "Feedback/Skeleton",
  component: Skeleton,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Skeleton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = { args: { width: 200 } };

export const LoadingCard: Story = {
  render: () => (
    <div className="max-w-sm space-y-3 rounded-lg border border-[color:var(--border-default)] p-4">
      <Skeleton width={120} height={16} />
      <Skeleton width="100%" height={40} radius={8} />
      <div className="flex gap-2">
        <Skeleton width={64} height={22} radius={999} />
        <Skeleton width={64} height={22} radius={999} />
      </div>
    </div>
  ),
};
