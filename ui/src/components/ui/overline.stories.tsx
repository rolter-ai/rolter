import type { Meta, StoryObj } from "@storybook/react";

import { Overline } from "./overline";

const meta = {
  title: "Display/Overline",
  component: Overline,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Overline>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <div>
      <Overline>Requests</Overline>
      <span className="text-lg font-semibold">12,408</span>
    </div>
  ),
};

export const InDescriptionList: Story = {
  render: () => (
    <dl>
      <Overline as="dt">Region</Overline>
      <dd className="text-sm">eu-west-1</dd>
    </dl>
  ),
};
