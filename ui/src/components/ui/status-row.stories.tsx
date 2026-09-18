import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { StatusRow, type StatusKind } from "./status-row";

const KINDS: StatusKind[] = ["pending", "running", "success", "error", "warning", "info", "idle"];

const meta = {
  title: "Feedback/StatusRow",
  component: StatusRow,
  parameters: { layout: "padded" },
  args: { label: "Streaming response" },
} satisfies Meta<typeof StatusRow>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = { args: { status: "running" } };

/** Every kind together: the diamond carries the state, the label the subject. */
export const EveryKind: Story = {
  render: () => (
    <div className="max-w-sm space-y-1">
      {KINDS.map((status) => (
        <StatusRow key={status} status={status} label={status} />
      ))}
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    for (const status of KINDS) {
      await expect(canvas.getByText(status)).toBeVisible();
    }
  },
};

/** `colorText` tints the label too, for a row that stands alone. */
export const ColoredLabel: Story = {
  args: { status: "error", colorText: true, label: "Upstream refused the credential" },
};

/** Without a chevron the row reads as a report rather than a destination. */
export const WithoutChevron: Story = {
  args: { status: "success", chevron: false, label: "Snapshot applied" },
};

/**
 * A row with an `onClick` renders as a real `<button>`, not a div with a
 * handler: it has to be reachable from the keyboard like anything else that
 * does something when pressed.
 */
export const Interactive: Story = {
  render: () => {
    const [opened, setOpened] = React.useState(0);
    return (
      <div className="max-w-sm space-y-1 text-sm">
        <StatusRow
          status="pending"
          label="3 requests queued"
          onClick={() => setOpened((n) => n + 1)}
        />
        <p>Opened {opened} times</p>
      </div>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const row = canvas.getByRole("button", { name: /3 requests queued/ });
    await userEvent.click(row);
    await expect(canvas.getByText("Opened 1 times")).toBeVisible();
    // the keyboard path: a button activates on Enter, a div would not
    row.focus();
    await userEvent.keyboard("{Enter}");
    await expect(canvas.getByText("Opened 2 times")).toBeVisible();
  },
};
