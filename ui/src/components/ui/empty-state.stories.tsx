import type { Meta, StoryObj } from "@storybook/react";
import { Inbox } from "lucide-react";

import { Button } from "./button";
import { EmptyState } from "./empty-state";

const meta = {
  title: "Feedback/EmptyState",
  component: EmptyState,
  parameters: { layout: "padded" },
} satisfies Meta<typeof EmptyState>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    icon: <Inbox />,
    title: "No routes yet",
    description: "Create a route to start forwarding traffic to your providers.",
  },
};

export const WithAction: Story = {
  args: {
    icon: <Inbox />,
    title: "No invocations in range",
    description: "Try widening the time window or clearing filters.",
    actions: <Button variant="outline">Clear filters</Button>,
  },
};
