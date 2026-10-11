import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { StrategyBadge } from "./StrategyBadge";
import { STRATEGIES } from "@/lib/api";

const meta = {
  title: "Components/StrategyBadge",
  component: StrategyBadge,
  parameters: { layout: "padded" },
  args: { strategy: "round_robin" },
} satisfies Meta<typeof StrategyBadge>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const badge = within(canvasElement).getByText("round_robin").closest("span.font-mono");
    await expect(badge).toBeTruthy();
    // verbatim: never upper-cased by the badge
    await expect(getComputedStyle(badge as Element).textTransform).toBe("none");
  },
};

export const EveryStrategy: Story = {
  render: () => (
    <div className="flex flex-wrap gap-2">
      {STRATEGIES.map((s) => (
        <StrategyBadge key={s} strategy={s} />
      ))}
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const s of STRATEGIES) await expect(canvas.getByText(s)).toBeInTheDocument();
  },
};
