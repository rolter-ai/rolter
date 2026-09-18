import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { SelfServiceUnavailable } from "./SelfServiceUnavailable";

const meta = {
  title: "Components/SelfServiceUnavailable",
  component: SelfServiceUnavailable,
} satisfies Meta<typeof SelfServiceUnavailable>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * What the API Keys screen shows instead of "Failed to load your keys." when
 * the control plane has no accounts to mint a key for.
 */
export const Default: Story = {
  play: async ({ canvas }) => {
    // the remedy is the point: an operator who reads this should know what to
    // change, not just that something failed
    await expect(await canvas.findByText(/ROLTER_ADMIN_TOKEN/)).toBeInTheDocument();
  },
};
