import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { TakesOverField } from "./TakesOverField";

function Controlled({ initial = [], error }: { initial?: string[]; error?: string }) {
  const [value, setValue] = React.useState<string[]>(initial);
  return (
    <ul className="grid max-w-[560px] grid-cols-1 p-4">
      <li className="grid">
        <TakesOverField
          groupLabel="Fallback classes of target 2"
          value={value}
          error={error}
          onChange={setValue}
        />
        <output data-testid="value">{JSON.stringify(value)}</output>
      </li>
    </ul>
  );
}

const meta = {
  title: "Components/TakesOverField",
  component: Controlled,
} satisfies Meta<typeof Controlled>;

export default meta;
type Story = StoryObj<typeof meta>;

/** An ordinary target folds its set behind one line saying what it is. */
export const Ordinary: Story = {
  play: async ({ canvas }) => {
    const toggle = canvas.getByRole("button", { name: "Ordinary target" });
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await expect(canvas.queryByTestId("fallback-badge")).toBeNull();
    await userEvent.click(toggle);
    await userEvent.click(canvas.getByRole("button", { name: "transport" }));
    await expect(canvas.getByTestId("value")).toHaveTextContent('["transport"]');
    await expect(canvas.getByTestId("fallback-badge")).toBeVisible();
  },
};

/** A fallback opens unfolded, marked, with its classes pressed. */
export const Restricted: Story = {
  args: { initial: ["throttled", "refusal:content_policy_violation"] },
  play: async ({ canvas }) => {
    const group = within(canvas.getByRole("group", { name: "Fallback classes of target 2" }));
    await expect(
      group.getByRole("button", { name: /^Fallback for throttled, refusal:/ }),
    ).toHaveAttribute("aria-expanded", "true");
    await expect(
      group.getByRole("button", { name: "refusal:content_policy_violation" }),
    ).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(group.getByRole("button", { name: "throttled" }));
    await expect(canvas.getByTestId("value")).toHaveTextContent(
      '["refusal:content_policy_violation"]',
    );
  },
};

/** A refusal from the control plane opens the set and is said under it. */
export const Refused: Story = {
  args: {
    initial: ["timeout"],
    error: "The control plane refused this set: a class it does not know, or one listed twice.",
  },
  play: async ({ canvas }) => {
    await expect(canvas.getByRole("alert")).toHaveTextContent(/refused this set/);
    await expect(canvas.getByRole("button", { name: "Fallback for timeout" })).toBeInvalid();
  },
};
