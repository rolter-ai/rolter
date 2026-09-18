import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { FieldLabel } from "./field-label";
import { Input } from "./input";
import { Segmented } from "./segmented";

const meta = {
  title: "Forms/FieldLabel",
  component: FieldLabel,
  parameters: { layout: "padded" },
  args: { label: "Upstream name" },
} satisfies Meta<typeof FieldLabel>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: (args) => (
    <div className="max-w-sm space-y-1.5">
      <FieldLabel {...args} htmlFor="fl-default" />
      <Input id="fl-default" defaultValue="gpt-4o" />
    </div>
  ),
};

/** A required field carries the asterisk beside its name, not in it. */
export const Required: Story = {
  args: { required: true },
  render: (args) => (
    <div className="max-w-sm space-y-1.5">
      <FieldLabel {...args} htmlFor="fl-required" />
      <Input id="fl-required" placeholder="gpt-4o" />
    </div>
  ),
};

/**
 * `htmlFor` is what keeps the label from dangling: the control it names is
 * reachable by that name, which is also how a screen reader announces it.
 */
export const NamesItsControl: Story = {
  render: (args) => (
    <div className="max-w-sm space-y-1.5">
      <FieldLabel {...args} htmlFor="fl-named" required />
      <Input id="fl-named" defaultValue="gpt-4o" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // bites if `htmlFor` stops reaching the control: the label would still be
    // on screen and the input would still be unnamed
    const input = canvas.getByLabelText(/Upstream name/);
    await expect(input).toHaveValue("gpt-4o");
    await expect(canvas.getByText("*")).toBeVisible();
  },
};

/**
 * `info` hangs an (i) beside the name. Its own accessible name is built from
 * the field's, so a screen reader hears which field the note is about rather
 * than a row of identical "More info" buttons.
 */
export const WithInfo: Story = {
  args: { info: "The model id the provider knows, sent upstream verbatim." },
  render: (args) => (
    <div className="max-w-sm space-y-1.5">
      <FieldLabel {...args} htmlFor="fl-info" />
      <Input id="fl-info" defaultValue="gpt-4o" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const hint = canvas.getByRole("button", { name: "About Upstream name" });
    // hover, not click: with a pointer the hover has already opened the note,
    // so a click is the gesture that closes it again (see InfoHint's stories)
    await userEvent.hover(hint);
    await expect(canvas.getByRole("tooltip")).toHaveTextContent(/sent upstream verbatim/);
  },
};

/**
 * A group has no single control to point `htmlFor` at, so it takes `id` and
 * the group references it with `aria-labelledby` instead.
 */
export const NamesAGroup: Story = {
  render: () => {
    const [mode, setMode] = React.useState("chat");
    return (
      <div className="max-w-sm space-y-1.5">
        <FieldLabel label="Modality" id="fl-group" />
        <Segmented
          value={mode}
          labelledBy="fl-group"
          options={[
            { value: "chat", label: "Chat" },
            { value: "embedding", label: "Embedding" },
          ]}
          onChange={setMode}
        />
      </div>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // bites if the `id` stops landing on the <label>: the group loses its name
    await expect(canvas.getByRole("radiogroup", { name: "Modality" })).toBeVisible();
  },
};
