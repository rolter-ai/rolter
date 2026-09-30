import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { resolveColorToken } from "@/lib/story-tokens";

import { IncompleteSpendNotice } from "./IncompleteSpendNotice";

/**
 * The 8-bit channels a computed colour paints, in any colour space the browser
 * serialises it in (`color-mix` comes back as `oklab(...)`): drawn onto a pixel
 * and read back. Translucent colours keep their own channels and alpha.
 */
function channelsOf(color: string): { r: number; g: number; b: number; a: number } {
  const context = document.createElement("canvas").getContext("2d")!;
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
  return { r, g, b, a };
}

const meta = {
  title: "Components/IncompleteSpendNotice",
  component: IncompleteSpendNotice,
} satisfies Meta<typeof IncompleteSpendNotice>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The #969 measurement: 132 requests across 8 models, all unpriced. */
export const ManyUnpriced: Story = {
  args: { requests: 132, models: 8 },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    await expect(notice).toHaveTextContent("132");
    // "floor, not a total" is the load-bearing phrase — it is what stops an
    // operator reading the number as final
    await expect(notice).toHaveTextContent(/floor, not a total/);
  },
};

/**
 * #1994: the glyph was amber on a red wash, two signals for one notice. The
 * wash, the border and the glyph are all the warning hue now, and none of them
 * is the red tint the selection and the brand use. Amber is about two thirds as
 * much green as red; the red tint is a quarter.
 */
export const OneToneOfWarning: Story = {
  args: { requests: 132, models: 8 },
  play: async ({ canvas }) => {
    const notice = await canvas.findByRole("status");
    const style = getComputedStyle(notice);
    for (const color of [style.backgroundColor, style.borderTopColor]) {
      const { r, g, b, a } = channelsOf(color);
      await expect(a).toBeGreaterThan(0);
      await expect(g / r).toBeGreaterThan(0.5);
      await expect(g / r).toBeLessThan(0.8);
      await expect(b / r).toBeLessThan(0.2);
    }
    const glyph = notice.querySelector("svg") as SVGElement;
    await expect(getComputedStyle(glyph).color).toBe(resolveColorToken("--status-warning-text"));
  },
};

export const OneUnpriced: Story = {
  args: { requests: 1, models: 1 },
  play: async ({ canvas }) => {
    // singular in both clauses
    await expect(await canvas.findByRole("status")).toHaveTextContent(
      /1 request in this window is unpriced/,
    );
  },
};

/** Everything priced: the spend figure is a real total, so say nothing. */
export const FullyPriced: Story = {
  args: { requests: 0, models: 0 },
  play: async ({ canvas }) => {
    await expect(canvas.queryByRole("status")).toBeNull();
  },
};
