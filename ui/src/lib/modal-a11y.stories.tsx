import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { openModalCount, useModalA11y } from "@/lib/modal-a11y";

// `useModalA11y` is what lets Dialog and Sheet claim `aria-modal` honestly
// (#1181): focus into the panel, Tab trapped inside it, Escape for the topmost
// modal only, the page behind frozen, focus back to the opener on close.
//
// Every one of those is a keyboard fact about a real document, and there is no
// DOM under `bun test`, so the hook is asserted here rather than in the unit
// suite — and directly rather than through whichever sheet happens to use it,
// so a regression in the trap reads as a failure of the trap (#1603).

/** the panel every story opens, wired to the hook and nothing else */
function Modal({
  open,
  onClose,
  initialFocus,
  label,
  children,
}: {
  open: boolean;
  onClose: () => void;
  initialFocus?: "first" | "panel";
  label: string;
  children?: React.ReactNode;
}) {
  const panel = React.useRef<HTMLDivElement>(null);
  const modal = useModalA11y(panel, { open, onEscape: onClose, initialFocus });
  if (!open) return null;
  return (
    <div
      ref={panel}
      role="dialog"
      aria-modal="true"
      aria-label={label}
      {...modal}
      className="rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] p-4"
    >
      {children}
    </div>
  );
}

/** the opener, so "focus went back where it came from" is observable */
function Harness({
  initialFocus,
  children,
}: {
  initialFocus?: "first" | "panel";
  children?: React.ReactNode;
}) {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="flex flex-col items-start gap-3">
      <button type="button" onClick={() => setOpen(true)}>
        open the panel
      </button>
      <Modal open={open} onClose={() => setOpen(false)} initialFocus={initialFocus} label="Edit provider">
        {children ?? (
          <>
            <input aria-label="name" defaultValue="openai" />
            <button type="button">save</button>
            <button type="button" onClick={() => setOpen(false)}>
              cancel
            </button>
          </>
        )}
      </Modal>
      <p data-testid="open-modals">{String(openModalCount())}</p>
    </div>
  );
}

const meta = {
  title: "Behaviour/ModalA11y",
  component: Harness,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Harness>;

export default meta;
type Story = StoryObj<typeof meta>;

const open = async (canvas: ReturnType<typeof within>) =>
  userEvent.click(canvas.getByRole("button", { name: "open the panel" }));

/**
 * A form's first control, not the close button every header opens with —
 * a keyboard user lands where they would have started typing.
 */
export const FocusesTheFirstFormControl: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await open(canvas);
    await waitFor(() => expect(canvas.getByLabelText("name")).toHaveFocus());
  },
};

/** a confirmation has no field to fill, so the panel itself takes focus */
export const FocusesThePanelWhenAsked: Story = {
  args: { initialFocus: "panel" },
  render: (args) => (
    <Harness {...args}>
      <p>this cannot be undone</p>
      <button type="button">delete</button>
    </Harness>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await open(canvas);
    await waitFor(() => expect(canvas.getByRole("dialog")).toHaveFocus());
    // and never the destructive button, which Enter would then fire
    await expect(canvas.getByRole("button", { name: "delete" })).not.toHaveFocus();
  },
};

/**
 * Tab cycles inside the panel. Without the trap the next Tab walks into the
 * page behind the scrim, where a keyboard user cannot see what they are on.
 */
export const TabCyclesInsideThePanel: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await open(canvas);
    await waitFor(() => expect(canvas.getByLabelText("name")).toHaveFocus());
    await userEvent.tab();
    await expect(canvas.getByRole("button", { name: "save" })).toHaveFocus();
    await userEvent.tab();
    await expect(canvas.getByRole("button", { name: "cancel" })).toHaveFocus();
    // the last control wraps to the first rather than leaving the panel
    await userEvent.tab();
    await expect(canvas.getByLabelText("name")).toHaveFocus();
  },
};

/** and backwards from the first control to the last */
export const ShiftTabWrapsBackwards: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await open(canvas);
    await waitFor(() => expect(canvas.getByLabelText("name")).toHaveFocus());
    await userEvent.tab({ shift: true });
    await expect(canvas.getByRole("button", { name: "cancel" })).toHaveFocus();
  },
};

/** Escape closes it, and focus lands back on the control that opened it */
export const EscapeClosesAndReturnsFocus: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await open(canvas);
    await waitFor(() => expect(canvas.getByRole("dialog")).toBeInTheDocument());
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
    // the panel is gone and focus goes back in the effect's cleanup, which is a
    // separate step: assert it with a waiter, never on the frame after removal
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "open the panel" })).toHaveFocus(),
    );
  },
};

/** the page behind a modal does not scroll, and gets its scrolling back after */
export const FreezesThePageBehind: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = canvasElement.ownerDocument.body;
    await open(canvas);
    await waitFor(() => expect(body.style.overflow).toBe("hidden"));
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(body.style.overflow).not.toBe("hidden"));
  },
};

/** two panels, so the stack has something to be wrong about */
function Stacked() {
  const [outer, setOuter] = React.useState(false);
  const [inner, setInner] = React.useState(false);
  return (
    <div className="flex flex-col items-start gap-3">
      <button type="button" onClick={() => setOuter(true)}>
        open the sheet
      </button>
      <Modal open={outer} onClose={() => setOuter(false)} label="Provider sheet">
        <button type="button" onClick={() => setInner(true)}>
          open the dialog
        </button>
        <Modal open={inner} onClose={() => setInner(false)} label="Confirm delete">
          <button type="button">delete</button>
        </Modal>
      </Modal>
    </div>
  );
}

/**
 * Escape takes down the dialog raised over the sheet, and only that one.
 * Closing both at once loses the form underneath, which is the user's work.
 */
export const EscapeClosesOnlyTheTopmostModal: Story = {
  render: () => <Stacked />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "open the sheet" }));
    await waitFor(() => expect(canvas.getByRole("button", { name: "open the dialog" })).toBeVisible());
    await userEvent.click(canvas.getByRole("button", { name: "open the dialog" }));
    await waitFor(() => expect(canvas.getAllByRole("dialog")).toHaveLength(2));
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.getAllByRole("dialog")).toHaveLength(1));
    await expect(canvas.getByRole("dialog")).toHaveAttribute("aria-label", "Provider sheet");
    // and the second Escape closes what is now on top
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
  },
};
