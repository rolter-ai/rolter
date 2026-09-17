import type { Meta, StoryObj } from "@storybook/react";
import { expect, waitFor, within } from "storybook/test";

import { Donut } from "./donut";

const meta = {
  title: "Charts/Donut",
  component: Donut,
  parameters: { layout: "padded" },
} satisfies Meta<typeof Donut>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    size: 160,
    segments: [
      { label: "OpenAI", value: 62 },
      { label: "Anthropic", value: 28 },
      { label: "Ollama", value: 10 },
    ],
  },
};

export const Single: Story = {
  args: { size: 160, segments: [{ label: "OpenAI", value: 100 }] },
};

// n providers named by position, weighted so the order is stable
const providers = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ label: `Provider ${i + 1}`, value: n - i }));

// past maxSegments the tail folds into one slice, and its legend row is the
// only place that slice is named, so the label is catalog copy (#1482)
export const RolledUp: Story = {
  args: { size: 160, segments: providers(8) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // five named slices plus the tail of three
    await expect(canvas.getByText("Provider 5")).toBeInTheDocument();
    await expect(canvas.queryByText("Provider 6")).not.toBeInTheDocument();
    await expect(await canvas.findByText("Other (3)")).toBeInTheDocument();
  },
};

// the tail count goes through the locale's number format, not a bare interpolation
export const RolledUpLongTail: Story = {
  args: { size: 160, segments: providers(1205) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText("Other (1,200)")).toBeInTheDocument();
  },
};

export const RolledUpRussian: Story = {
  globals: { locale: "ru" },
  args: { size: 160, segments: providers(1205) },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // russian groups thousands with a no-break space, which \s matches
    await waitFor(() => expect(canvas.getByText(/^Прочие \(1\s200\)$/)).toBeInTheDocument());
    await expect(canvas.queryByText(/Other/)).not.toBeInTheDocument();
  },
};
