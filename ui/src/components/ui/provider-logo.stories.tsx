import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { PROVIDER_KINDS } from "@/lib/api";

import { ProviderLogo } from "./provider-logo";

const meta = {
  title: "Primitives/ProviderLogo",
  component: ProviderLogo,
  parameters: { layout: "padded" },
  args: { kind: "anthropic" },
} satisfies Meta<typeof ProviderLogo>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Mark: Story = {
  play: async ({ canvasElement }) => {
    const mark = canvasElement.querySelector("svg")!;
    await expect(mark).toHaveAttribute("aria-hidden", "true");
    // painted in the text colour, so it follows the theme and never a brand hex
    await expect(mark).toHaveAttribute("fill", "currentColor");
  },
};

export const Monogram: Story = {
  args: { kind: "groq", name: "Groq" },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelector("[data-monogram]")).toHaveAttribute(
      "data-monogram",
      "G",
    );
  },
};

// every kind the dashboard knows, one cell each: a mark or a monogram, never a
// gap. The dashboard theme is dark-only (docs/dev-docs/development/dashboard-theme.md),
// so this grid is the dark rendering and there is no light one to cover
export const EveryKind: Story = {
  render: () => (
    <ul className="grid grid-cols-2 gap-2 text-sm sm:grid-cols-4">
      {PROVIDER_KINDS.map((kind) => (
        <li key={kind} className="flex items-center gap-2">
          <ProviderLogo kind={kind} />
          <span className="font-mono text-xs">{kind}</span>
        </li>
      ))}
    </ul>
  ),
  play: async ({ canvasElement }) => {
    const cells = within(canvasElement).getAllByRole("listitem");
    await expect(cells).toHaveLength(PROVIDER_KINDS.length);
    for (const cell of cells)
      await expect(cell.querySelector("[data-provider-logo]")).not.toBeNull();
  },
};
