import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect, userEvent, waitFor, within } from "storybook/test";

import { ScatterPlot, type ScatterPoint } from "./scatter-plot";
import { CHART_WIDTHS, expectLabelsAtTheFloor } from "@/lib/story-charts";

// latency against cost, one point per model — the shape the Playground plots
const POINTS: ScatterPoint[] = [
  { x: 340, y: 0.4, label: "llama-3.1-70b", group: 0 },
  { x: 812, y: 2.5, label: "gpt-4o", group: 1 },
  { x: 1240, y: 3.1, label: "claude-sonnet", group: 2 },
  { x: 210, y: 0.15, label: "fake-llm", group: 3 },
];

const meta = {
  title: "Charts/ScatterPlot",
  component: ScatterPlot,
  parameters: { layout: "padded" },
  args: {
    points: POINTS,
    xLabel: "Latency",
    yLabel: "Cost per 1k",
    xUnit: " ms",
    label: "Latency against cost per thousand tokens",
  },
} satisfies Meta<typeof ScatterPlot>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(
      canvas.getByRole("img", { name: "Latency against cost per thousand tokens" }),
    ).toBeVisible();
    await expect(canvasElement.querySelectorAll("circle")).toHaveLength(POINTS.length);
  },
};

/**
 * The plot is drawn at the width it is given, so its axis and point labels stay
 * at 11px on a phone-width card (#3457).
 */
export const LabelsKeepTheFloorOnANarrowCard: Story = {
  render: (args) => (
    <div className="flex flex-col gap-4">
      {CHART_WIDTHS.map((w) => (
        <div key={w} style={{ width: w }}>
          <ScatterPlot {...args} />
        </div>
      ))}
    </div>
  ),
  play: async ({ canvasElement }) => {
    await expectLabelsAtTheFloor(canvasElement, CHART_WIDTHS.length);
  },
};

/** Groups colour the points from the palette and name them under the plot. */
export const Grouped: Story = {
  args: { groups: ["Open weights", "OpenAI", "Anthropic", "Built-in"] },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("Open weights")).toBeVisible();
    await expect(canvas.getByText("Built-in")).toBeVisible();
  },
};

/**
 * A point carries its own `<title>`, so the reading is available without a
 * pointer; hovering promotes the same figures into the floating tooltip.
 */
export const HoverReadsAPoint: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const dot = canvasElement.querySelectorAll("circle")[1];
    await expect(dot).toBeTruthy();
    await userEvent.hover(dot as Element);
    await waitFor(() => expect(canvas.getByText("gpt-4o")).toBeVisible());
  },
};

/**
 * Unlabelled the graphic is decorative: `role="img"` promises a name and axe
 * fails a story that makes the promise without keeping it (#1181), so a caller
 * that leaves the heading beside the chart to do the naming gets
 * `aria-hidden` instead.
 */
export const UnlabelledIsDecorative: Story = {
  args: { label: undefined },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.queryByRole("img")).not.toBeInTheDocument();
    await expect(canvasElement.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
  },
};

/**
 * No points: the axes still draw against a unit scale rather than collapsing,
 * so an empty window reads as an empty window and not as a broken chart. The
 * caller supplies the sentence that says which one it is.
 */
export const NoPoints: Story = {
  args: { points: [] },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelectorAll("circle")).toHaveLength(0);
    await expect(canvasElement.querySelector("svg")).toBeTruthy();
  },
};
