import type { Meta, StoryObj } from "@storybook/react";
import { expect, userEvent, within } from "storybook/test";

import { atMobile, expectNoHorizontalOverflow } from "@/lib/story-viewport";

import { Switch } from "./switch";
import { Field } from "./field";
import { Input } from "./input";
import { SettingsPanel } from "./settings-panel";

const meta = {
  title: "Forms/SettingsPanel",
  component: SettingsPanel,
  parameters: { layout: "padded" },
} satisfies Meta<typeof SettingsPanel>;

export default meta;
type Story = StoryObj<typeof meta>;

const controls = (
  <>
    <Field label="Temperature">
      <Input defaultValue="0.7" />
    </Field>
    <Field label="Top P">
      <Input defaultValue="1.0" />
    </Field>
  </>
);

export const Default: Story = {
  args: {
    title: "Sampling",
    description: "Defaults applied to every request that does not set its own.",
    children: controls,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // the title is a real heading, so a screen reader can jump between sections
    await expect(canvas.getByRole("heading", { level: 2, name: "Sampling" })).toBeVisible();
    // the line saying what the group is for is the reason a panel is not just a
    // bordered div, so it is asserted rather than left to the snapshot
    await expect(
      canvas.getByText("Defaults applied to every request that does not set its own."),
    ).toBeVisible();
  },
};

/** the description is optional — a title alone is enough when the group is obvious */
export const TitleOnly: Story = {
  args: { title: "Timeouts", children: controls },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("heading", { name: "Timeouts" })).toBeVisible();
    // and no empty paragraph left behind where the description would have been
    await expect(canvasElement.querySelector("section p")).toBeNull();
  },
};

/**
 * Switched off as a block.
 *
 * The `fieldset` is what does it, not a faded `div`: fading a live div drags
 * its labels below 4.5:1 and tells assistive tech nothing, so the controls stay
 * readable and are genuinely disabled instead (#1181).
 */
export const Dimmed: Story = {
  args: {
    title: "Sampling",
    description: "Turn the policy on to edit these.",
    dimmed: true,
    children: controls,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const temperature = canvas.getByLabelText("Temperature");
    await expect(temperature).toBeDisabled();
    // the group reads as off through the disabled fieldset, never an opacity dim
    // (DESIGN.md "Don't dim with opacity", #2213)
    const fieldset = canvasElement.querySelector("fieldset");
    await expect(fieldset).toBeDisabled();
    await expect(fieldset?.style.opacity).toBe("");
    await expect(getComputedStyle(fieldset as Element).opacity).toBe("1");
    await expect(getComputedStyle(canvasElement.querySelector("section") as Element).opacity).toBe(
      "1",
    );
    // and disabled all the way through: typing into it changes nothing, which a
    // faded div would have allowed
    await userEvent.type(temperature, "9");
    await expect(temperature).toHaveValue("0.7");
  },
};

/** the same panel live, so the two states can be read against each other */
export const Enabled: Story = {
  args: {
    title: "Sampling",
    description: "Turn the policy on to edit these.",
    dimmed: false,
    children: controls,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByLabelText("Temperature")).toBeEnabled();
  },
};

/**
 * A switch in the header governs the group below it.
 *
 * It sits outside the fieldset, so a switched-off panel can be switched back on
 * — the fieldset would have disabled it otherwise.
 */
export const HeaderSwitch: Story = {
  args: {
    title: "Queue",
    description: "Hold requests when every upstream is busy.",
    dimmed: true,
    headingLevel: 3,
    action: <Switch aria-label="Enable queue" checked={false} onCheckedChange={() => {}} />,
    children: controls,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("heading", { level: 3, name: "Queue" })).toBeVisible();
    await expect(canvas.getByRole("switch", { name: "Enable queue" })).toBeEnabled();
    await expect(canvas.getByLabelText("Temperature")).toBeDisabled();
  },
};

/**
 * The panel at 375, where the control row wraps.
 *
 * `Performance` wrapped its controls in a plain `div` before #1682 and this one
 * is a `fieldset`, which brings a `min-width: min-content` a div does not have —
 * hence the `min-w-0` on it. This is the regression that would catch the row
 * refusing to wrap.
 */
export const AtMobile: Story = {
  ...atMobile,
  args: {
    title: "Timeouts",
    description: "How long the gateway waits before giving up on an upstream.",
    children: controls,
  },
  play: async () => {
    await expectNoHorizontalOverflow();
  },
};
