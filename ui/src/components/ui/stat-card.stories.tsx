import type { Meta, StoryObj } from "@storybook/react";
import { expect, within } from "storybook/test";

import { StatCard } from "./stat-card";
import { resolveColorToken } from "@/lib/story-tokens";

const meta = {
  title: "Display/StatCard",
  component: StatCard,
  parameters: { layout: "padded" },
  args: { label: "Requests / min", value: "1,284" },
  argTypes: {
    trend: { control: "select", options: ["up", "down", "flat"] },
    tone: { control: "select", options: ["good", "bad", "neutral"] },
  },
} satisfies Meta<typeof StatCard>;

export default meta;
type Story = StoryObj<typeof meta>;

const SUCCESS = "--status-success-text";
const DANGER = "--status-danger-text";

/** the delta line of the tile whose delta reads `text` */
function deltaOf(canvasElement: HTMLElement, text: string): HTMLElement {
  return within(canvasElement).getByText(text);
}

const colorOf = (el: Element) => getComputedStyle(el).color;

export const Default: Story = {};

/**
 * The arrow and the colour are separate calls. Traffic going up is good news,
 * so it keeps the tone its arrow implies; latency going down is good news too,
 * so it says so with `tone="good"` rather than inheriting red from its arrow.
 */
export const Grid: Story = {
  render: () => (
    <div className="grid max-w-3xl grid-cols-1 gap-3 sm:grid-cols-3">
      <StatCard label="Requests / min" value="1,284" delta="+12%" trend="up" />
      <StatCard label="p95 latency" value="342" unit="ms" delta="-8%" trend="down" tone="good" />
      <StatCard label="Error rate" value="0.4" unit="%" delta="0%" trend="flat" />
    </div>
  ),
  play: async ({ canvasElement }) => {
    await expect(colorOf(deltaOf(canvasElement, "+12%"))).toBe(resolveColorToken(SUCCESS));
    await expect(colorOf(deltaOf(canvasElement, "-8%"))).toBe(resolveColorToken(SUCCESS));
    // flat is neutral: the same muted grey as the label above it
    await expect(colorOf(deltaOf(canvasElement, "0%"))).toBe(
      colorOf(within(canvasElement).getByText("Error rate")),
    );
    for (const delta of ["+12%", "-8%", "0%"]) {
      await expect(deltaOf(canvasElement, delta).querySelector("svg")).not.toBeNull();
    }
  },
};

/**
 * A figure that rises is not always good news. A p95 latency up a third keeps
 * its up arrow and reads in the danger text colour. The Dashboard's error-rate
 * tile used to come out green here, because the arrow chose the colour (#1974).
 */
export const BadUp: Story = {
  args: {
    label: "p95 latency",
    value: "980",
    unit: "ms",
    delta: "+34%",
    trend: "up",
    tone: "bad",
  },
  play: async ({ canvasElement }) => {
    const delta = deltaOf(canvasElement, "+34%");
    await expect(colorOf(delta)).toBe(resolveColorToken(DANGER));
    await expect(colorOf(delta)).not.toBe(resolveColorToken(SUCCESS));
    await expect(delta.querySelector("svg")).not.toBeNull();
  },
};

/**
 * A delta that compares with nothing, like a count of errors in the window, is
 * drawn with no arrow at all: an arrow would claim a movement nobody measured.
 * The tone still says what the figure means.
 */
export const NoComparison: Story = {
  args: {
    label: "Error rate",
    value: "5.30",
    unit: "%",
    delta: "7 errors",
    tone: "bad",
  },
  play: async ({ canvasElement }) => {
    const delta = deltaOf(canvasElement, "7 errors");
    await expect(colorOf(delta)).toBe(resolveColorToken(DANGER));
    await expect(delta.querySelector("svg")).toBeNull();
  },
};
