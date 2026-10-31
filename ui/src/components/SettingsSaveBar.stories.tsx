import type { Meta, StoryObj } from "@storybook/react";
import { expect, fn, userEvent, waitFor, within } from "storybook/test";

import { SettingsSaveBar } from "./SettingsSaveBar";
import { Harness, json } from "@/pages/story-harness";

const clean = {
  dirty: false,
  changed: [] as string[],
  savedAt: null as number | null,
  leavePrompt: null,
  fields: { invalid: [] as string[] },
};

const node = (id: string, converged: boolean) => ({
  id,
  role: "gateway",
  build_version: "1.0.0",
  config_version: converged ? 8 : 7,
  desired_state: "active",
  state_changed_at: "2026-07-31T12:00:00Z",
  first_seen_at: "2026-07-31T12:00:00Z",
  last_seen_at: "2026-07-31T12:00:00Z",
  live: true,
  converged,
});

const meta = {
  title: "Forms/SettingsSaveBar",
  component: SettingsSaveBar,
  parameters: { layout: "padded" },
  args: { form: { ...clean, discard: fn() }, pending: false },
  // the bar reads the gateways' config versions once a save has happened
  decorators: [
    (Story) => (
      <Harness
        fetchStub={async (input) =>
          String(input).includes("/cluster/nodes")
            ? json([node("gw-a", true), node("gw-b", false)])
            : json({})
        }
      >
        <form onSubmit={(event) => event.preventDefault()}>
          <Story />
        </form>
      </Harness>
    ),
  ],
} satisfies Meta<typeof SettingsSaveBar>;

export default meta;
type Story = StoryObj<typeof meta>;

/** Nothing edited: Save and Discard are both out of reach, and the bar says when it was last saved. */
export const Clean: Story = {
  args: { updatedAt: "2026-07-31T12:00:00Z" },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await expect(canvas.getByRole("button", { name: "Discard changes" })).toBeDisabled();
    await expect(canvas.getByText(/^Last saved /)).toBeVisible();
  },
};

/** Edited: both buttons wake up, and the line counts the changes, in the plural forms. */
export const Edited: Story = {
  args: { form: { ...clean, dirty: true, changed: ["a", "b"], discard: fn() } },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByRole("button", { name: "Save changes" })).not.toHaveAttribute(
      "aria-disabled",
    );
    await expect(canvas.getByText(/^2 changes not saved yet/)).toHaveAttribute("role", "status");
    await userEvent.click(canvas.getByRole("button", { name: "Discard changes" }));
    await expect(args.form.discard).toHaveBeenCalledTimes(1);
  },
};

/** A field is wrong: the count wins over the edit line, and Save stays pressable. */
export const FieldsNeedAttention: Story = {
  args: {
    form: { ...clean, dirty: true, changed: ["a"], fields: { invalid: ["a"] }, discard: fn() },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("1 field needs attention")).toBeVisible();
    await expect(canvas.getByRole("button", { name: "Save changes" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  },
};

/** Saved: when, and how many live gateways already run it, from their config versions. */
export const SavedAndPickedUp: Story = {
  args: { form: { ...clean, savedAt: Date.now(), discard: fn() } },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/Saved at /)).toBeVisible();
    await waitFor(() => expect(canvas.getByText(/Applied to 1 of 2 live gateways/)).toBeVisible());
  },
};

/** A setting no gateway reads makes no claim about the gateways. */
export const WithoutGateways: Story = {
  args: { form: { ...clean, savedAt: Date.now(), discard: fn() }, gateways: false },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText(/Saved at /)).toBeVisible();
    await expect(canvas.queryByText(/Applied to/)).toBeNull();
    await expect(canvas.queryByText(/live gateway/)).toBeNull();
  },
};
