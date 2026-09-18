import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, fn, userEvent, waitFor, within } from "storybook/test";

import { ConfirmDialog } from "./ConfirmDialog";
import { UxScreenProvider } from "@/lib/ux-react";
import { expectNoUxEvent, expectUxEvent, recordUxEvents, uxEvents } from "@/pages/story-harness";

const meta = {
  title: "Overlays/ConfirmDialog",
  component: ConfirmDialog,
  parameters: { layout: "centered" },
  args: {
    name: "alert-channel-delete",
    open: true,
    title: "Delete channel ops-slack?",
    description:
      "Alerts routed to this webhook stop being delivered, and every rule pointing at it loses its destination.",
    confirmLabel: "Delete channel",
    tone: "danger",
    pending: false,
    onOpenChange: fn(),
    onConfirm: fn(),
  },
  // every story starts from an empty UX queue and leaves one behind (#1730)
  beforeEach: recordUxEvents,
} satisfies Meta<typeof ConfirmDialog>;

export default meta;
type Story = StoryObj<typeof meta>;

// the dialog portals onto document.body, so canvasElement is empty
const screen = () => within(document.body);

export const Default: Story = {
  play: async ({ args }) => {
    const canvas = screen();
    await waitFor(() => expect(canvas.getByRole("dialog")).toBeVisible());
    await expect(canvas.getByText("Delete channel ops-slack?")).toBeVisible();
    // no error line until something actually failed
    await expect(canvas.queryByRole("alert")).toBeNull();

    await userEvent.click(canvas.getByRole("button", { name: "Delete channel" }));
    await expect(args.onConfirm).toHaveBeenCalled();
    // confirming does not close the dialog — the caller does that on success,
    // so a failed mutation still has somewhere to report itself
    await expect(args.onOpenChange).not.toHaveBeenCalled();
  },
};

export const CancelCloses: Story = {
  play: async ({ args }) => {
    const canvas = screen();
    await userEvent.click(canvas.getByRole("button", { name: "Cancel" }));
    await expect(args.onOpenChange).toHaveBeenCalledWith(false);
    await expect(args.onConfirm).not.toHaveBeenCalled();
  },
};

export const Pending: Story = {
  args: { pending: true },
  play: async () => {
    const canvas = screen();
    // both buttons are out of reach while the request is on the wire: the
    // confirm because it would double-fire, the cancel because it cannot
    // recall a request that already left
    await expect(canvas.getByRole("button", { name: "Delete channel" })).toBeDisabled();
    await expect(canvas.getByRole("button", { name: "Cancel" })).toBeDisabled();
  },
};

export const Failed: Story = {
  args: { error: new Error("channel is referenced by 2 alert rules") },
  play: async () => {
    const canvas = screen();
    const alert = await canvas.findByRole("alert");
    // the control plane's own message, verbatim
    await expect(alert).toHaveTextContent("channel is referenced by 2 alert rules");
    // and the dialog stays usable so the operator can retry or back out
    await expect(canvas.getByRole("button", { name: "Delete channel" })).toBeEnabled();
  },
};

// a non-Error rejection (a thrown string, a rejected promise carrying a code)
// still has to reach the operator rather than render as "[object Object]"
export const FailedWithANonError: Story = {
  args: { error: "upstream timed out" },
  play: async () => {
    await expect(await screen().findByRole("alert")).toHaveTextContent("upstream timed out");
  },
};

export const NeutralTone: Story = {
  args: {
    tone: "default",
    title: "Rotate this key?",
    description: "The current secret stops working the moment the new one is issued.",
    confirmLabel: "Rotate key",
  },
  render: (args) => {
    // the neutral tone exists for actions that are irreversible without being
    // deletions; rotate is the one that motivated it
    const [open, setOpen] = React.useState(true);
    return <ConfirmDialog {...args} open={open} onOpenChange={setOpen} />;
  },
};

/* ---------------- UX stream (#1730) ---------------- */

// the screen key travels through context, so a confirmation rendered outside a
// provider is silent rather than mislabelled — the stories supply one the way
// the app shell does
const SCREEN = "alerting";
const TARGET = "alert-channel-delete";

/**
 * A confirmed delete is a `form_submit`, named by the stable key rather than
 * by the row it was pressed on. `EditorSheet` and this dialog back twenty-eight
 * call sites between them, so the event they emit is the only reason the
 * dogfood week records a destructive action at all.
 */
export const ConfirmEmitsASubmit: Story = {
  render: (args) => (
    <UxScreenProvider screen={SCREEN}>
      <ConfirmDialog {...args} name={TARGET} />
    </UxScreenProvider>
  ),
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Delete channel" }));
    const event = await expectUxEvent("form_submit", TARGET);
    await expect(event.screen).toBe(SCREEN);
    await expect(event.outcome).toBe("ok");
  },
};

/**
 * A delete opened and thought better of is a `form_abandon`, and it must *not*
 * also look like a submit — a dialog that emitted both would make the data say
 * the delete went through.
 */
export const CancelEmitsAnAbandon: Story = {
  render: (args) => {
    const [open, setOpen] = React.useState(true);
    return (
      <UxScreenProvider screen={SCREEN}>
        <ConfirmDialog {...args} name={TARGET} open={open} onOpenChange={setOpen} />
      </UxScreenProvider>
    );
  },
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Cancel" }));
    const event = await expectUxEvent("form_abandon", TARGET);
    await expect(event.screen).toBe(SCREEN);
    await expect(event.outcome).toBe("cancelled");
    expectNoUxEvent("form_submit", TARGET);
  },
};

/**
 * A confirm the control plane refused is a `form_submit` with `error` beside
 * the one that reported the attempt — "how often does this delete fail" is a
 * different question from "how often is it pressed", and one row cannot answer
 * both.
 */
function FailingConfirm(args: React.ComponentProps<typeof ConfirmDialog>) {
  const [pending, setPending] = React.useState(false);
  const [error, setError] = React.useState<unknown>(undefined);
  // settles the round trip on the commit after it started, which is the
  // transition the dialog reads the outcome from — no timer, so the story
  // asserts against a real state change rather than a scheduled one
  React.useEffect(() => {
    if (!pending) return;
    setPending(false);
    setError(new Error("channel is referenced by 2 alert rules"));
  }, [pending]);
  return (
    <UxScreenProvider screen={SCREEN}>
      <ConfirmDialog
        {...args}
        name={TARGET}
        pending={pending}
        error={error}
        onConfirm={() => setPending(true)}
      />
    </UxScreenProvider>
  );
}

export const AFailedConfirmEmitsAnError: Story = {
  render: (args) => <FailingConfirm {...args} />,
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Delete channel" }));
    await expect(await screen().findByRole("alert")).toHaveTextContent("referenced by 2 alert");
    // the press and the refusal are two rows, so the assertion is on the
    // outcomes present rather than on "the" form_submit
    await waitFor(() => {
      const outcomes = uxEvents()
        .filter((e) => e.action === "form_submit" && e.target === TARGET)
        .map((e) => e.outcome);
      expect(outcomes).toEqual(["ok", "error"]);
    });
  },
};
