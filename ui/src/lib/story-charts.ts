import { expect } from "storybook/test";

/** Viewport widths of a phone and a desktop card, where a chart's labels must still read at 11px (#3450). */
export const CHART_WIDTHS = [320, 375, 640];

/**
 * Asserts that every chart drawn under `root` is drawn at one viewBox unit to
 * the pixel and that its smallest label reads at the dashboard's 11px floor.
 */
export async function expectLabelsAtTheFloor(root: HTMLElement, charts: number) {
  const svgs = Array.from(root.querySelectorAll("svg"));
  await expect(svgs).toHaveLength(charts);
  for (const svg of svgs) {
    const scale = svg.getBoundingClientRect().width / (svg.viewBox.baseVal.width || 1);
    await expect(scale).toBeGreaterThan(0.95);
    await expect(scale).toBeLessThan(1.05);
    const labels = Array.from(svg.querySelectorAll("text"));
    await expect(labels.length).toBeGreaterThan(0);
    for (const text of labels) {
      await expect(parseFloat(getComputedStyle(text).fontSize) * scale).toBeGreaterThanOrEqual(
        10.9,
      );
    }
  }
}
