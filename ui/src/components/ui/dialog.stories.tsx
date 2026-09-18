import type { Meta, StoryObj } from "@storybook/react";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { Button } from "./button";
import { Dialog, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./dialog";

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
