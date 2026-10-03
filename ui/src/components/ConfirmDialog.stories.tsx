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

/**
 * The header's close button, Escape and the scrim still close the dialog
 * mid-flight. Nothing times a request out — not the fetch, not the control
 * plane — so a delete stuck behind a row lock would otherwise trap the operator
 * in a full-page modal until they reloaded.
 */
export const DismissalMidFlightStillCloses: Story = {
  args: { pending: true },
  play: async ({ args }) => {
    const dialog = await screen().findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await expect(args.onOpenChange).toHaveBeenCalledWith(false);
  },
};

/**
 * The confirm going disabled mid-flight takes focus with it, and `<body>` is
 * outside the panel whose Tab trap hears the next key (#1998). Focus falls to
 * the panel instead, Tab cycles inside it, the page stays inert, and a
 * dismissal hands focus back to the control that raised the dialog.
 */
function Raised(args: React.ComponentProps<typeof ConfirmDialog>) {
  const [open, setOpen] = React.useState(false);
  const [pending, setPending] = React.useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Remove ops-slack
      </button>
      <ConfirmDialog
        {...args}
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setPending(false);
        }}
        pending={pending}
        // the request never answers
        onConfirm={() => setPending(true)}
      />
    </>
  );
}

export const PendingKeepsFocusInside: Story = {
  render: (args) => <Raised {...args} />,
  play: async ({ canvasElement }) => {
    const trigger = within(canvasElement).getByRole("button", { name: "Remove ops-slack" });
    await userEvent.click(trigger);
    const dialog = await screen().findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete channel" }));
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled(),
    );
    await waitFor(() => expect(dialog).toHaveFocus());
    for (const shift of [false, false, false, true, true, true]) {
      await userEvent.tab({ shift });
      await expect(dialog).toContainElement(document.activeElement as HTMLElement);
    }
    await expect(trigger.closest("[inert]")).not.toBeNull();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen().queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(trigger).toHaveFocus());
    await expect(trigger.closest("[inert]")).toBeNull();
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

// the message keeps the gap the children keep between themselves (gap-2.5 =
// 10px), and a dialog with no children adds none above it (#2359)
export const FailedGapUnderChildren: Story = {
  args: { error: new Error("dashboard authentication requires a credential") },
  render: (args) => (
    <ConfirmDialog {...args}>
      <ul className="flex flex-col gap-2.5">
        <li data-testid="first">first loosening</li>
        <li data-testid="last">last loosening</li>
      </ul>
    </ConfirmDialog>
  ),
  play: async () => {
    const canvas = screen();
    const alert = await canvas.findByRole("alert");
    const last = canvas.getByTestId("last").getBoundingClientRect();
    const first = canvas.getByTestId("first").getBoundingClientRect();
    const gap = alert.getBoundingClientRect().top - last.bottom;
    await expect(gap).toBeCloseTo(last.top - first.bottom, 0);
    await expect(gap).toBeGreaterThan(0);
  },
};

export const FailedWithoutChildrenAddsNoGap: Story = {
  args: { error: new Error("channel is referenced by 2 alert rules") },
  play: async () => {
    const alert = await screen().findByRole("alert");
    await expect(getComputedStyle(alert).marginTop).toBe("0px");
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

/**
 * A refusal that lands in the tick the request started never commits
 * `pending={true}`: react-query hands the pending and the error to one notify
 * batch, which is what a stub answering a DELETE at once does. The dialog used
 * to read the failure off the pending edge and so reported nothing but the
 * press (#1761); it reads it off the press now.
 */
function RefusedAtOnce(args: React.ComponentProps<typeof ConfirmDialog>) {
  const [error, setError] = React.useState<unknown>(undefined);
  const refusals = React.useRef(0);
  return (
    <UxScreenProvider screen={SCREEN}>
      <ConfirmDialog
        {...args}
        name={TARGET}
        pending={false}
        error={error}
        onConfirm={() => {
          refusals.current += 1;
          // a fresh error each time, the way every failed request throws one
          setError(new Error(`channel is referenced by 2 alert rules (${refusals.current})`));
        }}
      />
    </UxScreenProvider>
  );
}

export const ASameTickRefusalEmitsAnError: Story = {
  render: (args) => <RefusedAtOnce {...args} />,
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Delete channel" }));
    await expect(await screen().findByRole("alert")).toHaveTextContent("(1)");
    await waitFor(() => {
      const outcomes = uxEvents()
        .filter((e) => e.action === "form_submit" && e.target === TARGET)
        .map((e) => e.outcome);
      expect(outcomes).toEqual(["ok", "error"]);
    });
    expectNoUxEvent("save_confirmed", TARGET);
  },
};

/**
 * A retry refused the same way leaves `pending` false and an error standing on
 * both sides of the press, so an effect keyed on "is there an error" never ran
 * again. Keyed on the error's identity, the second refusal is a row of its own
 * — and the press before it is a `retry_submit`, not a second first attempt.
 */
export const ARetryRefusedTheSameWayIsReportedAgain: Story = {
  render: (args) => <RefusedAtOnce {...args} />,
  play: async () => {
    const confirm = screen().getByRole("button", { name: "Delete channel" });
    await userEvent.click(confirm);
    await expect(await screen().findByRole("alert")).toHaveTextContent("(1)");
    await userEvent.click(confirm);
    await waitFor(() => expect(screen().getByRole("alert")).toHaveTextContent("(2)"));
    await waitFor(() => {
      const outcomes = uxEvents()
        .filter((e) => e.action === "form_submit" && e.target === TARGET)
        .map((e) => e.outcome);
      expect(outcomes).toEqual(["ok", "error", "error"]);
    });
    await expectUxEvent("retry_submit", TARGET);
  },
};

/**
 * A confirm that lands is a `save_confirmed`, the row the hand-rolled dialogs
 * emitted before #1738 moved them here (#1761). The caller closes the dialog
 * from its mutation's `onSuccess`, which for a hook-level callback runs while
 * the request is still pending — so this closes first and settles a commit
 * later, and the row still has to carry how long it took.
 */
function LandsAfterClosing(args: React.ComponentProps<typeof ConfirmDialog>) {
  const [open, setOpen] = React.useState(true);
  const [pending, setPending] = React.useState(false);
  const [landed, setLanded] = React.useState(false);
  React.useEffect(() => {
    if (!pending) return;
    // hook-level onSuccess: the dialog closes, the request is not settled yet
    setOpen(false);
    setLanded(true);
  }, [pending]);
  React.useEffect(() => {
    if (landed) setPending(false);
  }, [landed]);
  return (
    <UxScreenProvider screen={SCREEN}>
      <ConfirmDialog
        {...args}
        name={TARGET}
        open={open}
        onOpenChange={setOpen}
        pending={pending}
        onConfirm={() => setPending(true)}
      />
    </UxScreenProvider>
  );
}

export const ALandedConfirmEmitsSaveConfirmed: Story = {
  render: (args) => <LandsAfterClosing {...args} />,
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Delete channel" }));
    await waitFor(() => expect(screen().queryByRole("dialog")).toBeNull());
    const confirmed = await expectUxEvent("save_confirmed", TARGET);
    await expect(confirmed.screen).toBe(SCREEN);
    await expect(typeof confirmed.duration_ms).toBe("number");
    // a landed confirm is neither a refusal nor an abandon on its way out
    await expect(
      uxEvents()
        .filter((e) => e.action === "form_submit" && e.target === TARGET)
        .map((e) => e.outcome),
    ).toEqual(["ok"]);
    expectNoUxEvent("form_abandon", TARGET);
  },
};

/**
 * A dialog dismissed while its request is still out has the caller reset the
 * mutation, so `pending` falls with no error and the dialog closed — exactly
 * what a landing looks like from here. The dismissal disarms the read first, so
 * the press is the last row: the dialog never learned how the request ended and
 * does not claim to.
 */
function DismissedMidFlight(args: React.ComponentProps<typeof ConfirmDialog>) {
  const [open, setOpen] = React.useState(true);
  const [pending, setPending] = React.useState(false);
  return (
    <UxScreenProvider screen={SCREEN}>
      <ConfirmDialog
        {...args}
        name={TARGET}
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          // what every call site does on close: `remove.reset()`
          if (!next) setPending(false);
        }}
        pending={pending}
        // the request never answers
        onConfirm={() => setPending(true)}
      />
    </UxScreenProvider>
  );
}

export const ADismissalMidFlightConfirmsNothing: Story = {
  render: (args) => <DismissedMidFlight {...args} />,
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Delete channel" }));
    await waitFor(() => expect(screen().getByRole("button", { name: "Cancel" })).toBeDisabled());
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen().queryByRole("dialog")).toBeNull());
    await expectUxEvent("form_submit", TARGET);
    expectNoUxEvent("save_confirmed", TARGET);
    // a press on record is not an abandon either
    expectNoUxEvent("form_abandon", TARGET);
  },
};

/**
 * A confirmation handed no `pending` runs no request — the discard prompt
 * closes a sheet and is done — so closing it after the press is not a landing
 * and reports none.
 */
export const AConfirmWithNoRequestConfirmsNothing: Story = {
  render: (args) => {
    const [open, setOpen] = React.useState(true);
    return (
      <UxScreenProvider screen={SCREEN}>
        <ConfirmDialog
          {...args}
          name={TARGET}
          open={open}
          onOpenChange={setOpen}
          pending={undefined}
          onConfirm={() => setOpen(false)}
        />
      </UxScreenProvider>
    );
  },
  play: async () => {
    await userEvent.click(screen().getByRole("button", { name: "Delete channel" }));
    await waitFor(() => expect(screen().queryByRole("dialog")).toBeNull());
    await expectUxEvent("form_submit", TARGET);
    expectNoUxEvent("save_confirmed", TARGET);
    expectNoUxEvent("form_abandon", TARGET);
  },
};
