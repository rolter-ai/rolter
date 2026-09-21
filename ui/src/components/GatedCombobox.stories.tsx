import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, within } from "storybook/test";

import { GatedCombobox } from "./GatedCombobox";
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

// The row picker that knows whether the caller may change it (#1759).
//
// Same answers as `GatedSwitch`: allowed, refused, and only an explicit "no"
// disables. A key's cache mode is the case it was built for — an update, the
// same `virtual_key:update` the row's toggle and edit sheet ask for.

const stub = routes([]);
const OPTIONS = [
  { value: "inherit", label: "Inherit route setting" },
  { value: "off", label: "Off" },
  { value: "on", label: "On" },
];
const LABEL = "Response cache policy for backend service";

const meta = {
  title: "Controls/GatedCombobox",
  component: GatedCombobox,
  parameters: { layout: "centered" },
  beforeEach: recordUxEvents,
  // every story supplies its own `render`; these are the args the docs page
  // introspects, and `gate` and `control` are required so they cannot be left off
  args: {
    gate: "virtual_key:update",
    control: "key-cache",
    options: OPTIONS,
    value: "inherit",
    onChange: () => {},
    "aria-label": LABEL,
  },
} satisfies Meta<typeof GatedCombobox>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Allowed: Story = {
  render: (args) => (
    <Harness fetchStub={stub} role="admin">
      <GatedCombobox {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, LABEL, "combobox");
  },
};

export const Refused: Story = {
  render: (args) => (
    <Harness fetchStub={stub} role="viewer">
      <GatedCombobox {...args} />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, LABEL, NEEDS_ADMIN, "combobox");
  },
};

// the reach for a refused picker lands in the UX stream under its slug, which a
// screen that disabled the picker from its own `useGate` never recorded
export const RefusedRecordsTheReach: Story = {
  render: (args) => (
    <Harness fetchStub={stub} role="viewer">
      <UxScreenProvider screen="keys">
        <GatedCombobox {...args} />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, LABEL, NEEDS_ADMIN, "combobox");
    const picker = within(canvasElement).getByRole("combobox", { name: LABEL });
    await userEvent.click(picker, { pointerEventsCheck: 0 });
    const event = await expectUxEvent("refused_click", "key-cache:virtual_key:update");
    await expect(event.screen).toBe("keys");
    await expect(JSON.stringify(event)).not.toContain("backend service");
  },
};
