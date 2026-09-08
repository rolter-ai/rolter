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

// a tab may name the panel it controls; the relationship is optional so call
// sites that render no panel stay unchanged
function WithPanel() {
  const [value, setValue] = React.useState("overview");
  const tabs = TABS.map((t) => ({ ...t, id: `tab-${t.value}`, panelId: `panel-${t.value}` }));
  return (
    <div className="space-y-3">
      <Tabs tabs={tabs} value={value} onChange={setValue} aria-label="Sections" />
      <div
        role="tabpanel"
        id={`panel-${value}`}
        aria-labelledby={`tab-${value}`}
        tabIndex={0}
        className="text-sm text-muted-foreground"
      >
        Panel for <span className="font-mono text-foreground">{value}</span>
      </div>
    </div>
  );
}

export const LinkedToPanel: Story = {
  render: () => <WithPanel />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const overview = canvas.getByRole("tab", { name: "Overview" });
    await expect(overview).toHaveAttribute("aria-controls", "panel-overview");
    await expect(canvas.getByRole("tabpanel", { name: "Overview" })).toBeInTheDocument();
  },
};

// keyboard: the strip is one tab stop and the arrows walk it, wrapping at the
// ends, with home/end jumping to the edges (#1273)
export const WalksWithArrowKeys: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const [overview, invocations, keys] = canvas.getAllByRole("tab");

    // roving tabindex: only the selected tab is reachable by Tab
    await expect(overview).toHaveAttribute("tabindex", "0");
    await expect(invocations).toHaveAttribute("tabindex", "-1");
    await expect(keys).toHaveAttribute("tabindex", "-1");

    overview.focus();
    await userEvent.keyboard("{ArrowRight}");
    await expect(invocations).toHaveFocus();
    await expect(invocations).toHaveAttribute("aria-selected", "true");
    await expect(overview).toHaveAttribute("tabindex", "-1");

    // wrapping: right off the last tab lands on the first, left off the first
    // lands on the last
    await userEvent.keyboard("{ArrowRight}{ArrowRight}");
    await expect(overview).toHaveFocus();
    await userEvent.keyboard("{ArrowLeft}");
    await expect(keys).toHaveFocus();
    await expect(keys).toHaveAttribute("aria-selected", "true");

    await userEvent.keyboard("{Home}");
    await expect(overview).toHaveFocus();
    await expect(overview).toHaveAttribute("aria-selected", "true");

    await userEvent.keyboard("{End}");
    await expect(keys).toHaveFocus();
    await expect(canvas.getByText("keys")).toBeInTheDocument();
  },
};
