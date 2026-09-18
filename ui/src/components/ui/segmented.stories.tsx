import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { FieldLabel } from "./field-label";
import { Segmented } from "./segmented";

const MODES = [
  { value: "lockAll", label: "Lock all" },
  { value: "unlockAll", label: "Unlock all" },
  { value: "manual", label: "Manual" },
];

const meta = {
  title: "Forms/Segmented",
  component: Segmented,
  parameters: { layout: "padded" },
  args: {
    value: "lockAll",
    options: MODES,
    onChange: () => {},
    ariaLabel: "Parameter lock mode",
  },
} satisfies Meta<typeof Segmented<string>>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  disabled = false,
  labelled = false,
}: {
  disabled?: boolean;
  labelled?: boolean;
}) {
  const [value, setValue] = React.useState("lockAll");
  return (
    <div className="space-y-1.5">
      {labelled && <FieldLabel label="Parameter lock mode" id="sg-label" />}
      <Segmented
        value={value}
        options={MODES}
        onChange={setValue}
        disabled={disabled}
        labelledBy={labelled ? "sg-label" : undefined}
        ariaLabel={labelled ? undefined : "Parameter lock mode"}
      />
      <p className="text-xs text-muted-foreground">mode: {value}</p>
    </div>
  );
}

export const Default: Story = { render: () => <Controlled /> };
export const WithVisibleLabel: Story = { render: () => <Controlled labelled /> };
export const Disabled: Story = { render: () => <Controlled disabled /> };

/**
 * The group is a `radiogroup` of `radio`s, not three buttons that happen to be
 * adjacent: exactly one is checked, and that is what a screen reader reports.
 */
export const ReportsExactlyOneChecked: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const group = canvas.getByRole("radiogroup", { name: "Parameter lock mode" });
    const options = within(group).getAllByRole("radio");
    await expect(options).toHaveLength(3);
    // bites if `aria-checked` stops tracking `value`: the group would report
    // no selection, or every option as selected
    await expect(options.filter((o) => o.getAttribute("aria-checked") === "true")).toHaveLength(1);
    await expect(options[0]).toHaveAttribute("aria-checked", "true");
  },
};

/** Picking an option moves the selection and reports the new value up. */
export const SelectsOnClick: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("radio", { name: "Manual" }));
    await expect(canvas.getByRole("radio", { name: "Manual" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(canvas.getByRole("radio", { name: "Lock all" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    await expect(canvas.getByText("mode: manual")).toBeVisible();
  },
};

/**
 * Every option is a real, tabbable `<button>`: `Tab` walks into the group and
 * `Enter` picks an option — a div with an `onClick` would do neither.
 */
export const SelectsFromTheKeyboard: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.tab();
    await expect(canvas.getByRole("radio", { name: "Lock all" })).toHaveFocus();
    await userEvent.tab();
    await expect(canvas.getByRole("radio", { name: "Unlock all" })).toHaveFocus();
    await userEvent.keyboard("{Enter}");
    await expect(canvas.getByText("mode: unlockAll")).toBeVisible();
  },
};

/** Disabled means the whole choice is frozen — a view-only sheet, say. */
export const DisabledIgnoresClicks: Story = {
  render: () => <Controlled disabled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const manual = canvas.getByRole("radio", { name: "Manual" });
    await expect(manual).toBeDisabled();
    await userEvent.click(manual, { pointerEventsCheck: 0 });
    await expect(canvas.getByText("mode: lockAll")).toBeVisible();
  },
};
