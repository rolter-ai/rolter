import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, fn, userEvent, waitFor, within } from "storybook/test";

import { TimeWindowPicker } from "./TimeWindowPicker";
import { Field } from "@/components/ui/field";
import { setChartTimeZone } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
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

/**
 * Draws its children with the account's chart zone set to `zone`, and puts the
 * browser's back when the story unmounts.
 */
function InChartZone({ zone, children }: { zone: string; children: React.ReactNode }) {
  React.useState(() => {
    setChartTimeZone(zone);
    return null;
  });
  React.useEffect(() => () => setChartTimeZone(null), []);
  return <>{children}</>;
}

// fourteen hours ahead of UTC: never the zone a browser running the stories is in
const FAR_ZONE = "Pacific/Kiritimati";

/**
 * #3500: a calendar day is a day in the chart zone. Where the account draws its
 * charts in a zone that is not the browser's, the picker says which zone its
 * days are counted in, as the list's description, and a custom range starts
 * from that zone's today.
 */
export const DaysAreCountedInTheChartZoneAndThePickerSaysSo: Story = {
  render: () => (
    <InChartZone zone={FAR_ZONE}>
      <Held start="24h" />
    </InChartZone>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = en.common.timeWindow.countedIn.replace("{{zone}}", FAR_ZONE);
    await expect(canvas.getByText(note)).toBeVisible();
    await expect(canvas.getByRole("combobox", { name: "Time window" })).toHaveAccessibleDescription(
      note,
    );
    await pick(canvas, "Custom range…");
    // the last day is today where the charts are drawn, which can already be
    // tomorrow in the browser
    await waitFor(() =>
      expect(canvas.getByLabelText("To")).toHaveValue(formatDay(new Date(), FAR_ZONE)),
    );
    const picked = canvas.getByTestId("picked").textContent as WindowSelection;
    const since = new Date(windowBounds(picked).since);
    // the range opens at a midnight in that zone
    await expect(
      new Intl.DateTimeFormat("en-GB", {
        timeZone: FAR_ZONE,
        hour: "2-digit",
        minute: "2-digit",
        hourCycle: "h23",
      }).format(since),
    ).toBe("00:00");
  },
};

/** With no chart zone, or the browser's own, there is nothing to say. */
export const TheBrowsersOwnZoneIsNotNamed: Story = {
  render: () => (
    <InChartZone zone={Intl.DateTimeFormat().resolvedOptions().timeZone}>
      <Held start="24h" />
    </InChartZone>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const combobox = canvas.getByRole("combobox", { name: "Time window" });
    await expect(combobox).toBeVisible();
    await expect(combobox).not.toHaveAttribute("aria-describedby");
  },
};

/**
 * #3507: inside a `Field`, the picker's list is described by the field's hint,
 * and by its error once there is one, which also marks the list invalid. The
 * clone used to forward the label alone, so a screen reader heard neither.
 */
export const InAFieldItsHintAndErrorDescribeTheList: Story = {
  render: () => (
    <div className="flex w-64 flex-col gap-6">
      <Field label="Report window" hint="The figures cover these days.">
        <TimeWindowPicker value="7d" onChange={() => {}} />
      </Field>
      <Field label="Comparison window" hint="The figures cover these days." error="Pick a window.">
        <TimeWindowPicker value="30d" onChange={() => {}} />
      </Field>
      <Field label="Required window" required>
        <TimeWindowPicker value="24h" onChange={() => {}} />
      </Field>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const hinted = canvas.getByRole("combobox", { name: "Report window" });
    await expect(hinted).toHaveAccessibleDescription("The figures cover these days.");
    await expect(hinted).not.toHaveAttribute("aria-invalid");

    const failed = canvas.getByRole("combobox", { name: "Comparison window" });
    await expect(failed).toHaveAccessibleDescription("Pick a window.");
    await expect(failed).toHaveAttribute("aria-invalid", "true");

    await expect(canvas.getByRole("combobox", { name: /Required window/ })).toHaveAttribute(
      "aria-required",
      "true",
    );
  },
};

/** a field's description and the chart zone's note both describe the list */
export const InAFieldInAnotherChartZoneBothDescribeTheList: Story = {
  render: () => (
    <InChartZone zone={FAR_ZONE}>
      <div className="w-64">
        <Field label="Report window" hint="The figures cover these days.">
          <TimeWindowPicker value="7d" onChange={() => {}} />
        </Field>
      </div>
    </InChartZone>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const note = en.common.timeWindow.countedIn.replace("{{zone}}", FAR_ZONE);
    await expect(
      canvas.getByRole("combobox", { name: "Report window" }),
    ).toHaveAccessibleDescription(`The figures cover these days. ${note}`);
  },
};
