import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { UnservedConfigNotice } from "./UnservedConfigNotice";

const meta = {
  title: "Components/UnservedConfigNotice",
  component: UnservedConfigNotice,
} satisfies Meta<typeof UnservedConfigNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The #926 case: one provider dropped from an otherwise healthy fleet. */
export const OneProblem: Story = {
  args: {
    problems: [
      "provider 'openrouter-edge' omitted from the snapshot: openrouter provider 'openrouter-edge' api_base must be https://openrouter.ai/api/v1 unless the provider sets allow_custom_api_base = true",
    ],
  },
  play: async ({ canvas }) => {
    // singular, and the reason is visible — "1 provider is broken" without
    // saying why sends the operator back to the logs
    await expect(await canvas.findByText(/1 config entry/)).toBeInTheDocument();
    await expect(await canvas.findByText(/openrouter\.ai\/api\/v1/)).toBeInTheDocument();
  },
};

export const SeveralProblems: Story = {
  args: {
    problems: [
      "provider 'openrouter-edge' omitted from the snapshot: openrouter provider 'openrouter-edge' api_base must be https://openrouter.ai/api/v1 unless the provider sets allow_custom_api_base = true",
      "route 'gpt-4o-mini' omitted from the snapshot: it has no target that references a known provider with a positive weight",
    ],
  },
  play: async ({ canvas }) => {
    await expect(await canvas.findByText(/2 config entries/)).toBeInTheDocument();
  },
};

/** The healthy fleet: the notice must take up no space at all. */
export const Healthy: Story = {
  args: { problems: [] },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("list")).toBeNull();
  },
};
