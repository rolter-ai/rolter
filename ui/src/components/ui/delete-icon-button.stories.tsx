import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, within } from "storybook/test";

import { DeleteIconButton } from "./delete-icon-button";
import { UxScreenProvider } from "@/lib/ux-react";
import {
  expectAllowed,
  expectNoUxEvent,
  expectRefused,
  expectUxEvent,
  Harness,
  recordUxEvents,
  routes,
} from "@/pages/story-harness";

const meta = {
  title: "Primitives/DeleteIconButton",
  component: DeleteIconButton,
  parameters: { layout: "padded" },
  args: { label: "Delete provider openai" },
  // a fresh UX queue per story, so a reach recorded by one is never read by the next
  beforeEach: recordUxEvents,
} satisfies Meta<typeof DeleteIconButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

export const Pending: Story = { args: { pending: true } };

/**
 * The control is an icon with no text, so `label` is the only name it has — and
 * it names the row, not the action, so a list of eight delete buttons does not
 * read as eight identical ones.
 */
export const NamesTheRow: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Delete provider openai" });
    await expect(button).toBeEnabled();
    // the hover title falls back to the name rather than being left empty
    await expect(button).toHaveAttribute("title", "Delete provider openai");
  },
};

/**
 * A delete in flight swaps the bin for a spinner and locks the button.
 *
 * `Providers` and `ProviderGroups` had the same markup as `Models` and neither
 * did this, so the same action looked dead on two screens and alive on the
 * third (#1686). Pressing it twice queued two deletes.
 */
export const PendingBlocksASecondPress: Story = {
  render: function Render() {
    const [presses, setPresses] = React.useState(0);
    return (
      <div className="flex items-center gap-2 text-sm">
        <DeleteIconButton
          label="Delete provider openai"
          pending
          onClick={() => setPresses(presses + 1)}
        />
        <span>{presses} press(es)</span>
      </div>
    );
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const button = canvas.getByRole("button", { name: "Delete provider openai" });
    await expect(button).toBeDisabled();
    await userEvent.click(button, { pointerEventsCheck: 0 });
    await expect(canvas.getByText("0 press(es)")).toBeInTheDocument();
  },
};

const stub = routes([]);

/**
 * A gated row keeps the control visible and says why it is dead in the title.
 *
 * `gate` is the house pattern (#1759): a screen that disabled the button from
 * its own `useGate` got the same look, but the reach for it never reached the
 * UX stream.
 */
export const Refused: Story = {
  render: () => (
    <Harness fetchStub={stub} role="viewer">
      <DeleteIconButton
        gate="provider:delete"
        control="provider-delete"
        label="Delete provider openai"
      />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Delete provider openai");
  },
};

export const Allowed: Story = {
  render: () => (
    <Harness fetchStub={stub} role="admin">
      <DeleteIconButton
        gate="provider:delete"
        control="provider-delete"
        label="Delete provider openai"
      />
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectAllowed(canvasElement, "Delete provider openai");
    // an offered delete has no reason to give, so the title is the name again
    await expect(
      within(canvasElement).getByRole("button", { name: "Delete provider openai" }),
    ).toHaveAttribute("title", "Delete provider openai");
  },
};

/** A refused delete is recorded under its own slug, the way `GatedButton` is (#1731). */
export const RefusedRecordsTheReach: Story = {
  render: () => (
    <Harness fetchStub={stub} role="viewer">
      <UxScreenProvider screen="providers">
        <DeleteIconButton
          gate="provider:delete"
          control="provider-delete"
          label="Delete provider openai"
        />
      </UxScreenProvider>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    await expectRefused(canvasElement, "Delete provider openai");
    const button = within(canvasElement).getByRole("button", { name: "Delete provider openai" });
    await userEvent.click(button, { pointerEventsCheck: 0 });
    const event = await expectUxEvent("refused_click", "provider-delete:provider:delete");
    await expect(event.screen).toBe("providers");
    // the row's name is on the button and never on the event
    await expect(JSON.stringify(event)).not.toContain("openai");
  },
};

/**
 * A delete disabled for its own reason — no row selected, a write in flight —
 * records nothing: only a permission refusal is a struggle signal.
 */
export const UngatedRecordsNothing: Story = {
  render: () => (
    <UxScreenProvider screen="providers">
      <DeleteIconButton label="Delete provider openai" disabled />
    </UxScreenProvider>
  ),
  play: async ({ canvasElement }) => {
    const button = within(canvasElement).getByRole("button", { name: "Delete provider openai" });
    // story-wait-allow: disabled by its own prop from the first paint, and no
    // gate is asked, so there is no answer to wait for
    await expect(button).toBeDisabled();
    await userEvent.click(button, { pointerEventsCheck: 0 });
    expectNoUxEvent("refused_click");
  },
};
