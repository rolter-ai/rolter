import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import { GatedInput } from "./GatedInput";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  expectAllowed,
  expectRefused,
  expectUxEvent,
  Harness,
  NEEDS_ADMIN,
  recordUxEvents,
  routes,
} from "@/pages/story-harness";

const stub = routes([]);
const LABEL = "Value of tier";

const meta = {
  title: "Controls/GatedInput",
  component: GatedInput,
  parameters: { layout: "centered" },
  beforeEach: recordUxEvents,
  args: {
    gate: "label:update",
    control: "label-value",
    defaultValue: "gold",
    "aria-label": LABEL,
  },
} satisfies Meta<typeof GatedInput>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Allowed: Story = {
  render: (args) => (
    <Harness fetchStub={stub} role="admin">
      <GatedInput {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, LABEL, "textbox");
    await expect(within(canvasElement).getByRole("textbox", { name: LABEL })).not.toHaveAttribute(
      "readonly",
    );
  },
};

export const Refused: Story = {
  render: (args) => (
    <Harness fetchStub={stub} role="viewer">
      <GatedInput {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, LABEL, NEEDS_ADMIN, "textbox");
    await expect(within(canvasElement).getByRole("textbox", { name: LABEL })).toHaveAttribute(
      "readonly",
    );
  },
};

export const RefusedRecordsTheReach: Story = {
  render: (args) => (
    <Harness fetchStub={stub} role="viewer">
      <UxScreenProvider screen="providers">
        <GatedInput {...args} />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, LABEL, NEEDS_ADMIN, "textbox");
    await userEvent.click(within(canvasElement).getByRole("textbox", { name: LABEL }), {
      pointerEventsCheck: 0,
    });
    const event = await expectUxEvent("refused_click", "label-value:label:update");
    await expect(event.screen).toBe("providers");
  },
};
