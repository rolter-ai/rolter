import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { formattersFor } from "@/lib/i18n/format";

import { ModelPriceCell } from "./ModelPriceCell";
import { noteOf } from "@/pages/story-harness";

// the cell renders prices the catalogue screen has already formatted, so the
// sample args come from the same formatter that screen uses
const money = formattersFor("en");
const inPrice = money.currency(0.15);
const outPrice = money.currency(0.6);

const meta = {
  title: "Components/ModelPriceCell",
  component: ModelPriceCell,
} satisfies Meta<typeof ModelPriceCell>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Priced: Story = {
  args: { priced: true, inPrice, outPrice },
  play: async ({ canvas }) => {
    await expect(await canvas.findByText(`${inPrice} · ${outPrice}`)).toBeInTheDocument();
    await expect(canvas.queryByText(/no price set/i)).toBeNull();
  },
};

/** The #969 case: served, recorded, but not accounted for. */
export const Unpriced: Story = {
  args: { priced: false, inPrice: "—", outPrice: "—" },
  play: async ({ canvas }) => {
    const badge = await canvas.findByText(/no price set/i);
    await expect(badge).toBeInTheDocument();
    // the dash must not be what says it: "nothing to show" and "we cannot
    // account for this traffic" are different claims
    await expect(canvas.queryByText(/— · —/)).toBeNull();
    await expect(noteOf(badge)).toContain("unpriced");
  },
};

/**
 * The price catalogue could not be read. Not knowing whether a model is priced
 * is not the same as knowing it is unpriced, so the cell stays quiet.
 */
export const CatalogueUnavailable: Story = {
  args: { priced: null, inPrice: "—", outPrice: "—" },
  play: async ({ canvas }) => {
    await expect(canvas.queryByText(/no price set/i)).toBeNull();
    await expect(await canvas.findByText(/— · —/)).toBeInTheDocument();
  },
};
