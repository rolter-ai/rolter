import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { openModalCount, useModalA11y } from "@/lib/modal-a11y";
import { cn } from "@/lib/utils";

// `useModalA11y` is what lets Dialog and Sheet claim `aria-modal` honestly
// (#1181): focus into the panel, Tab trapped inside it, Escape for the topmost
// modal only, the page behind frozen, focus back to the opener on close.
//
// Since #1998 it also makes everything outside the topmost modal `inert` and
// hands focus back to the panel when the control holding it goes away, which
// is what a busy ConfirmDialog or a saving sheet does to its own button.
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
  className,
  children,
}: {
  open: boolean;
  onClose: () => void;
  initialFocus?: "first" | "panel";
  label: string;
  className?: string;
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
      className={cn(
        "rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-elevated)] p-4",
        className,
      )}
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
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        initialFocus={initialFocus}
        label="Edit provider"
      >
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
    await waitFor(() =>
      expect(canvas.getByRole("button", { name: "open the dialog" })).toBeVisible(),
    );
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

/* ---------------- the page behind, and a busy panel (#1998) ---------------- */

/**
 * The page outside the panel is `inert` while it is open, and live again once
 * it closes. The browser refusing focus to the opener is the half a Tab from
 * `<body>` depends on: with the page inert there is nowhere behind the scrim
 * for it to land.
 */
export const ThePageBehindIsInert: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const opener = canvas.getByRole("button", { name: "open the panel" });
    await open(canvas);
    await waitFor(() => expect(canvas.getByLabelText("name")).toHaveFocus());
    await expect(opener.closest("[inert]")).not.toBeNull();
    await expect(canvas.getByRole("dialog").closest("[inert]")).toBeNull();
    opener.focus();
    await expect(opener).not.toHaveFocus();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(opener).toHaveFocus());
    await expect(opener.closest("[inert]")).toBeNull();
  },
};

/** a panel whose save goes busy the way ConfirmDialog's and a sheet's do */
function Busy({ removes = false }: { removes?: boolean }) {
  const [open, setOpen] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  return (
    <div className="flex flex-col items-start gap-3">
      <button type="button" onClick={() => setOpen(true)}>
        open the panel
      </button>
      <Modal open={open} onClose={() => setOpen(false)} label="Edit provider">
        <input aria-label="name" defaultValue="openai" />
        {!(removes && busy) && (
          <button type="button" disabled={busy} onClick={() => setBusy(true)}>
            save
          </button>
        )}
        <button type="button" onClick={() => setOpen(false)}>
          cancel
        </button>
      </Modal>
      <button type="button">behind the scrim</button>
    </div>
  );
}

// Tab and Shift+Tab a few times each, and every stop has to be in the panel
const expectTabStaysIn = async (panel: HTMLElement) => {
  for (const shift of [false, false, false, true, true, true]) {
    await userEvent.tab({ shift });
    await expect(panel).toContainElement(panel.ownerDocument.activeElement as HTMLElement);
  }
};

/**
 * Pressing save disables the button that holds focus, and the browser drops
 * focus onto `<body>` — outside the panel, where its Tab trap never hears the
 * next key. The panel takes focus instead, so Tab keeps cycling inside.
 */
export const FocusFallsBackToThePanelWhenItsControlIsDisabled: Story = {
  render: () => <Busy />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "open the panel" }));
    const dialog = await canvas.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "save" }));
    await expect(within(dialog).getByRole("button", { name: "save" })).toBeDisabled();
    // handed over from an observer, a step after the button went disabled
    await waitFor(() => expect(dialog).toHaveFocus());
    await expectTabStaysIn(dialog);
    await expect(canvas.getByRole("button", { name: "behind the scrim" })).not.toHaveFocus();
  },
};

/** the same when the pressed control leaves the document instead */
export const FocusFallsBackToThePanelWhenItsControlIsRemoved: Story = {
  render: () => <Busy removes />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "open the panel" }));
    const dialog = await canvas.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "save" }));
    await waitFor(() => expect(within(dialog).queryByRole("button", { name: "save" })).toBeNull());
    await waitFor(() => expect(dialog).toHaveFocus());
    await expectTabStaysIn(dialog);
  },
};

/**
 * A dialog over a sheet leaves the sheet as unreachable as the page, and
 * closing the dialog hands both the sheet and the focus back.
 */
export const OnlyTheTopmostModalIsLive: Story = {
  render: () => <Stacked />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const opener = canvas.getByRole("button", { name: "open the sheet" });
    await userEvent.click(opener);
    const raise = await canvas.findByRole("button", { name: "open the dialog" });
    await userEvent.click(raise);
    await waitFor(() => expect(canvas.getAllByRole("dialog")).toHaveLength(2));
    await expect(raise.closest("[inert]")).not.toBeNull();
    await expect(canvas.getByRole("button", { name: "delete" }).closest("[inert]")).toBeNull();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(raise).toHaveFocus());
    await expect(raise.closest("[inert]")).toBeNull();
    // the sheet is still up, so the page behind it is still out of reach
    await expect(opener.closest("[inert]")).not.toBeNull();

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(opener).toHaveFocus());
    await expect(opener.closest("[inert]")).toBeNull();
  },
};

/** two modals side by side, so the lower one can be closed first */
function OutOfOrder() {
  const [sheet, setSheet] = React.useState(false);
  const [dialog, setDialog] = React.useState(false);
  return (
    <div className="flex flex-col items-start gap-3">
      <button type="button" onClick={() => setSheet(true)}>
        open the sheet
      </button>
      <div inert data-testid="already-inert">
        <button type="button">inert before any modal opened</button>
      </div>
      <Modal open={sheet} onClose={() => setSheet(false)} label="Provider sheet">
        <button type="button" onClick={() => setDialog(true)}>
          open the dialog
        </button>
      </Modal>
      <Modal open={dialog} onClose={() => setDialog(false)} label="Confirm delete">
        <button type="button" onClick={() => setSheet(false)}>
          close the sheet underneath
        </button>
      </Modal>
    </div>
  );
}

/**
 * The sheet closes while the dialog over it stays up. The page stays inert and
 * scroll-locked for the dialog, and once that closes too everything is exactly
 * as it was, including inert this hook never set.
 */
export const ModalsClosingOutOfOrderRestoreThePage: Story = {
  render: () => <OutOfOrder />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = canvasElement.ownerDocument.body;
    const overflow = body.style.overflow;
    const opener = canvas.getByRole("button", { name: "open the sheet" });
    await userEvent.click(opener);
    await userEvent.click(await canvas.findByRole("button", { name: "open the dialog" }));
    await waitFor(() => expect(canvas.getAllByRole("dialog")).toHaveLength(2));
    await expect(canvas.getByRole("dialog", { name: "Provider sheet" })).toHaveAttribute("inert");

    await userEvent.click(canvas.getByRole("button", { name: "close the sheet underneath" }));
    await waitFor(() => expect(canvas.getAllByRole("dialog")).toHaveLength(1));
    await expect(opener.closest("[inert]")).not.toBeNull();
    await expect(body.style.overflow).toBe("hidden");

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
    await expect(opener.closest("[inert]")).toBeNull();
    await expect(body.style.overflow).toBe(overflow);
    await expect(canvas.getByTestId("already-inert")).toHaveAttribute("inert");
  },
};

/** the logs filter rail's shape: the panel is the fixed box, its scrim beside it */
function Rail() {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="flex flex-col items-start gap-3">
      <button type="button" onClick={() => setOpen(true)}>
        open the filters
      </button>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        label="Filters"
        className="fixed inset-y-0 left-0 z-50 w-[240px]"
      >
        <button type="button">errors only</button>
      </Modal>
      {open && (
        <div
          data-testid="scrim"
          aria-hidden
          className="fixed inset-0 z-40 bg-black/50"
          onClick={() => setOpen(false)}
        />
      )}
    </div>
  );
}

/**
 * A scrim beside the panel rather than around it stays live. Inert hit-tests
 * as `pointer-events: none`, so an inert scrim would pass a real click through
 * to the page and the dismissal it promises would stop working.
 */
export const AScrimBesideThePanelStaysLive: Story = {
  render: () => <Rail />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const view = canvasElement.ownerDocument.defaultView as Window;
    await userEvent.click(canvas.getByRole("button", { name: "open the filters" }));
    const scrim = await canvas.findByTestId("scrim");
    await waitFor(() => expect(canvas.getByRole("button", { name: "errors only" })).toHaveFocus());
    await expect(scrim.closest("[inert]")).toBeNull();
    await expect(
      canvas.getByRole("button", { name: "open the filters" }).closest("[inert]"),
    ).not.toBeNull();
    // what a real pointer would land on, not what a synthetic click is sent to
    await expect(
      canvasElement.ownerDocument.elementFromPoint(view.innerWidth - 20, view.innerHeight / 2),
    ).toBe(scrim);
    await userEvent.click(scrim);
    await waitFor(() => expect(canvas.queryByRole("dialog")).toBeNull());
  },
};

/**
 * The toast stack is drawn above every modal and holds live regions, which
 * announce nothing from inside an inert subtree. It opts out with
 * `data-above-modals` and stays live.
 */
export const TheToastStackStaysLive: Story = {
  render: () => (
    <div className="flex flex-col items-start gap-3">
      <Harness />
      <div data-above-modals data-testid="toasts" role="status">
        saved
      </div>
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await open(canvas);
    await waitFor(() => expect(canvas.getByLabelText("name")).toHaveFocus());
    await expect(
      canvas.getByRole("button", { name: "open the panel" }).closest("[inert]"),
    ).not.toBeNull();
    await expect(canvas.getByTestId("toasts").closest("[inert]")).toBeNull();
  },
};
