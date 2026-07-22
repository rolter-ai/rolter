import type { Meta, StoryObj } from "@storybook/react";

import { Badge } from "./badge";

const meta = {
  title: "Primitives/Badge",
  component: Badge,
  parameters: { layout: "padded" },
  args: { children: "neutral" },
  argTypes: {
    tone: {
      control: "select",
      options: ["neutral", "outline", "success", "warning", "danger", "info", "accent"],
    },
    dot: { control: "boolean" },
  },
} satisfies Meta<typeof Badge>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const AllTones: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      <Badge tone="neutral">neutral</Badge>
      <Badge tone="outline">outline</Badge>
      <Badge tone="success" dot>
        healthy
      </Badge>
      <Badge tone="warning" dot>
        degraded
      </Badge>
      <Badge tone="danger" dot>
        down
      </Badge>
      <Badge tone="info">info</Badge>
      <Badge tone="accent">accent</Badge>
    </div>
  ),
};
