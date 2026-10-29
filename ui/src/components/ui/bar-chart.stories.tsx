import type { Meta, StoryObj } from "@storybook/react";
import { expect } from "storybook/test";

import { BarChart, BarTrack } from "./bar-chart";
import { CHART_WIDTHS, expectLabelsAtTheFloor } from "@/lib/story-charts";

const meta = {
  title: "Charts/BarChart",
  component: BarChart,
  parameters: { layout: "padded" },
} satisfies Meta<typeof BarChart>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    height: 180,
    data: [420, 380, 512, 640, 588, 700, 660],
    labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
    unit: "req",
  },
};

export const TopN: Story = {
  args: {
    height: 180,
    data: [900, 640, 512, 420, 380, 210, 120, 90],
    labels: ["gpt-4o", "claude", "llama", "mistral", "gemini", "groq", "xai", "ollama"],
    topN: 5,
  },
};

/**
 * The vertical chart is drawn at the width it is given, so its labels are read
 * at the 11px they are set at on a phone as well as on a desktop card (#3450).
 */
export const LabelsKeepTheFloorOnANarrowCard: Story = {
  args: Default.args,
  render: (args) => (
    <div className="flex flex-col gap-4">
      {CHART_WIDTHS.map((w) => (
        <div key={w} style={{ width: w }}>
          <BarChart {...args} />
        </div>
      ))}
    </div>
  ),
  play: async ({ canvasElement }) => {
    await expectLabelsAtTheFloor(canvasElement, CHART_WIDTHS.length);
  },
};

/**
 * The single bar the ranked chart draws each row with, and the one a table cell
 * puts beside its number (the dashboard's by-model table): a fill of `share`
 * of a quiet track, clamped to the track.
 */
export const BarInACell: StoryObj = {
  render: () => (
    <div className="flex flex-col gap-2">
      {[0, 0.4, 1, 1.6].map((share) => (
        <BarTrack key={share} share={share} color="var(--chart-1)" className="h-2 w-16" />
      ))}
    </div>
  ),
  play: async ({ canvasElement }) => {
    const fills = Array.from(canvasElement.querySelectorAll<HTMLElement>("[aria-hidden] > span"));
    await expect(fills.map((fill) => fill.style.width)).toEqual(["0%", "40%", "100%", "100%"]);
  },
};
