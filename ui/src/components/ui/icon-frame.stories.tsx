import type { Meta, StoryObj } from "@storybook/react";
import { Bell, Network } from "lucide-react";

import { IconFrame } from "./icon-frame";

const meta = {
  title: "Display/IconFrame",
  component: IconFrame,
  parameters: { layout: "padded" },
} satisfies Meta<typeof IconFrame>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <div className="flex items-center gap-3">
      <IconFrame>
        <Bell aria-hidden className="h-4 w-4" />
      </IconFrame>
      <span className="text-sm font-medium">Slack alerts</span>
    </div>
  ),
};

export const Tinted: Story = {
  render: () => (
    <IconFrame className="text-[color:var(--red-folk-text)]">
      <Network aria-hidden className="h-4 w-4" />
    </IconFrame>
  ),
};
