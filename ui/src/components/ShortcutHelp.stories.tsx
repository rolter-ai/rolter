import type { Meta, StoryObj } from "@storybook/react-vite";
import * as React from "react";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ShortcutHelp } from "./ShortcutHelp";
import { Button } from "@/components/ui/button";
import en from "@/lib/i18n/locales/en.json";
import { SHORTCUTS, chordText } from "@/lib/shortcuts";

const copy = en.shell.shortcuts;

const meta = {
  title: "Overlays/ShortcutHelp",
  component: ShortcutHelp,
  parameters: { layout: "fullscreen" },
  // the demo owns the open state; these satisfy the required-prop type
  args: { open: false, onOpenChange: () => {} },
} satisfies Meta<typeof ShortcutHelp>;

export default meta;
type Story = StoryObj<typeof meta>;

// a trigger of its own so the story has somewhere for focus to go back to —
// in the shell the sheet is opened by a keystroke, and this is the part that
// keystroke cannot stand in for
function Demo() {
  const [open, setOpen] = React.useState(false);
  return (
    <div className="p-6">
      <Button onClick={() => setOpen(true)}>{copy.title}</Button>
      {/* pinned: the runner's own platform must not decide which glyph the
          assertions below are looking for */}
      <ShortcutHelp open={open} onOpenChange={setOpen} apple />
    </div>
  );
}

export const Default: Story = { render: () => <Demo /> };

/**
 * Every registered shortcut is listed, named and printed — the whole point of
 * the sheet. The expectation walks `SHORTCUTS` rather than a list written here,
 * so a shortcut added later is asserted the day it lands rather than the day
 * someone remembers this file.
 */
export const ListsEveryShortcut: Story = {
  render: () => <Demo />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    await userEvent.click(canvas.getByRole("button", { name: copy.title }));

    const dialog = await body.findByRole("dialog", { name: copy.title });
    await expect(dialog).toHaveAccessibleDescription(copy.description);

    const items = copy.items as Record<string, string>;
    for (const shortcut of SHORTCUTS) {
      await expect(within(dialog).getByText(items[shortcut.id]!)).toBeVisible();
      await expect(
        within(dialog).getByRole("img", { name: chordText(shortcut.chord, true) }),
      ).toBeVisible();
    }
    // the mac glyph, because this story pinned it — not `Ctrl`
    await expect(within(dialog).queryByRole("img", { name: "Ctrl+K" })).toBeNull();
  },
};

/**
 * What `aria-modal` promises: focus moves into the panel, Escape closes it,
 * and focus goes back to whatever opened it. A reference sheet that swallowed
 * Escape would be a trap over the screen someone was reading.
 */
export const EscapeClosesAndReturnsFocus: Story = {
  render: () => <Demo />,
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(document.body);
    const opener = canvas.getByRole("button", { name: copy.title });
    await userEvent.click(opener);

    const dialog = await body.findByRole("dialog", { name: copy.title });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));

    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(body.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
  },
};
