import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { IncompleteSpendNotice } from "./IncompleteSpendNotice";

const meta = {
  title: "Components/IncompleteSpendNotice",
  component: IncompleteSpendNotice,
} satisfies Meta<typeof IncompleteSpendNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The #969 measurement: 132 requests across 8 models, all unpriced. */
export const ManyUnpriced: Story = {
  args: { requests: 132, models: 8 },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent("132");
    // "floor, not a total" is the load-bearing phrase — it is what stops an
    // operator reading the number as final
    await expect(notice).toHaveTextContent(/floor, not a total/);
  },
};

export const OneUnpriced: Story = {
  args: { requests: 1, models: 1 },
  play: async ({ canvas }) => {
    // singular in both clauses
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      /1 request in this window is unpriced/,
    );
  },
};

/** Everything priced: the spend figure is a real total, so say nothing. */
export const FullyPriced: Story = {
  args: { requests: 0, models: 0 },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("status")).toBeNull();
  },
};
