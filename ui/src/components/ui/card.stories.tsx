import type { Meta, StoryObj } from "@storybook/react";

import { Badge } from "./badge";
import { Card, CardContent, CardDescription, CardHeader, CardStack, CardTitle } from "./card";

const meta = {
  title: "Display/Card",
  component: Card,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Card>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <Card className="max-w-md">
      <CardHeader>
        <CardTitle>gpt-4o</CardTitle>
        <CardDescription>round_robin across 3 targets</CardDescription>
      </CardHeader>
      <CardContent className="flex items-center gap-2">
        <Badge tone="success" dot>
          healthy
        </Badge>
        <span className="text-sm text-muted-foreground">342 ms p95</span>
      </CardContent>
    </Card>
  ),
};

export const Stack: Story = {
  render: () => (
    <CardStack className="max-w-md">
      <span className="text-sm font-medium">openai / gpt-4o</span>
      <span className="text-sm text-muted-foreground">input 2.50, output 10.00 per 1M tokens</span>
    </CardStack>
  ),
};
