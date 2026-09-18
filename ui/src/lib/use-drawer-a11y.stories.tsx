import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { useDrawerA11y } from "@/lib/use-drawer-a11y";

// An inline detail drawer is deliberately not a modal — the table beside it
// stays usable — so it has no trap and no scrim, and what it owes the keyboard
// is narrower than `useModalA11y`: focus in on open, Escape to close, focus
// back to the row that opened it. Asserted here because there is no DOM under
// `bun test`, and directly rather than through a screen, so a regression in the
// drawer reads as a failure of the drawer (#1603).

function Drawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const drawer = useDrawerA11y(open, onClose);
  if (!open) return null;
  return (
    <aside
      ref={drawer.ref as React.RefObject<HTMLElement>}
      tabIndex={drawer.tabIndex}
      aria-label="Request detail"
      className="rounded-md border border-[color:var(--border-default)] p-4"
    >
      <p>req-1 · 200 · 412 ms</p>
      <button type="button" onClick={onClose}>
        close
      </button>
    </aside>
  );
}

function Harness({ withModal = false }: { withModal?: boolean }) {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="flex flex-col items-start gap-3">
      {/* the table the drawer opens beside: two rows, so "focus went back to
          the one that opened it" is a claim with a wrong answer available */}
      <button type="button" onClick={() => setOpen(true)}>
        row req-1
      </button>
      <button type="button">row req-2</button>
      <Drawer open={open} onClose={() => setOpen(false)} />
      {withModal && open && (
        <div role="dialog" aria-modal="true" aria-label="Confirm">
          <button type="button">delete</button>
        </div>
      )}
    </div>
  );
}

const meta = {
  title: "Behaviour/DrawerA11y",
  component: Harness,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Harness>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * Focus moves into the drawer, so the next Tab reads its content rather than
 * the row after the one just clicked.
 */
export const FocusesTheDrawerOnOpen: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "row req-1" }));
    await waitFor(() => expect(canvas.getByRole("complementary")).toHaveFocus());
  },
};

/** Escape closes it, and focus returns to the row that opened it — not the first row */
export const EscapeClosesAndReturnsFocusToTheRow: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "row req-1" }));
    await waitFor(() => expect(canvas.getByRole("complementary")).toHaveFocus());
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("complementary")).toBeNull());
    // the drawer is gone and focus goes back in the effect's cleanup, which is a
    // separate step: assert it with a waiter, never on the frame after removal
    await waitFor(() => expect(canvas.getByRole("button", { name: "row req-1" })).toHaveFocus());
  },
};

/**
 * A modal raised over the drawer owns Escape. Without that check one keystroke
 * closes both, and the drawer the dialog was asking about goes with it.
 */
export const AModalOverTheDrawerOwnsEscape: Story = {
  args: { withModal: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "row req-1" }));
    await waitFor(() => expect(canvas.getByRole("complementary")).toBeInTheDocument());
    await userEvent.keyboard("{Escape}");
    // still there: the dialog is what that Escape was for
    await expect(canvas.getByRole("complementary")).toBeInTheDocument();
  },
};
