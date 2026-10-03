import type { Meta, StoryObj } from "@storybook/react-vite";
import { expect } from "storybook/test";

import { formattersFor } from "@/lib/i18n/format";

import { LineChart } from "./line-chart";

// the chart takes a formatter rather than owning one; these stories pass the
// same locale-bound money formatter the screens do
const money = formattersFor("en");

const meta = {
  title: "Charts/LineChart",
  component: LineChart,
  parameters: { layout: "padded" },
} satisfies Meta<typeof LineChart>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  args: {
    height: 200,
    labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00"],
    series: [
      { name: "p50", values: [120, 132, 128, 140, 135, 150] },
      { name: "p95", values: [280, 320, 300, 360, 342, 380] },
    ],
    formatValue: (v: number) => `${v} ms`,
  },
};

/**
 * #960: the first and last tick labels used to be sliced by the panel edge —
 * `13:00` read as `3:00` and `21:00` lost its last character — because the plot
 * area was flush with it. Both ends must now be fully inside the viewBox.
 */
export const AxisLabelsAreNotClipped: Story = {
  args: {
    height: 200,
    labels: ["13:00", "15:00", "17:00", "19:00", "21:00"],
    series: [{ name: "spend", values: [1.2, 3.4, 2.8, 4.1, 3.6] }],
    formatValue: (v: number) => money.currency(v),
  },
  play: async ({ canvasElement }) => {
    const svg = canvasElement.querySelector("svg");
    await expect(svg).toBeTruthy();
    const viewBoxWidth = 640;
    const labels = [...canvasElement.querySelectorAll("text")].filter((node) =>
      /^\d{2}:\d{2}$/.test(node.textContent ?? ""),
    );
    await expect(labels.length).toBeGreaterThan(0);
    // an approximate half-width for a 9px monospace `HH:MM`
    const halfLabel = 14;
    for (const label of labels) {
      const x = Number(label.getAttribute("x"));
      await expect(x - halfLabel).toBeGreaterThan(0);
      await expect(x + halfLabel).toBeLessThan(viewBoxWidth);
    }
  },
};

/**
 * The y-axis carries a tick per gridline, so a point can be read against a
 * scale instead of against the single max label the chart used to draw.
 */
export const ReadableYAxisScale: Story = {
  args: {
    height: 200,
    labels: ["00:00", "06:00", "12:00", "18:00"],
    series: [{ name: "spend", values: [0.5, 2, 1.25, 3] }],
    formatValue: (v: number) => money.currency(v),
  },
  play: async ({ canvasElement }) => {
    // whatever the money formatter prefixes an amount with in this locale
    const symbol = money.currency(0).replace(/[\d.,\s]/g, "");
    const ticks = [...canvasElement.querySelectorAll("text")].filter((node) =>
      (node.textContent ?? "").startsWith(symbol),
    );
    await expect(ticks.length).toBeGreaterThanOrEqual(5);
  },
};

/**
 * No data at all: the chart must say so rather than draw a grid and a flat
 * line along zero, which reads as a plotted result.
 */
export const NoData: Story = {
  args: {
    height: 200,
    labels: [],
    series: [],
    formatValue: (v: number) => money.currency(v),
    emptyState: <p className="text-sm text-muted-foreground">No requests in this window.</p>,
  },
  play: async ({ canvas, canvasElement }) => {
    await expect(canvas.getByText("No requests in this window.")).toBeVisible();
    // the axes must be absent, not merely empty
    await expect(canvasElement.querySelector("svg")).toBeNull();
  },
};

/**
 * A series that exists but is flat at zero is genuine data and still plots —
 * distinguishing it from "no data" is the caller's job, because only the
 * caller knows whether zero means "nothing was billed" or "nothing arrived".
 */
export const FlatAtZero: Story = {
  args: {
    height: 200,
    labels: ["00:00", "06:00", "12:00", "18:00"],
    series: [{ name: "spend", values: [0, 0, 0, 0] }],
    formatValue: (v: number) => money.currency(v),
  },
  play: async ({ canvasElement }) => {
    await expect(canvasElement.querySelector("svg")).toBeTruthy();
    await expect(canvasElement.querySelector("path")).toBeTruthy();
  },
};
