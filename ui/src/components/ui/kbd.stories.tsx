import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, within } from "storybook/test";

import { Kbd, KbdChord } from "./kbd";
import { MOD, shortcutChord } from "@/lib/shortcuts";

const meta = {
  title: "Primitives/Kbd",
  component: Kbd,
  args: { children: "K" },
} satisfies Meta<typeof Kbd>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Single: Story = {};

/**
 * The same chord on both platforms, pinned rather than guessed: the glyph a
 * Mac prints is `⌘K` and everything else prints `Ctrl+K`, and a story that let
 * the runner decide would assert whichever machine it ran on.
 */
export const BothPlatforms: Story = {
  render: () => (
    <div className="flex items-center gap-6">
      <KbdChord chord={shortcutChord("palette")} apple />
      <KbdChord chord={shortcutChord("palette")} apple={false} />
      <KbdChord chord={shortcutChord("navSearch")} apple />
      <KbdChord chord={shortcutChord("help")} apple />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // one accessible name per chord, read as the thing it is rather than as
    // two unrelated letters
    await expect(canvas.getByRole("img", { name: "⌘K" })).toBeVisible();
    await expect(canvas.getByRole("img", { name: "Ctrl+K" })).toBeVisible();
    await expect(canvas.getByRole("img", { name: "/" })).toBeVisible();
    await expect(canvas.getByRole("img", { name: "?" })).toBeVisible();
    // the modifier placeholder never reaches the page
    await expect(canvas.queryByText(MOD)).toBeNull();
  },
};
