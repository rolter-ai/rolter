import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Button } from "./button";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./dialog";
import { Field } from "./field";
import { Input } from "./input";
import { atMobile, atShort, expectInViewport } from "@/lib/story-viewport";
import en from "@/lib/i18n/locales/en.json";

const meta = {
  title: "Overlays/Dialog",
  component: Dialog,
  parameters: { layout: "fullscreen" },
  // the demo owns the open state; these satisfy the required-prop type
  args: { open: false, onOpenChange: () => {}, children: null },
} satisfies Meta<typeof Dialog>;

export default meta;
type Story = StoryObj<typeof meta>;

function Demo() {
  const [open, setOpen] = React.useState(false);
  return (
    <div>
      <Button onClick={() => setOpen(true)}>Delete project</Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogHeader>
          <DialogTitle>Delete project</DialogTitle>
          <DialogDescription>
            This permanently removes the project and its routes. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={() => setOpen(false)}>
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </div>
  );
}

export const Default: Story = { render: () => <Demo /> };

// interaction: the dialog opens on the trigger and Cancel closes it. the panel
// is portalled to document.body, so assert against the document, not the canvas.
export const OpensAndCloses: Story = {
  render: () => <Demo />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    await expect(body.queryByRole("dialog")).toBeNull();

    await userEvent.click(canvas.getByRole("button", { name: "Delete project" }));
    const dialog = await body.findByRole("dialog");
    await expect(dialog).toBeInTheDocument();

    await userEvent.click(body.getByRole("button", { name: "Cancel" }));
    await expect(body.queryByRole("dialog")).toBeNull();
  },
};

// the close glyph is 16px; the button around it must still be a 24px target (#2573)
export const CloseButtonHasA24pxHitArea: Story = {
  render: () => <Demo />,
  play: async ({ canvasElement }) => {
    await userEvent.click(within(canvasElement).getByRole("button", { name: "Delete project" }));
    const close = await within(document.body).findByRole("button", { name: en.common.close });
    const box = close.getBoundingClientRect();
    await expect(box.width).toBeGreaterThanOrEqual(24);
    await expect(box.height).toBeGreaterThanOrEqual(24);
  },
};

// what aria-modal promises (#1181): the panel is labelled by its title, focus
// moves inside on open, Tab cycles within the panel, Escape closes it, and
// focus returns to the control that opened it
export const KeepsFocusAndReturnsIt: Story = {
  render: () => <Demo />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    const opener = canvas.getByRole("button", { name: "Delete project" });
    await userEvent.click(opener);

    const dialog = await body.findByRole("dialog", { name: "Delete project" });
    await expect(dialog).toHaveAccessibleDescription(/permanently removes/i);
    // no form control in a confirmation, so the first focusable — the close
    // button — takes focus (after paint)
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));

    // Tab from the last control wraps to the first
    body.getByRole("button", { name: "Delete" }).focus();
    await userEvent.tab();
    await expect(dialog.contains(document.activeElement)).toBe(true);
    await expect(document.activeElement).toBe(body.getByRole("button", { name: "Close" }));
    // and Shift+Tab from the first wraps to the last
    await userEvent.tab({ shift: true });
    await expect(document.activeElement).toBe(body.getByRole("button", { name: "Delete" }));

    await userEvent.keyboard("{Escape}");
    await expect(body.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(opener));
  },
};

const FIELDS = ["Name", "Slug", "Owner", "Region", "Timeout", "Retries", "Budget", "Notes"];

/** a form taller than a short window, with or without its fields in a body */
function TallForm({ withBody }: { withBody: boolean }) {
  const [open, setOpen] = React.useState(false);
  const fields = FIELDS.map((name) => (
    <Field key={name} label={name} htmlFor={`tall-${name}`}>
      <Input id={`tall-${name}`} />
    </Field>
  ));
  return (
    <div>
      <Button onClick={() => setOpen(true)}>Edit connector</Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogHeader>
          <DialogTitle>Edit connector</DialogTitle>
          <DialogDescription>Where the connector sends and how often it retries.</DialogDescription>
        </DialogHeader>
        {withBody ? (
          <DialogBody className="space-y-4">{fields}</DialogBody>
        ) : (
          <div className="space-y-4">{fields}</div>
        )}
        <DialogFooter>
          <Button variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button onClick={() => setOpen(false)}>Save connector</Button>
        </DialogFooter>
      </Dialog>
    </div>
  );
}

/**
 * A form dialog in a 640×360 window, which is 1280×720 at 200 % zoom (#2003).
 *
 * The panel caps itself at the window and only `DialogBody` scrolls, so the
 * title, the close button and the primary action are all on screen at once.
 * Moving the overlay into a scroll box must not cost what #1998 bought: the
 * page behind stays inert, Tab stays inside, and Escape hands focus back.
 */
export const FitsAShortScreen: Story = {
  ...atShort,
  render: () => <TallForm withBody />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    const opener = canvas.getByRole("button", { name: "Edit connector" });
    await userEvent.click(opener);

    const dialog = await body.findByRole("dialog", { name: "Edit connector" });
    await expectInViewport(dialog);
    await expectInViewport(within(dialog).getByRole("heading", { name: "Edit connector" }));
    await expectInViewport(within(dialog).getByRole("button", { name: "Close" }));
    await expectInViewport(within(dialog).getByRole("button", { name: "Save connector" }));

    // the fields gave way, and the last one is a scroll away inside the body
    const last = within(dialog).getByLabelText("Notes");
    const scroller = last.closest<HTMLElement>("[data-slot=dialog-body]")!;
    await expect(scroller.scrollHeight).toBeGreaterThan(scroller.clientHeight);

    // #1998 still holds with the overlay as the scroll container
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
    await expect(opener.closest("[inert]")).not.toBeNull();
    await expect(dialog.closest("[inert]")).toBeNull();
    last.focus();
    await waitFor(() => expectInViewport(last));
    await userEvent.tab();
    await userEvent.tab();
    await userEvent.tab();
    await expect(dialog.contains(document.activeElement)).toBe(true);

    await userEvent.keyboard("{Escape}");
    await expect(body.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(opener));
    await expect(opener.closest("[inert]")).toBeNull();
  },
};

/**
 * The same form without a `DialogBody`. Nothing tells the panel what may
 * shrink, so it keeps its height and the overlay scrolls the whole of it:
 * the top opens on screen rather than above it, and the footer is reachable
 * by scrolling rather than cut off.
 */
export const ScrollsWithoutABody: Story = {
  ...atShort,
  render: () => <TallForm withBody={false} />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(canvas.getByRole("button", { name: "Edit connector" }));
    const dialog = await within(document.body).findByRole("dialog", { name: "Edit connector" });

    await expectInViewport(within(dialog).getByRole("heading", { name: "Edit connector" }));
    await expectInViewport(within(dialog).getByRole("button", { name: "Close" }));
    await expect(dialog.getBoundingClientRect().height).toBeGreaterThan(window.innerHeight);

    const save = within(dialog).getByRole("button", { name: "Save connector" });
    save.scrollIntoView({ block: "nearest" });
    await waitFor(() => expectInViewport(save));
  },
};

/**
 * The footer wraps instead of running past a phone's edge. The panel is
 * 343px wide at 375, and a long label beside Cancel is wider than that; the
 * primary action stays last, so it drops to the bottom right.
 */
function LongFooter() {
  const [open, setOpen] = React.useState(false);
  return (
    <div>
      <Button onClick={() => setOpen(true)}>Rotate keys</Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogHeader>
          <DialogTitle>Rotate keys</DialogTitle>
          <DialogDescription>Every client has to pick up the new key.</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="ghost" onClick={() => setOpen(false)}>
            Keep the current keys
          </Button>
          <Button onClick={() => setOpen(false)}>Rotate every key in this project</Button>
        </DialogFooter>
      </Dialog>
    </div>
  );
}

export const FooterWrapsOnAPhone: Story = {
  ...atMobile,
  render: () => <LongFooter />,
  play: async ({ canvasElement }) => {
    await userEvent.click(within(canvasElement).getByRole("button", { name: "Rotate keys" }));
    const dialog = await within(document.body).findByRole("dialog", { name: "Rotate keys" });
    const keep = within(dialog).getByRole("button", { name: "Keep the current keys" });
    const rotate = within(dialog).getByRole("button", { name: "Rotate every key in this project" });
    await expectInViewport(keep);
    await expectInViewport(rotate);
    await expect(rotate.getBoundingClientRect().right).toBeLessThanOrEqual(
      dialog.getBoundingClientRect().right,
    );
    await expect(rotate.getBoundingClientRect().top).toBeGreaterThan(
      keep.getBoundingClientRect().top,
    );
  },
};
