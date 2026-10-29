import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, fn, userEvent, waitFor, within } from "storybook/test";

import { TimeWindowPicker } from "./TimeWindowPicker";
import { formatDay, windowBounds, type WindowSelection } from "@/lib/time-window";

const meta = {
  title: "Display/TimeWindowPicker",
  component: TimeWindowPicker,
  parameters: { layout: "padded" },
  args: { value: "24h", onChange: () => {}, "aria-label": "Time window" },
} satisfies Meta<typeof TimeWindowPicker>;

export default meta;
type Story = StoryObj<typeof meta>;

/** keeps the picked selection, the way a screen keeps it in its address */
function Held({
  start,
  onChange,
}: {
  start: WindowSelection;
  onChange?: (next: WindowSelection) => void;
}) {
  const [value, setValue] = React.useState(start);
  return (
    <div className="w-56">
      <TimeWindowPicker
        aria-label="Time window"
        value={value}
        onChange={(next) => {
          setValue(next);
          onChange?.(next);
        }}
      />
      <output data-testid="picked">{value}</output>
    </div>
  );
}

const pick = async (canvas: ReturnType<typeof within>, name: string | RegExp) => {
  await userEvent.click(canvas.getByRole("combobox", { name: "Time window" }));
  await userEvent.click(await canvas.findByRole("option", { name }));
};

/** A named window shows no day fields: they belong to the custom range alone. */
export const NamedWindowsShowNoDays: Story = {
  render: () => <Held start="7d" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveValue("Last 7 days");
    await expect(canvas.queryByLabelText("From")).toBeNull();
    await expect(canvas.queryByLabelText("To")).toBeNull();
  },
};

/**
 * "Custom range…" is the list's last entry. Choosing it keeps the screen on the
 * days it was already showing: the last 7 days become the seven days ending
 * today, and the fields hold them.
 */
export const CustomRangeStartsFromTheWindowOnScreen: Story = {
  render: () => <Held start="7d" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("combobox", { name: "Time window" }));
    const options = await canvas.findAllByRole("option");
    await expect(options[options.length - 1]).toHaveTextContent("Custom range…");
    await userEvent.click(options[options.length - 1]!);
    const bounds = windowBounds("7d");
    await waitFor(() =>
      expect(canvas.getByLabelText("From")).toHaveValue(formatDay(new Date(bounds.since))),
    );
    await expect(canvas.getByLabelText("To")).toHaveValue(formatDay(new Date()));
    await expect(canvas.getByTestId("picked").textContent).toMatch(
      /^custom:\d{4}-\d{2}-\d{2}:\d{4}-\d{2}-\d{2}$/,
    );
    // and the fields are one group, named
    await expect(canvas.getByRole("group", { name: "Custom range" })).toBeVisible();
  },
};

/** Editing a day hands the screen the new range at once, when it is whole. */
export const EditingADayChangesTheRange: Story = {
  render: () => <Held start="custom:2026-10-01:2026-10-07" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("From")).toHaveValue("2026-10-01");
    await expect(canvas.getByLabelText("To")).toHaveValue("2026-10-07");
    const to = canvas.getByLabelText("To");
    await userEvent.clear(to);
    await userEvent.type(to, "2026-10-09");
    await waitFor(() =>
      expect(canvas.getByTestId("picked")).toHaveTextContent("custom:2026-10-01:2026-10-09"),
    );
  },
};

/**
 * An end before the start is refused where it is typed: the message is tied to
 * both fields, and the screen keeps the last range that was whole instead of
 * being asked for one that does not exist.
 */
export const AnEndBeforeTheStartIsRefused: Story = {
  render: () => <Held start="custom:2026-10-01:2026-10-07" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const to = canvas.getByLabelText("To");
    await userEvent.clear(to);
    await userEvent.type(to, "2026-09-20");
    const alert = await canvas.findByRole("alert");
    await expect(alert).toHaveTextContent("The end day is before the start day");
    await expect(to).toHaveAttribute("aria-invalid", "true");
    await expect(canvas.getByLabelText("From")).toHaveAttribute("aria-invalid", "true");
    await expect(to).toHaveAccessibleDescription(/before the start day/);
    // nothing was committed
    await expect(canvas.getByTestId("picked")).toHaveTextContent("custom:2026-10-01:2026-10-07");
    // fixing it clears the message and commits
    await userEvent.clear(to);
    await userEvent.type(to, "2026-10-03");
    await waitFor(() => expect(canvas.queryByRole("alert")).toBeNull());
    await expect(canvas.getByTestId("picked")).toHaveTextContent("custom:2026-10-01:2026-10-03");
  },
};

/** A day cleared out is asked for again, not read as a range. */
export const AClearedDayAsksForBoth: Story = {
  render: () => <Held start="custom:2026-10-01:2026-10-07" />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.clear(canvas.getByLabelText("From"));
    await expect(await canvas.findByRole("alert")).toHaveTextContent(
      "Pick a start day and an end day.",
    );
    await expect(canvas.getByTestId("picked")).toHaveTextContent("custom:2026-10-01:2026-10-07");
  },
};

/** Going back to a named window drops the day fields and reports the name. */
export const BackToANamedWindowDropsTheDays: Story = {
  args: { onChange: fn() },
  render: (args) => <Held start="custom:2026-10-01:2026-10-07" onChange={args.onChange} />,
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    await pick(canvas, "Last 30 days");
    await expect(args.onChange).toHaveBeenLastCalledWith("30d");
    await waitFor(() => expect(canvas.queryByLabelText("From")).toBeNull());
  },
};
