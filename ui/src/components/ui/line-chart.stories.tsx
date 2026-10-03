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
    // the viewBox is as wide as the chart is drawn, which is the frame's width
    const viewBoxWidth = svg!.viewBox.baseVal.width;
    const labels = [...canvasElement.querySelectorAll("text")].filter((node) =>
      /^\d{2}:\d{2}$/.test(node.textContent ?? ""),
    );
    await expect(labels.length).toBeGreaterThan(0);
    // an approximate half-width for an 11px monospace `HH:MM`
    const halfLabel = 17;
    for (const label of labels) {
      const x = Number(label.getAttribute("x"));
      await expect(x - halfLabel).toBeGreaterThan(0);
      await expect(x + halfLabel).toBeLessThan(viewBoxWidth);
    }
  },
};

/** the axis text of the chart in `canvasElement`, measured as it is drawn on screen */
function axisOf(canvasElement: HTMLElement) {
  const svg = canvasElement.querySelector("svg") as SVGSVGElement;
  const frame = svg.getBoundingClientRect();
  const scale = frame.width / svg.viewBox.baseVal.width;
  const text = [...svg.querySelectorAll("text")];
  return { frame, scale, text, boxes: text.map((node) => node.getBoundingClientRect()) };
}

const DAY = ["00:00", "01:00", "02:00", "03:00", "04:00", "05:00", "06:00", "07:00"];

/**
 * #1994: the chart was a 640-wide viewBox scaled to whatever the card gave it,
 * so on a phone (233px here) its 9px axis text was drawn at about 3px and the
 * plot was a third as tall as it was set to be. It is drawn at the width it is
 * given: one viewBox unit is one pixel, the text is read at the size it was set,
 * and the x labels are thinned to what fits rather than overlapping.
 */
export const DrawnAtTheWidthItHas: Story = {
  args: {
    height: 220,
    labels: DAY,
    series: [{ name: "spend", values: [4.27, 5.9, 6.9, 7.3, 8.1, 8.8, 7.2, 9.5] }],
    formatValue: (v: number) => money.currency(v),
  },
  render: (args) => (
    <div style={{ width: 233 }}>
      <LineChart {...args} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const { frame, scale, text, boxes } = axisOf(canvasElement);
    await expect(scale).toBeGreaterThan(0.95);
    await expect(scale).toBeLessThan(1.05);
    await expect(frame.height).toBeCloseTo(220, 0);
    for (const node of text) {
      await expect(parseFloat(getComputedStyle(node).fontSize) * scale).toBeGreaterThanOrEqual(10);
    }
    // nothing is clipped by the frame, and no two x labels touch
    for (const box of boxes) {
      await expect(box.left).toBeGreaterThanOrEqual(frame.left);
      await expect(box.right).toBeLessThanOrEqual(frame.right);
    }
    const clock = text
      .map((node, i) => [node, boxes[i]] as const)
      .filter(([node]) => /^\d{2}:\d{2}$/.test(node.textContent ?? ""))
      .map(([, box]) => box);
    await expect(clock.length).toBeGreaterThan(1);
    await expect(clock.length).toBeLessThan(DAY.length);
    for (let i = 1; i < clock.length; i += 1) {
      await expect(clock[i].left).toBeGreaterThanOrEqual(clock[i - 1].right);
    }
  },
};

/**
 * The same chart across a wide card keeps every label, up to the six the axis
 * carries, and still draws at one unit to the pixel.
 */
export const DrawnAtTheWidthItHasWhenWide: Story = {
  args: DrawnAtTheWidthItHas.args,
  render: (args) => (
    <div style={{ width: 720 }}>
      <LineChart {...args} />
    </div>
  ),
  play: async ({ canvasElement }) => {
    const { scale, text } = axisOf(canvasElement);
    await expect(scale).toBeGreaterThan(0.95);
    await expect(scale).toBeLessThan(1.05);
    const clock = text.filter((node) => /^\d{2}:\d{2}$/.test(node.textContent ?? ""));
    await expect(clock).toHaveLength(4);
  },
};

/**
 * A tick label wider than the gutter was clipped at the frame's left edge: the
 * gutter was a fixed 44 units, and `$12,345.67` is wider than that at any size.
 * It is as wide as the longest label.
 */
export const WideTickLabelsGetTheirOwnGutter: Story = {
  args: {
    height: 200,
    labels: ["00:00", "06:00", "12:00", "18:00"],
    series: [{ name: "spend", values: [1200, 4300.5, 8800.25, 12345.67] }],
    formatValue: (v: number) => money.currency(v),
  },
  play: async ({ canvasElement }) => {
    const { frame, boxes, text } = axisOf(canvasElement);
    const ticks = text
      .map((node, i) => [node, boxes[i]] as const)
      .filter(([node]) => (node.textContent ?? "").startsWith("$"));
    await expect(ticks.length).toBeGreaterThanOrEqual(5);
    for (const [, box] of ticks) await expect(box.left).toBeGreaterThanOrEqual(frame.left);
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
