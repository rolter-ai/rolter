import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { Tabs } from "./tabs";

const meta = {
  title: "Navigation/Tabs",
  component: Tabs,
  parameters: { layout: "padded" },
  // the render-only stories drive their own state via a wrapper; these satisfy
  // the required-prop type without being used
  args: { tabs: [], value: "" },
} satisfies Meta<typeof Tabs>;

export default meta;
type Story = StoryObj<typeof meta>;

const TABS = [
  { value: "overview", label: "Overview" },
  { value: "invocations", label: "Invocations", count: 128 },
  { value: "keys", label: "Keys", count: 4 },
];

function Controlled() {
  const [value, setValue] = React.useState("overview");
  return (
    <div className="space-y-3">
      <Tabs tabs={TABS} value={value} onChange={setValue} />
      <p className="text-sm text-muted-foreground">
        Active tab: <span className="font-mono text-foreground">{value}</span>
      </p>
    </div>
  );
}

export const Default: Story = { render: () => <Controlled /> };

// interaction: clicking a tab moves aria-selected and updates the panel
export const SwitchesTab: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const overview = canvas.getByRole("tab", { name: "Overview" });
    await expect(overview).toHaveAttribute("aria-selected", "true");

    const keys = canvas.getByRole("tab", { name: /Keys/ });
    await userEvent.click(keys);
    await expect(keys).toHaveAttribute("aria-selected", "true");
    await expect(overview).toHaveAttribute("aria-selected", "false");
    await expect(canvas.getByText("keys")).toBeInTheDocument();
  },
};
