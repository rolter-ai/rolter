import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { ChipGroup } from "./chip-group";

const TEAMS = [
  { id: "3f1b0a5e-0000-4000-8000-000000000001", name: "platform" },
  { id: "3f1b0a5e-0000-4000-8000-000000000002", name: "growth" },
  { id: "3f1b0a5e-0000-4000-8000-000000000003", name: "research" },
];

const meta = {
  title: "Forms/ChipGroup",
  component: ChipGroup,
  parameters: { layout: "padded" },
  args: { label: "Teams", options: TEAMS, selected: [], onToggle: () => {} },
} satisfies Meta<typeof ChipGroup>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  options = TEAMS,
  initial = [],
  disabled = false,
}: {
  options?: { id: string; name: string }[];
  initial?: string[];
  disabled?: boolean;
}) {
  const [selected, setSelected] = React.useState<string[]>(initial);
  return (
    <div className="max-w-sm space-y-2">
      <ChipGroup
        label="Teams"
        options={options}
        selected={selected}
        disabled={disabled}
        onToggle={(id) =>
          setSelected((s) => (s.includes(id) ? s.filter((v) => v !== id) : [...s, id]))
        }
      />
      <p className="text-xs text-muted-foreground">picked: {selected.length}</p>
    </div>
  );
}

export const Default: Story = { render: () => <Controlled /> };
export const WithSelection: Story = { render: () => <Controlled initial={[TEAMS[1].id]} /> };
export const Disabled: Story = { render: () => <Controlled initial={[TEAMS[0].id]} disabled /> };

/**
 * Nothing to pick from is a state, not a blank: an operator who sees an empty
 * row cannot tell it apart from one that failed to load.
 */
export const Empty: Story = {
  render: () => <Controlled options={[]} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // bites if the empty branch goes away: the group would render an empty row
    await expect(canvas.getByText("none available")).toBeVisible();
    await expect(canvas.queryAllByRole("button")).toHaveLength(0);
  },
};

/**
 * The chips are one named group, so a screen reader announces what the choice
 * is about before reading three bare team names.
 */
export const IsOneNamedGroup: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = canvas.getByRole("group", { name: "Teams" });
    await expect(within(group).getAllByRole("button")).toHaveLength(3);
  },
};

/** A chip is a toggle: `aria-pressed` carries whether it is in the selection. */
export const TogglesSelection: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const growth = canvas.getByRole("button", { name: "growth" });
    await expect(growth).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(growth);
    await expect(growth).toHaveAttribute("aria-pressed", "true");
    await expect(canvas.getByText("picked: 1")).toBeVisible();
    // selecting is not exclusive: a second chip adds rather than replaces
    await userEvent.click(canvas.getByRole("button", { name: "research" }));
    await expect(canvas.getByText("picked: 2")).toBeVisible();
    await userEvent.click(growth);
    await expect(growth).toHaveAttribute("aria-pressed", "false");
    await expect(canvas.getByText("picked: 1")).toBeVisible();
  },
};

/** Disabled freezes the selection without hiding what it is. */
export const DisabledIgnoresClicks: Story = {
  render: () => <Controlled initial={[TEAMS[0].id]} disabled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const growth = canvas.getByRole("button", { name: "growth" });
    await expect(growth).toBeDisabled();
    await userEvent.click(growth, { pointerEventsCheck: 0 });
    await expect(canvas.getByText("picked: 1")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "platform" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  },
};
