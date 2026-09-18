import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { SwitchRow } from "./switch-row";

const meta = {
  title: "Forms/SwitchRow",
  component: SwitchRow,
  parameters: { layout: "padded" },
  args: { title: "Enabled", checked: true, onChange: () => {} },
} satisfies Meta<typeof SwitchRow>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  initial = false,
  hint,
  info,
  disabled = false,
}: {
  initial?: boolean;
  hint?: string;
  info?: string;
  disabled?: boolean;
}) {
  const [checked, setChecked] = React.useState(initial);
  return (
    <div className="max-w-md space-y-2">
      <SwitchRow
        title="Streaming"
        hint={hint}
        info={info}
        checked={checked}
        onChange={setChecked}
        disabled={disabled}
      />
      <p className="text-xs text-muted-foreground">streaming: {String(checked)}</p>
    </div>
  );
}

export const Off: Story = { render: () => <Controlled /> };
export const On: Story = { render: () => <Controlled initial /> };
export const WithHint: Story = {
  render: () => <Controlled initial hint="Token-by-token responses over SSE." />,
};
export const Disabled: Story = { render: () => <Controlled initial disabled /> };

/**
 * The title names the switch. Without that wiring the row reads fine on screen
 * and the control is announced as an unnamed toggle.
 */
export const SwitchIsNamedByTheTitle: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // bites if the aria-label drops off the Switch: the query finds nothing
    const control = canvas.getByRole("switch", { name: "Streaming" });
    await expect(control).toHaveAttribute("aria-checked", "false");
  },
};

/** Clicking the switch reports the new value up; the hint is not a control. */
export const TogglesOnClick: Story = {
  render: () => <Controlled hint="Token-by-token responses over SSE." />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const control = canvas.getByRole("switch", { name: "Streaming" });
    await userEvent.click(control);
    await expect(control).toHaveAttribute("aria-checked", "true");
    await expect(canvas.getByText("streaming: true")).toBeVisible();
    await expect(canvas.getByText("Token-by-token responses over SSE.")).toBeVisible();
  },
};

/** `info` adds an (i) named after the row, beside the title rather than in it. */
export const WithInfo: Story = {
  render: () => <Controlled info="Only routes whose upstream supports SSE can stream." />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const hint = canvas.getByRole("button", { name: "About Streaming" });
    await userEvent.hover(hint);
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(/supports SSE/);
    // revealing the note is not flipping the switch
    await expect(canvas.getByRole("switch", { name: "Streaming" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
  },
};

/** A read-only sheet renders the row inert but still readable. */
export const DisabledIgnoresClicks: Story = {
  render: () => <Controlled initial disabled />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const control = canvas.getByRole("switch", { name: "Streaming" });
    await expect(control).toBeDisabled();
    await userEvent.click(control, { pointerEventsCheck: 0 });
    await expect(canvas.getByText("streaming: true")).toBeVisible();
  },
};
